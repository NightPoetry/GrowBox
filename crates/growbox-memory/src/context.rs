//! 上下文组装层(P4)—— 记忆置换系统的"换入上下文"那一环。
//!
//! 真理:`设计文档/记忆置换系统-总纲.md` + `用户决策/决策日志.md`(用户原话逐字)。
//! 把无界记忆映射进有限 LLM 上下文窗口,按"稳定→易变"分区,命中 prompt 缓存:
//!   1 system(gui 持有,不在此) 2 工作记忆区(指针调入) 3 8K 最近 ring 4 当前回合。
//!
//! 本模块 **llm 无关**:只产出抽象 `ContextBlock`,由 gui 套"每区独特标记 + 时间戳 +
//! 按时间戳判序"的外壳转成 ChatMessage。置换策略(两态、预算淘汰、ring)在此单测。
//!
//! ★命名(2026-06-16 统一概念)★:本结构 `ContextWindow` = **存放区 / 缓存队列** = 临时记忆,
//! **唯一的"记忆缓存"**——检索到的内容(不论 RAG 还是 L2)都 `page_in` 进这里,这就是 AI"怎么记住"的。
//! 别与两个同名物混淆:
//!   · `cache.rs` 邻域缓存 = **索引区**的 L2 翻图加速器(LFU,非存储;空/满不代表记忆有没有进场);
//!   · LLM prompt KV 缓存 = 远端按字节稳定前缀命中的省钱缓存(下面的两态铁律正是为它服务)。
//! RAG/L2 只是把内容调进本存放区的两种**索引手段**(见 `用户决策/记忆架构-索引区与存放区.md`)。
//!
//! 铁律(决策日志):
//! - 信息两态:上下文里信息只有"在/移除",**原位不动、不随缓存队列重排**;排队的是指针。
//! - 工作区非线性:载入顺序≠时间顺序 → 每块带完整时间戳,提示词按时间戳判先后。
//! - 8K ring 永远最末、着重标记;固定大小、覆盖最旧;有意不为缓存优化。

use growbox_core::Timestamp;

/// 上下文分区。顺序即稳定性:越靠前越稳定(prompt 缓存前缀)。
/// system 与当前回合由 gui 持有,不在此枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    /// 工作记忆区(指针数据区):热指针所指数据,两态(在/移除),原位不动。
    Working,
    /// 8K 最近记忆 ring:永远在最末、着重标记,有意不优化缓存。
    RecentRing,
}

/// 一条常驻数据是经哪种"索引手段"调入的 —— 决定它是**真指针**还是**假指针**
/// (用户原话"将 RAG 的指针作为假指针方便统一在队列和缓存中",见 `用户决策/记忆架构-索引区与存放区.md`)。
/// 两者都 `page_in` 进同一个存放区(缓存队列)、一同参与置换;区别只在**序列语义**:
/// - `Llm`:L2/精确层(LLM 顺序读原文 + 指针图导航)调入 = 真指针,有序列位置。
///   它的"换出留二级锚 + 碎片回收"那套机制在 `retrieval.rs`(retrieve_exact / `secondary` / `fragments`),不在本模块。
/// - `RagFake`:RAG(ANN 向量直接跳、无扫描路径)命中调入 = 假指针,**无序列位置**。
///   ★铁律★:换出缓存队列时**绝不留二级锚、不进碎片系统**(ANN 无扫描 gap,没债可还)。
///   本模块的淘汰(`enforce_budget`)本就不碰 `secondary`/`fragments`,故该铁律在此结构性成立;
///   `RagFake` 标签把它**显式化** + 让面板能数清真/假指针。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// L2/精确层命中(真指针,有序列位置)。也是 `page_in` 的默认来源。
    Llm,
    /// RAG/ANN 命中(假指针,无序列位置;换出不落序列、不进碎片)。
    RagFake,
}

/// 组装出的一块上下文(llm 无关)。每块带完整时间戳(工作区非线性,按时间戳判先后)。
#[derive(Debug, Clone)]
pub struct ContextBlock {
    pub region: Region,
    pub node_id: String,
    pub role: String,
    pub timestamp: Timestamp,
    pub content: String,
}

/// 工作记忆区里的一条常驻数据。`heat` 用于预算超额时淘汰最冷。
#[derive(Debug, Clone)]
struct ResidentBlock {
    node_id: String,
    role: String,
    timestamp: Timestamp,
    content: String,
    heat: u32,
    /// 调入来源(真指针 Llm / 假指针 RagFake);供面板计真假指针、守"假指针不落序列"铁律。
    origin: Origin,
}

/// 工作记忆区的常驻态(跨回合存活,放在 `Memory` 里)。
///
/// 两态铁律:`resident` 按"调入顺序"排列(append-only → 稳定前缀,命中 prompt 缓存);
/// 信息只有"在(在 resident 里)/移除(被淘汰)",**绝不随热度重排**。热度只决定
/// 超预算时淘汰谁,不动幸存者的相对顺序。
#[derive(Debug)]
pub struct ContextWindow {
    /// 常驻工作集,调入顺序。
    resident: Vec<ResidentBlock>,
    /// 工作区字符预算(≈ token;P4d 接设置随模型可调)。
    working_budget_chars: usize,
    /// 8K 最近 ring 字符预算。
    ring_budget_chars: usize,
    /// ★置换计数(2026-06-15 仪表盘 B 组改挂真置换)★:工作区是记忆置换系统的"物理内存"(总纲),
    /// **真置换就发生在这里**——`page_in` 真换入(新块入驻)、`enforce_budget` 真淘汰(超预算挤掉最冷)。
    /// 累计数喂面板「置换率 / 队列占用 / 劳累度」(此前这三表错挂在 L2 邻域边缓存上,RAG 命中时不下沉 → 恒 0)。
    /// RAG/L2 命中都经 `assemble_context` 的 `page_in` 进这里,故此计数对两层都生效。
    /// Nap(`clear`)擦工作集时一并归零 = 每个"两次小息之间的工作期"独立计 churn。
    page_ins: u64,
    evictions: u64,
}

/// 工作区默认预算(字符≈token;P4d 随模型上下文窗口可调)。
pub const DEFAULT_WORKING_CHARS: usize = 48_000;
/// 最近记忆 ring 默认预算(字符≈token;P4d 可调)。
/// ★2026-06-15 由 8K 提到 64K(用户决策):最近原文常驻窗扩大,连续建站时近期上下文整块留在场,
/// 大幅减小"近期契约滑出 ring → 割裂"的窗口(dream-board CSS 漏接即此类)。ring 仍在最末、有意不为缓存优化。
pub const DEFAULT_RING_CHARS: usize = 64_000;

/// 按模型上下文窗口(token)推算"建议的"工作区字符预算(决策:随模型可调 + 留余量)。
/// 粗略换算:中英混合约 3 字符/token;扣掉约 35% 给 system/当前回合/ring/输出余量。
/// 仅作 UI 预填默认,用户可改;0 或过小 → 回退 `DEFAULT_WORKING_CHARS`。
pub fn suggest_working_chars(model_context_tokens: u32) -> usize {
    if model_context_tokens == 0 {
        return DEFAULT_WORKING_CHARS;
    }
    let usable_tokens = (model_context_tokens as f64 * 0.65) as usize;
    let chars = usable_tokens.saturating_mul(3);
    chars.max(DEFAULT_WORKING_CHARS)
}

impl Default for ContextWindow {
    fn default() -> Self {
        Self::new(DEFAULT_WORKING_CHARS, DEFAULT_RING_CHARS)
    }
}

impl ContextWindow {
    pub fn new(working_budget_chars: usize, ring_budget_chars: usize) -> Self {
        ContextWindow {
            resident: Vec::new(),
            working_budget_chars,
            ring_budget_chars,
            page_ins: 0,
            evictions: 0,
        }
    }

    pub fn ring_budget_chars(&self) -> usize {
        self.ring_budget_chars
    }

    /// 调整预算(P4d 随模型 / 用户设置)。任一参数为 0 = 保持当前值不变。
    /// 调小工作区会立即按新预算淘汰最冷,保证不超额。
    pub fn set_budgets(&mut self, working_budget_chars: usize, ring_budget_chars: usize) {
        if working_budget_chars > 0 {
            self.working_budget_chars = working_budget_chars;
        }
        if ring_budget_chars > 0 {
            self.ring_budget_chars = ring_budget_chars;
        }
        self.enforce_budget();
    }

    /// 某节点是否已常驻工作区(两态判定:在 → 不重复拼)。
    pub fn is_resident(&self, node_id: &str) -> bool {
        self.resident.iter().any(|b| b.node_id == node_id)
    }

    /// 常驻条数(可观测)。
    pub fn resident_len(&self) -> usize {
        self.resident.len()
    }

    /// 当前存放区里**假指针(RAG/ANN 命中)**的常驻条数(面板:缓存队列里真/假指针占比)。
    pub fn resident_fake_count(&self) -> usize {
        self.resident.iter().filter(|b| b.origin == Origin::RagFake).count()
    }

    /// 当前存放区里**真指针(L2/精确层命中)**的常驻条数。
    pub fn resident_real_count(&self) -> usize {
        self.resident.iter().filter(|b| b.origin == Origin::Llm).count()
    }

    /// 工作区置换率 [0,1]:真实淘汰相对真实换入的比例(每换入一条平均挤掉多少旧块,封顶 1)。
    /// 0 = 还在填充(没满、未淘汰过);接近 1 = 队列已满、几乎每换入一条就挤掉一条(churn 高)。
    /// 面板「记忆置换率」与疲劳度 evict 项的真实来源(替代此前 L2 邻域边缓存的压力)。
    pub fn replacement_rate(&self) -> f64 {
        if self.page_ins == 0 {
            return 0.0;
        }
        (self.evictions as f64 / self.page_ins as f64).min(1.0)
    }

    /// 累计真实淘汰次数(面板「置换率 / 队列占用」hint;Nap 归零)。
    pub fn evictions(&self) -> u64 {
        self.evictions
    }

    /// 工作区填充率 [0,1]:当前占用字符 / 工作区预算。预算为 0(未配置)时返回 0。
    /// 面板 budget_pct 用(P6 接真)。
    pub fn fill_pct(&self) -> f64 {
        if self.working_budget_chars == 0 {
            return 0.0;
        }
        (self.total_chars() as f64 / self.working_budget_chars as f64).min(1.0)
    }

    /// 清空常驻工作集(小息 Nap:擦黑板,不动磁盘记忆)。预算保持不变;置换计数归零
    /// (Nap 是正常的周期重置 → 队列占用回 0、churn 重新计;满了本是常态,见仪表盘设计)。
    pub fn clear(&mut self) {
        self.resident.clear();
        self.page_ins = 0;
        self.evictions = 0;
    }

    /// 调入一条数据到工作区(两态),**默认真指针来源**(`Origin::Llm`);多数调用 + 测试走这个。
    pub fn page_in(
        &mut self,
        node_id: impl Into<String>,
        role: impl Into<String>,
        timestamp: Timestamp,
        content: impl Into<String>,
        heat: u32,
    ) {
        self.page_in_with_origin(node_id, role, timestamp, content, heat, Origin::Llm);
    }

    /// 带来源标签的调入(`assemble_context` 据检索层打标:RAG→`RagFake` 假指针 / Exact→`Llm` 真指针)。
    /// 两态:已在 → 仅更新热度 + 更新来源(原位不动、不重复);不在 → append 末尾(保前缀稳定),再按预算淘汰最冷。
    /// 注:RAG/L2 命中都经此进同一存放区 → 故 `page_ins`/`evictions` 对两层都生效。
    pub fn page_in_with_origin(
        &mut self,
        node_id: impl Into<String>,
        role: impl Into<String>,
        timestamp: Timestamp,
        content: impl Into<String>,
        heat: u32,
        origin: Origin,
    ) {
        let node_id = node_id.into();
        if let Some(b) = self.resident.iter_mut().find(|b| b.node_id == node_id) {
            b.heat = heat; // 已在上下文:原位不动,只刷新热度供淘汰参考
            b.origin = origin; // 来源取最近一次索引手段:RAG 命中后又被 L2 扫到 → 假指针升真指针(有了被扫路径)
            return;
        }
        self.resident.push(ResidentBlock {
            node_id,
            role: role.into(),
            timestamp,
            content: content.into(),
            heat,
            origin,
        });
        self.page_ins += 1; // 真换入(新块入驻;上面"已驻只刷新热度"已 early-return,不计)
        self.enforce_budget();
    }

    /// 超预算则淘汰最冷(heat 最小;并列淘汰最早调入者)。淘汰 = 信息"移除"态。
    /// 幸存者相对顺序不变(不重排),故只有"末段被砍/中段被挖"会破缓存,无队列重排。
    /// ★假指针铁律★:此处淘汰**只移除常驻块,不碰 `secondary`/`fragments`**(本结构无此引用)→
    /// 任何来源(含 `RagFake`)换出都不落二级锚、不进碎片;真指针的二级锚/碎片由 `retrieval.rs` L2 扫描期建。
    fn enforce_budget(&mut self) {
        while self.total_chars() > self.working_budget_chars && self.resident.len() > 1 {
            // 选最冷:heat 最小;并列时 index 最小(最早)。
            let victim = self
                .resident
                .iter()
                .enumerate()
                .min_by_key(|(i, b)| (b.heat, *i))
                .map(|(i, _)| i);
            match victim {
                Some(i) => {
                    self.resident.remove(i);
                    self.evictions += 1; // 真淘汰(超预算挤掉最冷)→ 喂置换率 / 劳累度
                }
                None => break,
            }
        }
    }

    /// 工作区当前占用(`content.len()` = UTF-8 字节数)。作为 token 的**保守上界代理**:
    /// 中文每字约 3 字节,故按字节卡预算会比真 token 更早淘汰(偏安全)。非精确 tokenizer 计数。
    fn total_chars(&self) -> usize {
        self.resident.iter().map(|b| b.content.len()).sum()
    }

    /// 工作区当前常驻块(调入顺序)。`exclude` 里的 id 跳过(已被 ring 覆盖,不重复拼)。
    pub fn working_blocks(&self, exclude: &[String]) -> Vec<ContextBlock> {
        self.resident
            .iter()
            .filter(|b| !exclude.contains(&b.node_id))
            .map(|b| ContextBlock {
                region: Region::Working,
                node_id: b.node_id.clone(),
                role: b.role.clone(),
                timestamp: b.timestamp,
                content: b.content.clone(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts() -> Timestamp {
        growbox_core::now()
    }

    #[test]
    fn page_in_then_resident() {
        let mut cw = ContextWindow::new(10_000, 8_000);
        assert!(!cw.is_resident("a"));
        cw.page_in("a", "user", ts(), "hello", 1);
        assert!(cw.is_resident("a"));
        assert_eq!(cw.resident_len(), 1);
    }

    #[test]
    fn two_state_no_duplicate() {
        // 同一 id 反复调入:只更新热度,不重复、不挪位。
        let mut cw = ContextWindow::new(10_000, 8_000);
        cw.page_in("a", "user", ts(), "hello", 1);
        cw.page_in("b", "assistant", ts(), "world", 1);
        cw.page_in("a", "user", ts(), "hello", 9); // 再次命中 a
        assert_eq!(cw.resident_len(), 2);
        let blocks = cw.working_blocks(&[]);
        // 顺序仍是 a,b(原位不动,不因 a 被再命中而挪到队尾)
        assert_eq!(blocks[0].node_id, "a");
        assert_eq!(blocks[1].node_id, "b");
    }

    #[test]
    fn budget_evicts_coldest() {
        // 预算只够约 2 条 5 字符;调入 3 条,最冷被淘汰。
        let mut cw = ContextWindow::new(10, 8_000);
        cw.page_in("a", "user", ts(), "11111", 5); // 热
        cw.page_in("b", "user", ts(), "22222", 1); // 冷 → 该被淘汰
        cw.page_in("c", "user", ts(), "33333", 5); // 触发淘汰
        assert!(cw.is_resident("a"));
        assert!(cw.is_resident("c"));
        assert!(!cw.is_resident("b"), "最冷的 b 应被淘汰");
    }

    #[test]
    fn shrinking_budget_evicts_immediately() {
        let mut cw = ContextWindow::new(10_000, 8_000);
        cw.page_in("a", "user", ts(), "11111", 1);
        cw.page_in("b", "user", ts(), "22222", 5);
        assert_eq!(cw.resident_len(), 2);
        // 调小到只够 1 条 → 立即淘汰最冷的 a。
        cw.set_budgets(5, 0);
        assert_eq!(cw.resident_len(), 1);
        assert!(cw.is_resident("b"));
        // ring 传 0 = 不变。
        assert_eq!(cw.ring_budget_chars(), 8_000);
    }

    #[test]
    fn churn_counters_feed_replacement_rate() {
        // ★仪表盘 B 组改挂真置换(2026-06-15)★:工作区真换入/淘汰被计数 → 喂置换率/队列占用/劳累度
        //（此前这三表错挂在 L2 邻域边缓存上,RAG 命中时不下沉 → 恒 0)。
        let mut cw = ContextWindow::new(10, 8_000); // 10 字符预算
        cw.page_in("a", "user", ts(), "11111", 1); // 5 字符,未满
        assert_eq!(cw.evictions(), 0);
        assert_eq!(cw.replacement_rate(), 0.0, "未淘汰 → 置换率 0");
        cw.page_in("b", "user", ts(), "22222", 1); // 共 10,刚好不超
        assert_eq!(cw.evictions(), 0);
        cw.page_in("c", "user", ts(), "33333", 1); // 超预算 → 淘汰最冷
        assert_eq!(cw.evictions(), 1, "超预算应淘汰一条");
        assert!(cw.replacement_rate() > 0.0, "有淘汰 → 置换率 > 0(此前错挂 L2 边缓存恒 0)");
        // 已驻只刷新热度,不算"换入"(不增 churn)。
        let before = cw.replacement_rate();
        cw.page_in("c", "user", ts(), "33333", 9);
        assert_eq!(cw.replacement_rate(), before, "已驻仅刷新热度,不计换入");
        // Nap(clear)归零:占用回 0、churn 重新计(满了是常态,Nap 是正常的周期重置)。
        cw.clear();
        assert_eq!(cw.evictions(), 0);
        assert_eq!(cw.replacement_rate(), 0.0);
        assert_eq!(cw.resident_len(), 0);
        assert_eq!(cw.fill_pct(), 0.0);
    }

    #[test]
    fn origin_tracks_fake_vs_real_and_eviction_is_anchor_free() {
        // 假指针(RagFake)与真指针(Llm)都进同一存放区、一同计数;换出只移除常驻块
        //（本结构不持有 secondary/fragments)→ 假指针铁律"换出不落序列/不进碎片"在此结构性成立。
        let mut cw = ContextWindow::new(10, 8_000); // 10 字符预算
        cw.page_in_with_origin("rag1", "user", ts(), "11111", 1, Origin::RagFake); // 假指针 5 字符,最冷
        cw.page_in_with_origin("l2a", "user", ts(), "22222", 5, Origin::Llm); // 真指针 5 字符,共 10
        assert_eq!(cw.resident_fake_count(), 1);
        assert_eq!(cw.resident_real_count(), 1);
        // 超预算淘汰最冷(rag1 heat1)→ 假指针计数随之减;淘汰只动 resident(见 enforce_budget 铁律注)。
        cw.page_in_with_origin("l2b", "user", ts(), "33333", 9, Origin::Llm);
        assert!(!cw.is_resident("rag1"), "最冷的假指针被换出");
        assert_eq!(cw.resident_fake_count(), 0, "假指针换出后计数归零");
        assert_eq!(cw.evictions(), 1);

        // 默认 page_in = 真指针来源。
        let mut cw2 = ContextWindow::new(10_000, 8_000);
        cw2.page_in("x", "user", ts(), "hi", 1);
        assert_eq!(cw2.resident_real_count(), 1);
        assert_eq!(cw2.resident_fake_count(), 0, "默认来源=真指针");

        // 来源升级:假指针后又被 L2 命中 → 升真指针(两态:原位不动、不重复)。
        let mut cw3 = ContextWindow::new(10_000, 8_000);
        cw3.page_in_with_origin("p", "user", ts(), "data", 1, Origin::RagFake);
        assert_eq!(cw3.resident_fake_count(), 1);
        cw3.page_in_with_origin("p", "user", ts(), "data", 2, Origin::Llm);
        assert_eq!(cw3.resident_len(), 1, "两态:原位不动不重复");
        assert_eq!(cw3.resident_fake_count(), 0, "假升真");
        assert_eq!(cw3.resident_real_count(), 1);
    }

    #[test]
    fn suggest_scales_and_floors() {
        assert_eq!(suggest_working_chars(0), DEFAULT_WORKING_CHARS, "未知窗口回退默认");
        // 小窗口模型不低于默认。
        assert_eq!(suggest_working_chars(8_000), DEFAULT_WORKING_CHARS);
        // 大窗口模型放大。
        assert!(suggest_working_chars(200_000) > DEFAULT_WORKING_CHARS);
    }

    #[test]
    fn working_blocks_exclude_ring() {
        let mut cw = ContextWindow::new(10_000, 8_000);
        cw.page_in("a", "user", ts(), "hello", 1);
        cw.page_in("b", "user", ts(), "world", 1);
        let blocks = cw.working_blocks(&["b".to_string()]);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].node_id, "a");
    }
}
