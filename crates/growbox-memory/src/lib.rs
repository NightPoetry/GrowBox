//! growbox-memory — 存与取:对话/经验/结论的存储 + 分层检索。
//!
//! 实现 `设计文档/系统架构/04-memory.md` 与 `设计/02-记忆检索.md`。
//! 只管存与取;提炼/因果归 learn。

mod cache;
mod context;
mod fragments;
mod index;
mod memory;
pub mod node_kind;
mod pointer;
mod secondary;
pub mod skill_format;
mod store;
pub mod tool_memory_format;
mod subconscious;
mod timeline;

pub use cache::NeighborCache;
pub use context::{
    suggest_working_chars, ContextBlock, ContextWindow, Origin, Region, DEFAULT_RING_CHARS,
    DEFAULT_WORKING_CHARS,
};
pub use fragments::{Fragment, FragmentLedger};
pub use index::{ArroyIndex, BruteForceIndex, HnswIndex, VectorIndex};
pub use memory::{
    DreamReport, FatigueConfig, Hit, Layer, Memory, PointerConfig, PointerMatchMode,
    RetrievalConfig, SkillConfig, SleepReport, TransientCapsConfig,
};
pub use pointer::{Pointer, PointerNet};
pub use secondary::{Anchor, SecondaryIndex};
pub use store::Store;
pub use subconscious::{cosine, Subconscious};
pub use timeline::{Node, NodeMeta, Stain, Timeline};
