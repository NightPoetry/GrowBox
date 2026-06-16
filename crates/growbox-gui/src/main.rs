//! GrowBox 桌面应用入口(Tauri 薄壳)。
//!
//! 只做装配:建运行时状态 → 注册命令 → 起窗口。逻辑全在 lib 的脊柱与各 crate。

// 发布构建不弹控制台窗口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use growbox_gui::cmds;
use growbox_gui::state::AppState;
use growbox_gui::web_debug;
use tauri::Manager;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init()
        .ok();

    let builder = tauri::Builder::default().setup(|app| {
        let data_dir = app
            .path()
            .app_data_dir()
            .unwrap_or_else(|_| std::env::temp_dir().join("growbox"));
        app.manage(std::sync::Arc::new(tokio::sync::Mutex::new(AppState::new(data_dir))));
        // 活的 IDE:UI 往返登记表(独立 managed state,不走 AppState 锁,见 `ui.rs`)。
        app.manage(growbox_gui::ui::UiAckRegistry::default());
        // 用户决定脊柱:shell 审批 / 路径授权 / 隐私确认统一走的 round-trip 登记表 + shell 会话级信任记忆
        // (独立 managed state,不走 AppState 锁,见 `decision.rs`)。
        app.manage(growbox_gui::decision::Decisions::default());
        // 回合级取消(造物交互 v2 §2「可终止」):独立 managed state,cancel_chat 瞬时置位、脊柱每轮读。
        app.manage(growbox_gui::chat_control::ChatControl::default());
        // 实时上下文压力计(主链最近请求的 prompt_tokens;独立原子,不走 AppState 锁,见 `context_meter.rs`)。
        app.manage(growbox_gui::context_meter::ContextMeter::default());

        // 调试 HTTP 端口(:19999)仅测试包(debug-endpoints feature)启动。
        #[cfg(feature = "debug-endpoints")]
        growbox_gui::debug::start_server(app.handle().clone());

        Ok(())
    });

    register_and_run(builder);
}

/// 注册所有命令并启动。基础命令表只写一处;调试命令作为 `$extra` 追加,
/// 由 feature 决定是否带入 —— 正式包与测试包共用同一份基础表,不重复。
macro_rules! run_with {
    ($builder:expr $(, $extra:path)* $(,)?) => {
        $builder.invoke_handler(tauri::generate_handler![
            // 连接 / 运行时
            cmds::app_version,
            cmds::connect,
            cmds::set_runtime_dir,
            cmds::list_models,
            // 对话(脊柱)
            cmds::send_message_stream,
            cmds::send_internal_message,
            cmds::send_message,
            cmds::self_drive_step,
            cmds::cancel_chat,
            cmds::reset_chat_session,
            cmds::set_truncate_tool_display,
            cmds::set_auto_mode,
            cmds::set_danger_mode,
            cmds::get_privacy_dirs,
            cmds::set_privacy_dirs,
            cmds::grant_shell_access,
            // 项目
            cmds::list_projects,
            cmds::current_project,
            cmds::switch_project,
            cmds::create_project,
            cmds::get_project_directories,
            cmds::update_project_directories,
            cmds::pick_directory,
            // 工具 / 状态
            cmds::get_tools,
            cmds::set_tools,
            cmds::get_tool_kinds,
            cmds::get_status,
            cmds::get_health,
            cmds::get_translations,
            cmds::set_prompt_lang,
            cmds::get_notice_catalog,
            cmds::perceive_notice,
            // v1 面板空桩
            cmds::get_control_state,
            cmds::get_memory_stats,
            cmds::get_fatigue_level,
            cmds::get_tasks,
            cmds::set_neighbor_cache_cap,
            cmds::set_pointer_config,
            cmds::set_retrieval_config,
            cmds::set_fatigue_config,
            cmds::set_transient_caps,
            cmds::set_agent_config,
            cmds::get_agent_config,
            cmds::set_idle_config,
            cmds::get_idle_config,
            cmds::set_misc_config,
            cmds::get_misc_config,
            cmds::list_skills,
            cmds::get_skill_body,
            cmds::set_skill_active,
            cmds::set_skill_config,
            cmds::get_skill_config,
            cmds::list_skill_proposals,
            cmds::accept_skill_proposal,
            cmds::reject_skill_proposal,
            cmds::set_lazy_tools_config,
            cmds::set_tool_limits,
            cmds::get_tool_limits,
            cmds::set_web_config,
            cmds::get_web_config,
            cmds::grant_net_host,
            cmds::set_tool_memory_config,
            cmds::get_tool_memory_config,
            cmds::transpile_prompts,
            cmds::set_transpile_enabled,
            cmds::get_transpile_status,
            cmds::set_transpile_concurrency,
            cmds::transpile_list_snapshots,
            cmds::transpile_activate_snapshot,
            cmds::transpile_rename_snapshot,
            cmds::transpile_delete_snapshot,
            cmds::transpile_export_snapshot,
            cmds::transpile_import_snapshot,
            cmds::install_tsserver,
            cmds::tsserver_status,
            cmds::get_chat_history,
            cmds::save_chat_transcript,
            cmds::load_chat_transcript,
            cmds::get_project_conversation_history,
            cmds::get_citation_context,
            cmds::list_sessions,
            cmds::dream_start,
            cmds::dream_status,
            cmds::nap,
            cmds::run_digest_pass,
            cmds::reference_history,
            // 活的 IDE:面板声明 + UI 操作回执 + 可见态上报(推论 7)
            cmds::register_ui_surfaces,
            cmds::ui_action_ack,
            cmds::decision_ack,
            cmds::open_external_url,
            cmds::ui_state_changed,
            // 被造物:造物 UI 交互回传(流2,Phase 2)+ 关闭硬机制(v2 §4)
            cmds::artifact_event,
            cmds::artifact_closed,
            // 交互式终端(人机共驾 shell):用户键入 / 事件点唤醒 / 关闭
            cmds::pty_input,
            cmds::terminal_event,
            cmds::terminal_closed,
            // 自关机能力(关闭自己 / 系统关机,经一次性授权或永久权)
            cmds::do_shutdown,
            cmds::set_auto_shutdown_allowed,
            // 疫苗式预接种 OS 授权(spawn helper 探针触发系统弹窗)
            cmds::vaccinate_permission,
            cmds::set_cache_prewarm,
            cmds::confirm_app_exit,
            cmds::suggestion_response,
            // 二期 D2:MCP server 连接管理(配置持久 + 全量重连 + 实时状态)
            cmds::mcp_set_servers,
            cmds::mcp_get_status,
            // 网页调试(Phase 2):打开可导航 webview 加载本地 URL + 注入套索运行时;改完源刷新调试窗
            web_debug::create_debug_webview,
            web_debug::reload_debug_webview,
            web_debug::web_debug_drive,
            $($extra),*
        ])
    };
}

/// 正式包:不注册任何调试命令。
#[cfg(not(feature = "debug-endpoints"))]
fn register_and_run(builder: tauri::Builder<tauri::Wry>) {
    run_with!(builder)
        .run(tauri::generate_context!())
        .expect("启动 GrowBox 失败");
}

/// 测试包:追加调试 / E2E 命令(端口与 IPC)。
#[cfg(feature = "debug-endpoints")]
fn register_and_run(builder: tauri::Builder<tauri::Wry>) {
    use growbox_gui::debug;
    run_with!(
        builder,
        debug::get_debug_ping,
        debug::debug_capture,
        debug::receive_test_result,
        debug::e2e_report,
        debug::debug_eval,
        // 后端直驱(确定性测 4 块新能力,不靠 LLM 轮盘)
        debug::debug_dispatch,
        debug::debug_consult_tool_memory,
        debug::debug_seed_tool_memory,
        debug::debug_propose_skill,
    )
    .run(tauri::generate_context!())
    .expect("启动 GrowBox 失败");
}
