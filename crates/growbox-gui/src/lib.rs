//! growbox-gui — 组装层:Agent 循环脊柱 + 执行器注册表 + LLM 桥接(+ Tauri 薄壳见 bin)。
//!
//! 实现 `设计文档/系统架构/06-app.md`。脊柱(agent/registry/executors/bridge)不依赖 Tauri,
//! 可独立单测;Tauri 命令薄壳与窗口装配在二进制 `main.rs` 里组装。

pub mod agent;
/// 造物文件夹(`计划/造物交互-v2.md` §6):每个造物的可写持久状态/记忆目录 + 主记忆隔离判据。
pub mod artifact_fs;
pub mod arbiter;
pub mod branch_log;
pub mod bridge;
pub mod chat_control;
pub mod cmds;
/// 用户决定脊柱:凡需用户裁决才能继续的动作(shell 审批 / 路径授权 / 隐私确认)统一走的 round-trip。
pub mod decision;
pub mod context_meter;
/// 调试/E2E 端点,仅 `debug-endpoints` feature 下编译(正式包不含)。
#[cfg(feature = "debug-endpoints")]
pub mod debug;
pub mod executors;
pub mod health;
pub mod idle;
pub mod lsp;
pub mod mcp;
pub mod outline;
pub mod registry;
/// 工作流运行时存储(工作流即动态工具,见 `设计/07-工作流机制.md`):define_workflow 写入、脊柱读取。
pub mod workflow_store;
/// 个人文件夹识别(自动模式隐私网,见 `计划/luminous-dancing-prism`)。
pub mod privacy;
/// OS 授权 helper app 体系(疫苗式持久授权):Contents/Helpers/ 下签名小 app。
pub mod helpers;
/// 交互式终端会话(人机共驾 shell):PTY 引擎 + 会话注册表。
pub mod pty;
pub mod skills;
/// Skill 提议存储(设计/09 S3:idle 飞轮起草的待裁决 skill 提议,capped kv 列表,非记忆节点)。
pub mod skill_proposals;
pub mod state;
pub mod supervisor;
pub mod tasks;
/// 工具文案多语言单一源(见 `计划/luminous-dancing-prism`):label/ui_desc 给 UI(4 国),
/// llm_desc/params 给 LLM(中/英),编译期内嵌 `prompts/tools.i18n.json`。
pub mod tool_i18n;
/// 提示词自转译(自我负责-输入侧,设计/08 推论2):用消费该提示词的模型把喂给模型的提示词按自己风格
/// 重写(decoder 自亲和);覆盖层按(模型,语言,键)分桶,默认关=逐字原文。见 `计划/提示词自转译.md`。
pub mod transpile;
/// 提示词转译版本库("历史提示词"):每次重写存成新版本(gzip+base64 压缩),可列史/加载/改名/删除,
/// 默认原文删不掉 —— 让自转译可后悔。见 `transpile_store`。
pub mod transpile_store;
/// 提示/告知 文案多语言单一源(见 `设计文档/感知告知-双受众.md`):human 给用户(4 国,对外显示),
/// llm 给 LLM(中/英,对内 perceive),编译期内嵌 `prompts/notices.i18n.json`。
pub mod notice_i18n;
/// 告知原语:双受众(对内 perceive + 对外显示)。Phase 1 落对内半(见 `感知告知-双受众.md`)。
pub mod notify;
/// 活的 IDE:UI 操控的共享类型与往返登记表(推论 7,见 `计划/活的IDE-UI执行器.md`)。
pub mod ui;
/// 网页调试(Phase 2):可导航 webview 加载本地 URL + 注入套索 + 本机 HTTP 回传(见 `web_debug_runtime.js`)。
pub mod web_debug;
