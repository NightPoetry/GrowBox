//! 启动 Tauri 窗口 → 等 webview 就绪 → 注入 JS 跑 UI 测试 → 打印结果。
//!
//! 调试 / E2E 专用,仅 `debug-endpoints` feature 下编译(用到 debug_eval/e2e_report 端点):
//!   cargo test -p growbox-gui --features debug-endpoints --test e2e_ui_debug -- --nocapture
#![cfg(feature = "debug-endpoints")]

use std::time::Duration;
use tauri::Manager;

#[test]
fn ui_full_test() {
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path().to_path_buf();

    // 结果通过全局变量回传 (debug_eval → JS invoke e2e_report → oneshot channel)
    let app = tauri::Builder::default()
        .setup(move |app| {
            let state = growbox_gui::state::AppState::new(data_dir.clone());
            app.manage(tokio::sync::Mutex::new(state));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            growbox_gui::debug::debug_eval,
            growbox_gui::debug::e2e_report,
            growbox_gui::cmds::get_status,
            growbox_gui::cmds::list_projects,
            growbox_gui::cmds::get_tools,
            growbox_gui::cmds::get_translations,
            growbox_gui::cmds::get_project_directories,
            growbox_gui::cmds::current_project,
            growbox_gui::cmds::create_project,
            growbox_gui::cmds::switch_project,
            growbox_gui::cmds::pick_directory,
        ])
        .build(tauri::generate_context!())
        .unwrap();

    // 等 webview 完全加载
    std::thread::sleep(Duration::from_secs(4));

    let handle = app.handle().clone();

    let js_tests = vec![
        ("UI 健康检查", "var gb=window.__GROWBOX__; if(gb) JSON.stringify(gb.runFullTest()); else 'no __GROWBOX__'"),
        ("弹窗初始状态", "document.querySelector('.project-create-overlay.visible') ? '弹窗打开' : '弹窗关闭'"),
        ("store 连接状态", "var gt=window.__GROWBOX_TEST__; gt ? JSON.stringify({connected:gt.connected(), projects:gt.projects().length, msgs:gt.messages().length}) : 'no __GROWBOX_TEST__'"),
    ];

    for (name, js) in &js_tests {
        let js_owned = js.to_string();
        let w = handle.get_webview_window("main").unwrap();
        // 直接用 eval (不返回值),JS 结果走 console.log
        let wrapped = format!(
            "console.log('[E2E:{}]', JSON.stringify((function(){{ try {{ var r = ({}); return r; }} catch(e) {{ return 'ERROR:'+String(e); }} }})()));",
            name, js_owned
        );
        let _ = w.eval(&wrapped);
        std::thread::sleep(Duration::from_millis(500));
    }

    println!("\n========================================");
    println!("  UI 测试已注入,检查应用控制台输出");
    println!("  或查看 Tauri webview console:");
    println!("  Console.app → 搜索 growbox-gui");
    println!("========================================\n");

    // 测试项目创建流程
    println!("--- 模拟项目创建流程 ---");

    // 1. 打开弹窗(通过 DOM 点击)
    let open_modal = r#"
        (function(){
            var sidebar=document.querySelector('.sidebar');
            var btn=sidebar?.querySelector('.project-btn');
            if(btn){ btn.click(); }
            setTimeout(function(){
                var action=document.querySelector('.project-dropdown-action');
                if(action) action.click();
            }, 300);
            return 'clicking...';
        })()
    "#;
    let _ = handle.get_webview_window("main").unwrap().eval(
        &format!("console.log('[E2E:打开弹窗]', JSON.stringify({open_modal}))")
    );
    std::thread::sleep(Duration::from_secs(1));

    // 2. 填写表单 + 点确认
    let fill_submit = r#"
        (function(){
            var inputs=document.querySelectorAll('.project-create-panel input');
            var overlay=document.querySelector('.project-create-overlay.visible');
            if(!overlay) return '弹窗未打开';
            var idEl=inputs[0], nameEl=inputs[1];
            if(idEl){
                idEl.value='personal-blog';
                idEl.dispatchEvent(new Event('input',{bubbles:true}));
            }
            if(nameEl){
                nameEl.value='个人博客';
                nameEl.dispatchEvent(new Event('input',{bubbles:true}));
            }
            // 等 SolidJS 响应
            setTimeout(function(){
                var afterId=idEl?inputs[0].value:'?';
                var afterName=nameEl?inputs[1].value:'?';
                console.log('[E2E:填表后]','id='+afterId,'name='+afterName);
                var confirmBtn=document.querySelector('.project-create-panel button.primary');
                if(confirmBtn) confirmBtn.click();
                setTimeout(function(){
                    var stillOpen=document.querySelector('.project-create-overlay.visible');
                    console.log('[E2E:提交后]', stillOpen?'弹窗仍在(可能缺可写目录)':'弹窗已关闭');
                }, 500);
            }, 200);
            return 'filling...';
        })()
    "#;
    let _ = handle.get_webview_window("main").unwrap().eval(&format!("console.log('[E2E:填表提交]', JSON.stringify({fill_submit}))"));
    std::thread::sleep(Duration::from_secs(2));

    println!("测试流程注入完成,应用窗口应该可见");
    println!("保持窗口打开 5 秒供观察...");
    std::thread::sleep(Duration::from_secs(5));

    drop(app);
    println!("完成");
}
