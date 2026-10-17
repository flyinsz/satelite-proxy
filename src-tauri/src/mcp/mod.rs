//! 内置 MCP server：让外部 agent（WorkBuddy / Claude Desktop 等）通过
//! Streamable HTTP 调用与修改 Satelite 配置。
//!
//! - 协议：JSON-RPC 2.0 over `POST /mcp`，非流式 `application/json` 响应
//!   （MCP 2025-03-26/2025-06-18 规范允许；无推送需求，不实现 SSE）
//! - 绑定：仅 `127.0.0.1:{mcp_port}`（默认 2086），无鉴权（用户决策）
//! - 生命周期：`mcp_enabled`（默认 false）开启时随应用启动；设置页开关/
//!   改端口经 `update_settings` 即时启停
//!
//! 模块划分：[`rpc`] 编解码 / [`server`] 传输 / [`tools`] 工具面。

mod rpc;
pub(crate) mod server;
mod tools;

pub(crate) use server::McpServer;

use std::sync::Arc;
use tauri::AppHandle;
use tauri::Manager;

use crate::app_log;
use crate::state::AppState;

/// async 工具名集合：tools/call 分流到 [`tools::dispatch_async`]，
/// 其余进 `spawn_blocking(tools::dispatch)`。
fn is_async_tool(name: &str) -> bool {
    matches!(
        name,
        "smart_switch"
            | "add_subscription"
            | "refresh_subscription"
            | "check_exit_ip"
            | "test_nodes_latency"
            | "network_self_heal"
    )
}

/// 生产 RPC handler：initialize / ping / tools/list 同步回；
/// tools/call 按工具性质分流（async 直接 await，其余 spawn_blocking）。
pub(crate) fn handler_for(app: &AppHandle) -> server::RpcHandler {
    real_handler(app.clone())
}

fn real_handler(app: AppHandle) -> server::RpcHandler {
    Arc::new(move |req: crate::mcp::rpc::RpcRequest| {
        let app = app.clone();
        Box::pin(async move {
            use crate::mcp::rpc::{RpcResponse, ERR_INTERNAL, ERR_INVALID_PARAMS};
            let id = req.id.clone();
            match req.method.as_str() {
                "initialize" => RpcResponse::success(id, initialize_result(&req.params)),
                "ping" => RpcResponse::success(id, serde_json::json!({})),
                "tools/list" => RpcResponse::success(
                    id,
                    serde_json::json!({ "tools": tools::list() }),
                ),
                "tools/call" => match tools::parse_call(&req.params) {
                    Ok((name, args)) => {
                        let result = if is_async_tool(&name) {
                            tools::dispatch_async(&app, &name, args).await
                        } else {
                            let worker = app.clone();
                            let joined = tauri::async_runtime::spawn_blocking(move || {
                                tools::dispatch(&worker, &name, args)
                            })
                            .await;
                            // JoinError → 业务层错误文本（isError:true）
                            match joined {
                                Ok(r) => r,
                                Err(e) => Err(format!("tool task failed: {e}")),
                            }
                        };
                        match result {
                            Ok(value) => RpcResponse::success(
                                id,
                                tools::tool_result_text(value, false),
                            ),
                            Err(msg) => {
                                // 业务失败 → isError:true（MCP 惯例），agent
                                // 可直接读中文消息决定重试/上报。
                                RpcResponse::success(
                                    id,
                                    tools::tool_result_text(
                                        serde_json::json!({ "error": msg }),
                                        true,
                                    ),
                                )
                            }
                        }
                    }
                    Err(msg) => RpcResponse::error(id, ERR_INVALID_PARAMS, msg),
                },
                other => RpcResponse::error(
                    id,
                    crate::mcp::rpc::ERR_METHOD_NOT_FOUND,
                    format!("Method not found: {other}"),
                ),
                #[allow(unreachable_patterns)]
                _ => RpcResponse::error(id, ERR_INTERNAL, "unreachable"),
            }
        })
    })
}

/// `initialize` 响应：客户端请求的版本在支持集内则回显，否则取最新支持版。
fn initialize_result(params: &serde_json::Value) -> serde_json::Value {
    const SUPPORTED: [&str; 2] = ["2025-03-26", "2025-06-18"];
    const LATEST: &str = "2025-06-18";
    let requested = params
        .get("protocolVersion")
        .and_then(|v| v.as_str())
        .unwrap_or(LATEST);
    let negotiated = if SUPPORTED.contains(&requested) {
        requested
    } else {
        LATEST
    };
    serde_json::json!({
        "protocolVersion": negotiated,
        "capabilities": { "tools": {} },
        "serverInfo": {
            "name": "satelite-proxy",
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}

/// 应用启动时挂载：读取设置，`mcp_enabled` 则拉起 server。
/// 阻塞操作全在独立线程，绝不拖慢 setup。
pub fn spawn(app: tauri::AppHandle) {
    std::thread::Builder::new()
        .name("mcp-spawn".into())
        .spawn(move || {
            let Some(state) = app.try_state::<AppState>() else {
                return;
            };
            let (enabled, port) = state
                .with_store(|store| Ok((store.settings.mcp_enabled, store.settings.mcp_port)))
                .unwrap_or((false, 0));
            if !enabled {
                app_log::info("mcp", "MCP server disabled by settings");
                return;
            }
            if let Err(e) = state.start_mcp(port, real_handler(app.clone())) {
                app_log::error("mcp", format!("MCP server start failed: {e}"));
            }
        })
        .expect("spawn mcp thread");
}
