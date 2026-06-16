//! growbox-learn — 飞轮:把每次实践沉淀成可复用、可演化的认知。
//!
//! 实现 `设计文档/系统架构/05-learn.md` 与 `设计/04-飞轮学习.md`。
//! 只管提炼/压缩/调度;存原文与检索归 memory。
//!
//! 落地范围(首版):收集 `Flywheel::collect`、压缩一轮 `Flywheel::turn`(聚类→Reasoner 提炼)、
//! 永久目标调度 `Scheduler`(L0 > P1 > P2 > P3)。验证/泛化需真跑实验,接在 app 的 Agent 循环里。

mod flywheel;
mod scheduler;

pub use flywheel::{cluster, Distillation, Flywheel, ProposedSkill, Reasoner, Snapshot};
pub use scheduler::{Goal, Scheduler};
