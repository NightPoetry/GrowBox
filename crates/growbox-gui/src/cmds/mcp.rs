//! MCP server 连接管理命令(二期 D2)。薄壳:持久配置 → 操作共享 `McpHub` → 回状态。
//!
//! 配置持久进 Settings(`mcp_servers`,write-through 落 redb),跨重启自动重连(见 `connection::connect`)。
//! MCP 工具结果按**外部不可信输入**处理(过一期安全门 + 来源标注,见 `mcp.rs` McpToolExecutor + 05-MCP)。

use serde_json::{json, Value};
use tauri::State;

use growbox_core::McpServerConfig;

use super::SharedState;
use crate::mcp::McpHub;

/// 全量重连:断开所有当前连接 → 按配置连"启用"的 → 返回每个 server 的连接状态。
/// 幂等(每次 apply / 启动都全量重置),不持状态锁(由调用方在锁外 await)。
pub(crate) async fn reconnect_all(hub: &McpHub, configs: &[McpServerConfig]) -> Vec<Value> {
    for name in hub.server_names() {
        hub.disconnect(&name);
    }
    let mut statuses = Vec::new();
    for cfg in configs {
        if !cfg.enabled {
            statuses.push(json!({ "name": cfg.name, "enabled": false, "connected": false, "tool_count": 0 }));
            continue;
        }
        let is_http = cfg.transport.eq_ignore_ascii_case("http");
        if cfg.name.trim().is_empty() || (is_http && cfg.url.trim().is_empty()) || (!is_http && cfg.command.trim().is_empty()) {
            statuses.push(json!({
                "name": cfg.name, "enabled": true, "connected": false,
                "error": if is_http { "name/url 不能为空" } else { "name/command 不能为空" }
            }));
            continue;
        }
        // 传输分流:http → Streamable HTTP(连 url);否则 stdio(spawn command)。
        let result = if is_http {
            hub.connect_http(&cfg.name, &cfg.url).await
        } else {
            let env: Vec<(String, String)> = cfg.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            hub.connect_stdio(&cfg.name, &cfg.command, &cfg.args, &env).await
        };
        match result {
            Ok(tools) => statuses.push(json!({
                "name": cfg.name, "enabled": true, "connected": true, "tool_count": tools.len()
            })),
            Err(e) => statuses.push(json!({
                "name": cfg.name, "enabled": true, "connected": false, "error": e
            })),
        }
    }
    statuses
}

/// 设置 MCP server 配置:落库持久 → 全量重连 → 回每个 server 状态(供前端展示连成/报错)。
#[tauri::command]
pub async fn mcp_set_servers(
    state: State<'_, SharedState>,
    servers: Vec<McpServerConfig>,
) -> Result<Value, String> {
    // 锁内只做"写设置 + 取 hub 句柄",随即释放锁 —— 重连是 await,绝不持状态锁跨 await。
    let hub = {
        let mut st = state.lock().await;
        st.settings.mcp_servers = servers.clone();
        st.save_settings();
        st.registry.mcp_hub()
    };
    let statuses = reconnect_all(&hub, &servers).await;
    Ok(json!({ "servers": statuses }))
}

/// 取 MCP 配置 + 实时连接状态(前端面板加载/刷新用)。纯读,无 await,可短持锁。
#[tauri::command]
pub async fn mcp_get_status(state: State<'_, SharedState>) -> Result<Value, String> {
    let st = state.lock().await;
    let hub = st.registry.mcp_hub();
    let connected = hub.server_names();
    let servers: Vec<Value> = st
        .settings
        .mcp_servers
        .iter()
        .map(|c| {
            json!({
                "name": c.name,
                "command": c.command,
                "args": c.args,
                "enabled": c.enabled,
                "transport": c.transport,
                "url": c.url,
                "connected": connected.contains(&c.name),
                "tool_count": hub.server_tool_count(&c.name),
            })
        })
        .collect();
    // configs 原样回(前端 textarea 回显);servers 带实时状态。
    Ok(json!({ "servers": servers, "configs": st.settings.mcp_servers }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::McpHub;
    use std::collections::BTreeMap;

    /// reconnect_all 对"未启用 / 名或命令为空"的配置短路(不 spawn 子进程),状态如实标注。
    /// 启用且有效的连接需真子进程,已在 `mcp.rs` 的 python 端到端测试覆盖,此处只验编排短路分支。
    #[tokio::test]
    async fn reconnect_all_skips_disabled_and_invalid() {
        let hub = McpHub::new();
        let cfgs = vec![
            McpServerConfig {
                name: "off".into(), command: "whatever".into(), args: vec![], env: BTreeMap::new(), enabled: false,
                transport: "stdio".into(), url: String::new(),
            },
            McpServerConfig {
                name: "".into(), command: "".into(), args: vec![], env: BTreeMap::new(), enabled: true,
                transport: "stdio".into(), url: String::new(),
            },
        ];
        let st = reconnect_all(&hub, &cfgs).await;
        assert_eq!(st.len(), 2);
        assert_eq!(st[0]["enabled"], false);
        assert_eq!(st[0]["connected"], false);
        assert!(st[1]["error"].as_str().unwrap_or("").contains("不能为空"), "空名/命令应报错不连: {}", st[1]);
        assert!(hub.is_empty(), "短路分支不应连任何 server");
    }
}
