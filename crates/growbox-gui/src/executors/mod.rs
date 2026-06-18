//! 内置执行器集合。
//!
//! 一切能力皆"执行器",经唯一注册表 + 唯一分发路径调用(架构公理,见 `系统架构/00`)。

mod artifact_command;
mod ask_user;
mod code_outline;
mod code_search;
mod define_workflow;
mod file;
mod finish;
mod learn_process;
mod learn_skill;
mod load_skill;
mod lsp;
mod note_tool_memory;
mod open_debug_url;
mod project;
mod pty_tools;
mod push_artifact_notice;
mod render_artifact;
mod selftest_artifact;
mod set_appearance;
mod settings;
mod shell;
mod shutdown;
mod task;
mod tool_search;
mod ui_control;
mod web;
mod web_debug_drive;
mod workflow_return;

pub use artifact_command::ArtifactCommand;
pub use ask_user::AskUser;
pub use code_outline::CodeOutline;
pub use code_search::CodeSearch;
pub use define_workflow::DefineWorkflow;
pub use file::{FileEdit, FileList, FileRead, FileWrite};
pub use finish::Finish;
pub use learn_process::{LearnProcess, LEARN_PROCESS};
pub use learn_skill::{LearnSkill, LEARN_SKILL};
pub use load_skill::{LoadSkill, LOAD_SKILL};
pub use lsp::Lsp;
pub use note_tool_memory::{NoteToolMemory, NOTE_TOOL_MEMORY};
pub use open_debug_url::OpenDebugUrl;
pub use project::CreateProject;
pub use pty_tools::{PtyClose, PtyPeek, PtySend, PtyWatch};
pub use push_artifact_notice::PushArtifactNotice;
pub use render_artifact::RenderArtifact;
pub use selftest_artifact::SelftestArtifact;
pub use set_appearance::SetAppearance;
pub use settings::OpenSettings;
pub use shell::Shell;
pub use shutdown::Shutdown;
pub use task::{ListTasks, SpawnTask, WaitTasks};
pub use tool_search::{ToolSearch, TOOL_SEARCH};
pub use ui_control::UiControl;
pub use web::{SharedWebConfig, WebConfig, WebFetch, WebSearch};
pub use web_debug_drive::WebDebugDrive;
pub use workflow_return::{WorkflowReturn, WORKFLOW_RETURN};

use std::sync::Arc;

use growbox_core::Executor;

use crate::lsp::LspManager;
use crate::tasks::TaskManager;
use crate::ui::UiSurfaceCatalog;
use crate::workflow_store::WorkflowStore;

/// 内置执行器全集。后台任务三件套共享同一个 TaskManager;`ui_control` 持前端声明的面板目录;
/// `define_workflow` 与注册表共享同一个 WorkflowStore(它写入、脊柱读取);`lsp` 持 LspManager(语言服务器懒起复用);
/// web 两件套与注册表共享同一份 WebConfig(连接/改设置时写,执行时读)。
pub fn builtins(
    task_mgr: Arc<TaskManager>,
    ui_catalog: UiSurfaceCatalog,
    workflows: Arc<WorkflowStore>,
    lsp_mgr: Arc<LspManager>,
    web_cfg: SharedWebConfig,
) -> Vec<Box<dyn Executor>> {
    vec![
        Box::new(DefineWorkflow::new(workflows)),
        Box::new(FileRead),
        Box::new(FileWrite),
        Box::new(FileEdit),
        Box::new(FileList),
        Box::new(CodeSearch),
        Box::new(CodeOutline),
        Box::new(Shell),
        Box::new(PtySend),
        Box::new(PtyPeek),
        Box::new(PtyClose),
        Box::new(PtyWatch),
        Box::new(Lsp::new(lsp_mgr)),
        Box::new(CreateProject),
        Box::new(OpenSettings),
        Box::new(SetAppearance),
        Box::new(UiControl::new(ui_catalog)),
        Box::new(RenderArtifact),
        Box::new(PushArtifactNotice),
        Box::new(ArtifactCommand),
        Box::new(SelftestArtifact),
        Box::new(OpenDebugUrl),
        Box::new(WebDebugDrive),
        Box::new(WebFetch::new(web_cfg.clone())),
        Box::new(WebSearch::new(web_cfg)),
        Box::new(Shutdown),
        Box::new(Finish),
        Box::new(AskUser),
        Box::new(WorkflowReturn),
        Box::new(LearnProcess),
        Box::new(LearnSkill),
        Box::new(LoadSkill),
        Box::new(NoteToolMemory),
        Box::new(ToolSearch),
        Box::new(SpawnTask::new(task_mgr.clone())),
        Box::new(WaitTasks::new(task_mgr.clone())),
        Box::new(ListTasks::new(task_mgr)),
    ]
}
