//! 潜意识 LLM 仲裁器 —— 共用一个潜意识 LLM 的优先级调度槽(P5 硬前置件)。
//!
//! 真理:`设计/02` 维护节末"对话检索、做梦、飞轮提炼共用一个潜意识 LLM,同时刻只一个用,
//! 优先级 Agent > 睡眠 > 飞轮";`补遗/做梦睡眠期也在检索.md`——做梦/推演自己也走检索,
//! 与前台 Agent 抢的是同一个潜意识 LLM,**竞争是真实的**,造做梦/睡眠之前必须先有仲裁器,
//! 否则一上线就 race。
//!
//! 为什么需要它(而不只靠全局锁 + idle 让位):`run_chat` 全程持 AppState 锁,故前台对后台
//! 天然互斥("Agent > 一切"已由锁 + `last_activity` idle 让位保证)。但**后台之间**——睡眠
//! worker 与飞轮 idle 压缩——它们各自的"想"那一拍(慢 LLM 调用)在 AppState 锁之外进行
//! (见 `idle.rs` 三拍),会真并发同时打潜意识 LLM。仲裁器把它们串起来并按优先级排序:
//! 同时刻只一个后台 LLM 调用在飞,睡眠优先于飞轮;且若前台(Agent 档)来抢,排在最前。
//!
//! 实现 = 容量 1 的优先级互斥闸:`acquire(priority)` 拿到 `ArbiterGuard`(RAII,drop 即释放)。
//! 保证:有更高优先级在排队时,严格更低优先级拿不到下一个槽(高优先级不被低优先级饿死)。
//! 持有中的调用不被抢占(单次 LLM 调用是原子的);抢占发生在"调用之间"——长循环(睡眠/飞轮)
//! 每次 LLM 调用各 acquire 一次,故能在调用间隙让位给新来的高优先级。

use std::sync::Arc;
use parking_lot::Mutex;

use tokio::sync::Notify;

/// 优先级(数值越小越高):Agent > Sleep > Flywheel。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Priority {
    /// 前台用户回合的检索。最高,谁都得让。
    Agent = 0,
    /// 睡眠期(做梦 / 推演)的检索。
    Sleep = 1,
    /// 飞轮 idle 压缩的提炼。最低。
    Flywheel = 2,
}

const LEVELS: usize = 3;

#[derive(Default)]
struct Inner {
    /// 槽是否被占(容量 1)。
    busy: bool,
    /// 各优先级当前在排队的等待者数。
    waiting: [usize; LEVELS],
}

/// 潜意识 LLM 仲裁器(容量 1 的优先级互斥闸)。
pub struct Arbiter {
    inner: Mutex<Inner>,
    notify: Notify,
}

impl Default for Arbiter {
    fn default() -> Self {
        Self::new()
    }
}

impl Arbiter {
    pub fn new() -> Self {
        Arbiter { inner: Mutex::new(Inner::default()), notify: Notify::new() }
    }

    /// 取槽:按优先级排队,拿到返回 RAII 守卫(drop 即释放并唤醒后续)。
    /// 取消安全:在等待中被丢弃(如 CancellationToken 取消任务)会自动注销等待计数。
    pub async fn acquire(&self, p: Priority) -> ArbiterGuard<'_> {
        self.acquire_slot(p).await;
        ArbiterGuard { arbiter: self }
    }

    /// 同 `acquire`,但返回**持有 `Arc` 的 owned 守卫** —— 用于守卫要跨越借用了 AppState 的
    /// 异步段存活(如前台 `run_chat` 整回合持 Agent 档)。
    pub async fn acquire_owned(self: Arc<Self>, p: Priority) -> OwnedArbiterGuard {
        self.acquire_slot(p).await;
        OwnedArbiterGuard { arbiter: self }
    }

    /// 排队取槽的核心:拿到(busy=true)即返回,守卫类型由调用方包。
    async fn acquire_slot(&self, p: Priority) {
        let idx = p as usize;
        self.inner.lock().waiting[idx] += 1;
        // RAII:无论正常拿到还是中途被取消,都注销本等待者计数(避免饿死更低优先级)。
        let dec = WaiterDec { arbiter: self, idx };

        let fut = self.notify.notified();
        tokio::pin!(fut);
        loop {
            // 先登记唤醒意图,再检查条件,避免检查与等待之间丢唤醒。
            fut.as_mut().enable();
            {
                let mut g = self.inner.lock();
                // 有严格更高优先级在排队 → 让它先走(本档不抢)。
                let higher_waiting = g.waiting[..idx].iter().any(|&n| n > 0);
                if !g.busy && !higher_waiting {
                    g.busy = true;
                    drop(g);
                    drop(dec); // 不再是等待者:注销计数
                    return;
                }
            }
            fut.as_mut().await;
            fut.as_mut().set(self.notify.notified());
        }
    }

    fn release(&self) {
        self.inner.lock().busy = false;
        self.notify.notify_waiters(); // 唤醒所有等待者重判,最高可走的优先级胜出
    }

    #[cfg(test)]
    fn waiters(&self, p: Priority) -> usize {
        self.inner.lock().waiting[p as usize]
    }
    #[cfg(test)]
    fn is_busy(&self) -> bool {
        self.inner.lock().busy
    }
}

/// 等待计数的 RAII 注销器(取消安全)。
struct WaiterDec<'a> {
    arbiter: &'a Arbiter,
    idx: usize,
}
impl Drop for WaiterDec<'_> {
    fn drop(&mut self) {
        let mut g = self.arbiter.inner.lock();
        g.waiting[self.idx] = g.waiting[self.idx].saturating_sub(1);
    }
}

/// 持槽守卫:drop 即释放槽并唤醒后续等待者。
pub struct ArbiterGuard<'a> {
    arbiter: &'a Arbiter,
}
impl Drop for ArbiterGuard<'_> {
    fn drop(&mut self) {
        self.arbiter.release();
    }
}

/// 持 `Arc` 的持槽守卫(owned 版,可跨借用段存活)。drop 即释放。
pub struct OwnedArbiterGuard {
    arbiter: Arc<Arbiter>,
}
impl Drop for OwnedArbiterGuard {
    fn drop(&mut self) {
        self.arbiter.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn acquire_release_roundtrip() {
        let arb = Arbiter::new();
        assert!(!arb.is_busy());
        let g = arb.acquire(Priority::Agent).await;
        assert!(arb.is_busy());
        drop(g);
        assert!(!arb.is_busy());
        // 释放后可再取。
        let _g2 = arb.acquire(Priority::Flywheel).await;
        assert!(arb.is_busy());
    }

    /// 持槽时另一个 acquire 必须阻塞,直到释放。
    #[tokio::test]
    async fn second_acquire_blocks_until_release() {
        let arb = Arc::new(Arbiter::new());
        let g = arb.acquire(Priority::Sleep).await;

        let arb2 = arb.clone();
        let (tx, mut rx) = mpsc::channel(1);
        let h = tokio::spawn(async move {
            let _g = arb2.acquire(Priority::Sleep).await;
            tx.send(()).await.unwrap();
        });
        // 等任务排上队。
        while arb.waiters(Priority::Sleep) == 0 {
            tokio::task::yield_now().await;
        }
        assert!(rx.try_recv().is_err(), "占用中,第二个不应拿到");
        drop(g);
        rx.recv().await.unwrap(); // 释放后才拿到
        h.await.unwrap();
    }

    /// 优先级:槽被占且 Flywheel 与 Agent 同时排队,释放后 Agent 先于 Flywheel 拿到。
    #[tokio::test]
    async fn higher_priority_wins_the_queue() {
        let arb = Arc::new(Arbiter::new());
        let hold = arb.acquire(Priority::Agent).await; // 先占住

        let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));

        // 先排 Flywheel(低),确保它在队列里。
        let (a1, o1) = (arb.clone(), order.clone());
        let fly = tokio::spawn(async move {
            let _g = a1.acquire(Priority::Flywheel).await;
            o1.lock().push("flywheel");
            // 持槽片刻,确保顺序可观测。
            tokio::task::yield_now().await;
        });
        while arb.waiters(Priority::Flywheel) == 0 {
            tokio::task::yield_now().await;
        }
        // 再排 Agent(高)。
        let (a2, o2) = (arb.clone(), order.clone());
        let agent = tokio::spawn(async move {
            let _g = a2.acquire(Priority::Agent).await;
            o2.lock().push("agent");
            tokio::task::yield_now().await;
        });
        while arb.waiters(Priority::Agent) == 0 {
            tokio::task::yield_now().await;
        }

        // 两者都在排队,释放占用 → Agent 应先拿到。
        drop(hold);
        fly.await.unwrap();
        agent.await.unwrap();
        assert_eq!(order.lock().as_slice(), &["agent", "flywheel"], "Agent 优先于 Flywheel");
    }
}
