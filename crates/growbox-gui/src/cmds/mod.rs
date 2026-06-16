//! Tauri 命令薄壳 —— 只做"接收前端调用 → 转发给脊柱 → 回结果/抛事件"。
//!
//! 实现 `系统架构/06-app.md`:命令是薄壳,逻辑在 `agent_loop` 与各 crate。
//! 命令名/参数/事件名与复用的前端对齐(camelCase 由 Tauri 自动映射到 snake_case)。
//! 核心流程(连接/对话流/项目/工具/目录)完整实装;v1 专属面板(DLC/做梦/审计/引用)给安全空桩,
//! 让前端加载不报错,后续按需接厚。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State};

use growbox_core::UiIntent;

use crate::agent::{agent_loop, agent_loop_internal, AgentConfig, AgentEvent, EventSink};
use crate::decision::{Decision, DecisionKind, Decisions};
use crate::state::AppState;
use crate::ui::{UiAck, UiAckRegistry, UiSurface};

// UI 往返回执等待 / shell 批准等待 超时已暴露为可设(推论9),由 TauriSink 持有(取自 Settings)。
// UI 落地毫秒级超时即诚实判未生效;shell 批准给用户读命令再裁决的时间,超时按拒绝(安全侧)。

/// Tauri managed state:Arc 包裹 async 锁,可 clone 给 Supervisor。
pub type SharedState = std::sync::Arc<tokio::sync::Mutex<AppState>>;

/// 内置系统提示词(简洁、中文、无 Emoji)。脊柱每轮对话注入。
const SYSTEM_PROMPT: &str = "你是 GrowBox,一个长期陪伴用户、越用越强的桌面 AI 助手。\n\
你能调用工具直接动手:file_read/file_write/file_edit/file_list 读写项目文件,shell 跑命令,create_project 发起新建项目。\n\
你可以用 spawn_task 启动后台命令(构建、测试、起服务等耗时操作),用 wait_tasks 等待完成,用 list_tasks 查看状态。\n\
用户问设置在哪改/想调参数(如循环轮数、输出 token)时,调 open_settings 打开设置并滚动到对应项,把建议值写在 note 里——你不直接改,改由用户定。\n\
用户想开/关/切换某个界面面板(如记忆可视化、做梦、健康监控、对话历史)时,调 ui_control 直接代劳(target=面板标识,op=open/close/toggle),省去用户找按钮——这是你对自身界面的手脚(活的 IDE)。\n\
原则:能自己查证/动手的就别只动嘴;改文件前先读、看清现状再改。\n\
你工作在用户授权的项目沙箱内;越界的读写会被拦下并请用户授权,危险命令会被拒绝——遇到被拦,向用户说明原因而非反复重试。\n\
重要:用户要求新建/创建项目时,直接调 create_project,不要先 file_list 或 file_read 探查目录——\n\
此时项目未建、沙箱不覆盖目标路径,探查必然触发权限弹窗。create_project 会弹出面板让用户选目录。\n\
调 create_project 后你会在此暂停、把控制权交还用户:面板由用户确认或取消,处理完会以新消息驱动你继续——\n\
在那之前绝不要在别处新建文件或假定项目已建好。若用户取消了面板,说明他改主意了,向他确认意图或重新发起,别绕开项目系统硬干。\n\
工具结果会回给你,据此继续推进。任务全部做完后,调用 finish 给出简洁中文总结收口——\n\
这是结束任务的唯一方式:在你调用 finish 之前只输出文字,系统会要求你继续动手,不会停。被外部条件卡住(缺 key/授权/需用户决策)也调 finish 并说明卡点。";


mod chat;
mod config;
mod connection;
mod history;
mod mcp;
mod projects;
mod skill;
mod status;
mod transpile;

// 各域命令经此重导出,保 `cmds::connect` 等路径不变(main.rs generate_handler! 引用)。
pub use chat::*;
pub use config::*;
pub use connection::*;
pub use history::*;
pub use mcp::*;
pub use projects::*;
pub use skill::*;
pub use status::*;
pub use transpile::*;

/// 健康/异常告知状态(见 `异常告知.md`):level = 最高未解除级别,issues = 明细。
/// 跨域共享(connect 连接后推 health-alert + get_status/get_health 都用),故置 mod.rs,子模块经 `use super::*` 取。
///
/// 组装两路:① AppState.health 里登记的问题(如 store 打不开的 Fatal);
/// ② store 运行期写失败(`write_fault`)——write-through 写路径不返回 Result,
/// 失败只 funnel 进 store 内部记账,这里每次轮询读出上浮成 Fatal,确保不静默(铁律)。
fn health_json(st: &AppState) -> Value {
    use crate::health::{Issue, Severity};
    let mut issues = st.health.snapshot();
    let mut level = st.health.worst();
    if let Some(store) = &st.store {
        if let Some((count, last)) = store.write_fault() {
            // code 对齐 catalog(store.write_failed,surface=health):前端按 ui_lang 渲染四国文案。
            let issue = Issue {
                code: "store.write_failed".into(),
                severity: Severity::Fatal,
                params: json!({ "count": count, "last": last }),
            };
            if issue.severity > level {
                level = issue.severity;
            }
            issues.insert(0, issue);
        }
    }
    json!({ "level": level, "issues": issues })
}
