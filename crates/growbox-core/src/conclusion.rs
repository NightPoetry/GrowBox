//! 结论模型 —— 经验/知识/理解是同一个模型在压缩谱上的不同位置。
//!
//! 实现 `设计文档/设计/04-飞轮学习.md` 推论 2:
//! - 压缩程度连续(0~1),飞轮自动调,不手工标。
//! - 信度是算出来的,不是贴的标签。
//! - append-only:不删不改,只缩小适用范围。

use serde::{Deserialize, Serialize};

/// 结论的作用域:项目级 / 全局。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    #[default]
    Project,
    Global,
}

/// 信度 —— 计算值,不是静态标签。
///
/// - 经验:它就是客观事实,无需信度(`ratio()` 返回中性 0.5)。
/// - 知识:支持 ÷(支持 + 矛盾)。
/// - 理解:已验证推论 ÷ 总推论。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Confidence {
    Experience,
    Knowledge { supporting: u32, contradicting: u32 },
    Understanding { verified: u32, total: u32 },
}

impl Confidence {
    /// 算出的信度,值域 [0,1]。
    pub fn ratio(&self) -> f32 {
        match *self {
            Confidence::Experience => 0.5,
            Confidence::Knowledge { supporting, contradicting } => {
                let denom = supporting + contradicting;
                if denom == 0 {
                    0.5
                } else {
                    supporting as f32 / denom as f32
                }
            }
            Confidence::Understanding { verified, total } => {
                if total == 0 {
                    0.0
                } else {
                    verified as f32 / total as f32
                }
            }
        }
    }
}

/// 一条结论。
///
/// `结论 = 前提 + 操作 + 预期后果 + 来源 + 信度`(+ 压缩率 + 取代关系)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Conclusion {
    pub id: String,
    /// 压缩率 0~1:0=原始经验,1=高度压缩的理解。飞轮自动调整。
    pub compression: f32,
    pub prerequisites: Vec<String>,
    pub operation: String,
    pub expected: String,
    /// 来源标注:从哪条经验/知识提炼而来。
    pub source: String,
    pub confidence: Confidence,
    pub scope: Scope,
    /// 被哪个更精确的新版本取代;None = 当前有效。
    pub superseded_by: Option<String>,
    pub created_at: super::Timestamp,
}

impl Conclusion {
    /// 创建一条经验级结论(压缩率 0,信度中性)。
    pub fn experience(operation: impl Into<String>, expected: impl Into<String>, source: impl Into<String>) -> Self {
        let operation = operation.into();
        let created_at = super::now();
        Conclusion {
            id: gen_id("exp", &operation, created_at),
            compression: 0.0,
            prerequisites: Vec::new(),
            operation,
            expected: expected.into(),
            source: source.into(),
            confidence: Confidence::Experience,
            scope: Scope::Project,
            superseded_by: None,
            created_at,
        }
    }

    /// 创建一条提炼出的结论(知识/理解级)。压缩率与信度由飞轮算出后传入。
    ///
    /// 实现 `设计/04` 推论1/2:提炼/压缩把多条经验沉淀为更高压缩率的结论;
    /// 信度算出来(知识=支持÷总,理解=已验÷总),不是贴标签。
    pub fn derived(
        operation: impl Into<String>,
        expected: impl Into<String>,
        source: impl Into<String>,
        compression: f32,
        confidence: Confidence,
    ) -> Self {
        let operation = operation.into();
        let created_at = super::now();
        Conclusion {
            id: gen_id("der", &operation, created_at),
            compression: compression.clamp(0.0, 1.0),
            prerequisites: Vec::new(),
            operation,
            expected: expected.into(),
            source: source.into(),
            confidence,
            scope: Scope::Project,
            superseded_by: None,
            created_at,
        }
    }

    /// 设定最少前提(压缩阶段:从知识反推最少前提)。
    pub fn with_prerequisites(mut self, prerequisites: Vec<String>) -> Self {
        self.prerequisites = prerequisites;
        self
    }

    /// 是否仍然有效(未被取代)。
    pub fn is_active(&self) -> bool {
        self.superseded_by.is_none()
    }

    /// 检索排序权重:信度越高、压缩越深,越靠前。
    ///
    /// 实现 `设计/02-记忆检索`——理解层(高压缩、已验证)排在前面。
    pub fn rank_weight(&self) -> f32 {
        self.confidence.ratio() * (1.0 + self.compression)
    }
}

/// 由类型前缀 + 内容 + 时间生成稳定 ID。
fn gen_id(prefix: &str, content: &str, ts: super::Timestamp) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(content.as_bytes());
    h.update(ts.to_rfc3339().as_bytes());
    let digest = h.finalize();
    format!("{prefix}-{:x}", digest)[..prefix.len() + 13].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn experience_ratio_is_neutral() {
        assert_eq!(Confidence::Experience.ratio(), 0.5);
    }

    #[test]
    fn knowledge_ratio() {
        let c = Confidence::Knowledge { supporting: 3, contradicting: 1 };
        assert!((c.ratio() - 0.75).abs() < 1e-6);
    }

    #[test]
    fn understanding_ratio() {
        let c = Confidence::Understanding { verified: 2, total: 4 };
        assert_eq!(c.ratio(), 0.5);
        let empty = Confidence::Understanding { verified: 0, total: 0 };
        assert_eq!(empty.ratio(), 0.0);
    }

    #[test]
    fn experience_is_active_by_default() {
        let c = Conclusion::experience("加 JSON 要求", "输出变 JSON", "EXP-1");
        assert!(c.is_active());
        assert_eq!(c.compression, 0.0);
    }

    #[test]
    fn higher_confidence_ranks_higher() {
        let mut understanding = Conclusion::experience("op", "exp", "src");
        understanding.confidence = Confidence::Understanding { verified: 9, total: 10 };
        understanding.compression = 0.9;
        let experience = Conclusion::experience("op", "exp", "src");
        assert!(understanding.rank_weight() > experience.rank_weight());
    }
}
