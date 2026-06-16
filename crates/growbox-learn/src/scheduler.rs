//! 调度 —— idle 时飞轮按永久目标自驱,永不停转。
//!
//! 实现 `设计/04` 推论4:有用户任务时 L0 最高优先级全力完成;
//! idle 时按 P1(元优化)→ P2(探索)→ P3(自动化)轮转。三条是写入初始飞轮的种子目标,
//! 用户可随时加 P4、P5……(开放可扩展)。

use serde::{Deserialize, Serialize};

/// 一个永久目标。`tier`:0 = L0(用户任务,最高);1=P1;2=P2;3=P3;4+ = 用户扩展。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Goal {
    pub tier: u8,
    pub name: String,
    pub description: String,
}

impl Goal {
    fn new(tier: u8, name: impl Into<String>, description: impl Into<String>) -> Self {
        Goal { tier, name: name.into(), description: description.into() }
    }
}

/// 永久目标调度器。持有 idle 目标环,按轮转推进;有用户任务则让位 L0。
pub struct Scheduler {
    /// idle 目标(tier ≥ 1),按 tier 升序推进。
    idle_goals: Vec<Goal>,
    /// 轮转游标。
    cursor: usize,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::with_seeds()
    }
}

impl Scheduler {
    /// 写入初始飞轮的三条种子目标(P1/P2/P3)。
    pub fn with_seeds() -> Self {
        Scheduler {
            idle_goals: vec![
                Goal::new(1, "元优化", "让检索/索引/组织越来越快(磨刀):引擎优化引擎"),
                Goal::new(2, "探索冲动", "验证未验证猜想——不存在真正的噪音,只有暂未解释的信号"),
                Goal::new(3, "最大化自动化", "能自动的绝不手动,消除人工中间人角色"),
            ],
            cursor: 0,
        }
    }

    /// 用户扩展一条永久目标(P4、P5……),tier 顺延。
    pub fn push_goal(&mut self, name: impl Into<String>, description: impl Into<String>) {
        let tier = self.idle_goals.len() as u8 + 1;
        self.idle_goals.push(Goal::new(tier, name, description));
    }

    /// 取下一个要推进的目标。
    /// 有用户任务 → 返回 L0(不动 idle 游标);idle → 轮转 P1→P2→P3→…。
    pub fn next(&mut self, has_user_task: bool) -> Goal {
        if has_user_task {
            return Goal::new(0, "用户任务", "全力完成当前用户任务(L0,最高优先级)");
        }
        let g = self.idle_goals[self.cursor].clone();
        self.cursor = (self.cursor + 1) % self.idle_goals.len();
        g
    }

    /// 当前 idle 目标清单(只读,供观测/前端展示)。
    pub fn idle_goals(&self) -> &[Goal] {
        &self.idle_goals
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_are_p1_p2_p3() {
        let s = Scheduler::with_seeds();
        assert_eq!(s.idle_goals().len(), 3);
        assert_eq!(s.idle_goals()[0].tier, 1);
        assert_eq!(s.idle_goals()[2].tier, 3);
    }

    #[test]
    fn user_task_is_l0() {
        let mut s = Scheduler::with_seeds();
        let g = s.next(true);
        assert_eq!(g.tier, 0);
        // L0 不消耗 idle 游标:随后 idle 仍从 P1 起。
        assert_eq!(s.next(false).tier, 1);
    }

    #[test]
    fn idle_round_robins() {
        let mut s = Scheduler::with_seeds();
        let tiers: Vec<u8> = (0..4).map(|_| s.next(false).tier).collect();
        assert_eq!(tiers, vec![1, 2, 3, 1], "P1→P2→P3→回 P1");
    }

    #[test]
    fn user_can_extend_goals() {
        let mut s = Scheduler::with_seeds();
        s.push_goal("自定义", "用户加的 P4");
        assert_eq!(s.idle_goals().len(), 4);
        assert_eq!(s.idle_goals()[3].tier, 4);
    }
}
