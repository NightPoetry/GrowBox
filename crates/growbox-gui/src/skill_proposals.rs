//! Skill 提议存储(设计/09 S3 = 飞轮自学)。
//!
//! idle 飞轮看到同类经验反复成模式时,经 `Reasoner::propose_skill` 起草一个 skill **提议**(结晶谱
//! 「经验 → Skill」)。提议是**待用户裁决的建议、不是知识**,所以存这里(一个有上限的列表,落 redb kv),
//! **不进记忆节点**——避免半成品草稿污染语义召回/自动注入。用户**采纳**才经 `crystallize_skill` 变成真正
//! 的 skill 节点;**丢弃**则记入"不再提"名单(防反复打扰)。
//!
//! 三道防膨胀(设计/09 推论8 精神 + S3「idle 提议要防膨胀,后置」):① 待裁决队列容量上限(满了不再新增)
//! ② 同名/已存在 skill 去重 ③ 拒过的名不再提。再加 idle 每次激活至多提 1 条(在 `idle.rs`)。

use serde::{Deserialize, Serialize};

/// 待用户裁决的一条 skill 提议。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillProposal {
    /// 稳定 id(采纳/丢弃按它定位)。
    pub id: String,
    /// kebab-case 名(采纳后即 skill 名)。
    pub name: String,
    /// 触发描述(一句话,何时用)。
    pub trigger: String,
    /// playbook 正文(markdown)。
    pub body: String,
    /// 起草依据摘要(来源经验,给用户判断"凭什么提这个")。
    #[serde(default)]
    pub rationale: String,
    /// 创建时刻(epoch millis)。
    #[serde(default)]
    pub created_ms: i64,
}

/// 提议存储:待裁决队列 + 已拒名单。整体落一个 kv 键(`skill_proposals`)。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillProposalStore {
    /// 待用户裁决的提议(FIFO 展示)。
    pub pending: Vec<SkillProposal>,
    /// 用户拒过的提议名(小写)——不再重复提议。
    #[serde(default)]
    pub rejected: Vec<String>,
}

/// 待裁决队列容量上限(防膨胀:满了就先让用户消化,不再新增)。
pub const MAX_PENDING: usize = 12;

impl SkillProposalStore {
    /// 这个名是否被用户拒过(不再提)。
    pub fn is_rejected(&self, name: &str) -> bool {
        let n = name.to_ascii_lowercase();
        self.rejected.iter().any(|r| r == &n)
    }

    /// 队列里是否已有同名待裁决提议。
    pub fn has_pending(&self, name: &str) -> bool {
        self.pending.iter().any(|p| p.name.eq_ignore_ascii_case(name))
    }

    /// 还能否接纳新提议(未满)。
    pub fn has_room(&self) -> bool {
        self.pending.len() < MAX_PENDING
    }

    /// 入队一条提议(调用方已做去重 + 容量判断)。
    pub fn push(&mut self, p: SkillProposal) {
        self.pending.push(p);
    }

    /// 按 id 取出(采纳时:取出后交 crystallize_skill)。无则 None。
    pub fn take(&mut self, id: &str) -> Option<SkillProposal> {
        let i = self.pending.iter().position(|p| p.id == id)?;
        Some(self.pending.remove(i))
    }

    /// 按 id 丢弃 + 记入"不再提"名单。返回被丢弃的提议(无则 None)。
    pub fn reject(&mut self, id: &str) -> Option<SkillProposal> {
        let p = self.take(id)?;
        let n = p.name.to_ascii_lowercase();
        if !self.rejected.iter().any(|r| r == &n) {
            self.rejected.push(n);
        }
        Some(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(id: &str, name: &str) -> SkillProposal {
        SkillProposal {
            id: id.into(),
            name: name.into(),
            trigger: "t".into(),
            body: "b".into(),
            rationale: String::new(),
            created_ms: 0,
        }
    }

    #[test]
    fn push_take_roundtrip() {
        let mut s = SkillProposalStore::default();
        s.push(mk("p1", "alpha"));
        s.push(mk("p2", "beta"));
        assert!(s.has_pending("ALPHA"), "大小写不敏感");
        let taken = s.take("p1").unwrap();
        assert_eq!(taken.name, "alpha");
        assert!(!s.has_pending("alpha"), "取出后不在队列");
        assert_eq!(s.pending.len(), 1);
        assert!(s.take("nope").is_none());
    }

    #[test]
    fn reject_records_name_and_blocks_future() {
        let mut s = SkillProposalStore::default();
        s.push(mk("p1", "Gamma"));
        let r = s.reject("p1").unwrap();
        assert_eq!(r.name, "Gamma");
        assert!(s.is_rejected("gamma"), "拒过的名进黑名单(小写)");
        assert!(s.pending.is_empty());
    }

    #[test]
    fn room_caps_at_max_pending() {
        let mut s = SkillProposalStore::default();
        for i in 0..MAX_PENDING {
            assert!(s.has_room());
            s.push(mk(&format!("p{i}"), &format!("s{i}")));
        }
        assert!(!s.has_room(), "满了不再接纳");
    }

    #[test]
    fn survives_json_roundtrip() {
        let mut s = SkillProposalStore::default();
        s.push(mk("p1", "alpha"));
        s.rejected.push("beta".into());
        let j = serde_json::to_string(&s).unwrap();
        let back: SkillProposalStore = serde_json::from_str(&j).unwrap();
        assert_eq!(back.pending.len(), 1);
        assert!(back.is_rejected("beta"));
    }
}
