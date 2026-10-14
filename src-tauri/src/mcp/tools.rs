//! MCP 工具注册表与分发。
//!
//! 31 个精选工具覆盖应用的全部配置面（查询 / 出口切换 / 内核控制 /
//! 订阅 / 规则集 / PAC / 设置），内部直接复用 AppState 层方法与既有
//! 命令的业务闭包——`State<'_, AppState>` 只是 `&AppState` 的包装，
//! MCP handler 拿到 `AppHandle` 后 `try_state::<AppState>()` 即可。
//!
//! `dispatch` 是同步阻塞函数（由 server.rs 丢进 `spawn_blocking` 执行）；
//! 业务失败返回 `Err(中文消息)` → 上层转为 `isError: true` 的 tool result
//! （MCP 惯例），协议层错误才走 JSON-RPC error。

use serde_json::{json, Value};
use tauri::AppHandle;
use tauri::Manager;

use crate::error::AppError;
use crate::state::AppState;

/// 单个工具的静态描述。
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

fn objSchema(props: Value, required: &[&str]) -> Value {
    let mut schema = json!({
        "type": "object",
        "properties": props,
        "additionalProperties": false,
    });
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    schema
}

/// 工具注册表：`ToolDef` 的 schema 含运行时构造的 JSON，用 OnceLock 惰性构建
/// （static 上下文不能调用非 const fn）。
fn tool_defs() -> &'static [ToolDef] {
    static DEFS: std::sync::OnceLock<Vec<ToolDef>> = std::sync::OnceLock::new();
    DEFS.get_or_init(build_tool_defs)
}

fn build_tool_defs() -> Vec<ToolDef> {
    vec![
    // —— 只读 11 ——
    ToolDef {
        name: "get_status",
        description: "查询代理运行状态：内核状态/流量/当前出口/系统代理与 TUN",
        input_schema: objSchema(json!({}), &[]),
    },
    ToolDef {
        name: "get_settings",
        description: "读取全部应用设置（端口/模式/内核/偏好等）",
        input_schema: objSchema(json!({}), &[]),
    },
    ToolDef {
        name: "list_subscriptions",
        description: "列出全部订阅（id/名称/启用状态/节点数）",
        input_schema: objSchema(json!({}), &[]),
    },
    ToolDef {
        name: "list_nodes",
        description: "列出节点（可按订阅过滤）",
        input_schema: objSchema(
            json!({
                "subscription_id": {"type": "string", "description": "按订阅 id 过滤（可选）"},
                "limit": {"type": "integer", "description": "返回条数上限，默认 100"},
            }),
            &[],
        ),
    },
    ToolDef {
        name: "list_rule_sets",
        description: "列出规则集（id/名称/启用/策略/规则数）",
        input_schema: objSchema(json!({}), &[]),
    },
    ToolDef {
        name: "list_pools",
        description: "列出节点池（含池内成员节点数）",
        input_schema: objSchema(json!({}), &[]),
    },
    ToolDef {
        name: "list_chains",
        description: "列出代理链（跳数/落地池）",
        input_schema: objSchema(json!({}), &[]),
    },
    ToolDef {
        name: "get_dns_settings",
        description: "读取 DNS 设置",
        input_schema: objSchema(json!({}), &[]),
    },
    ToolDef {
        name: "get_pac_status",
        description: "PAC 状态快照（模式/启用/URL/名单规模）",
        input_schema: objSchema(json!({}), &[]),
    },
    ToolDef {
        name: "list_connections",
        description: "列出当前活动连接（经内核连接快照）",
        input_schema: objSchema(
            json!({"limit": {"type": "integer", "description": "返回条数上限，默认 50"}}),
            &[],
        ),
    },
    ToolDef {
        name: "get_core_log_tail",
        description: "读取当前内核日志尾部",
        input_schema: objSchema(
            json!({"lines": {"type": "integer", "description": "行数，默认 50，上限 1000"}}),
            &[],
        ),
    },
    // —— 出口切换 4 ——
    ToolDef {
        name: "switch_node",
        description: "切换当前出口到指定节点（热切+持久化，必要时排队重启）",
        input_schema: objSchema(
            json!({"node_id": {"type": "string", "description": "节点 id（list_nodes 返回的 id）"}}),
            &["node_id"],
        ),
    },
    ToolDef {
        name: "select_pool",
        description: "切换当前出口到指定节点池",
        input_schema: objSchema(
            json!({"pool_id": {"type": "string"}}),
            &["pool_id"],
        ),
    },
    ToolDef {
        name: "select_chain",
        description: "切换当前出口到指定代理链",
        input_schema: objSchema(
            json!({"chain_id": {"type": "string"}}),
            &["chain_id"],
        ),
    },
    ToolDef {
        name: "smart_switch",
        description: "立即探测所有节点并切换到最优节点（智能选优）",
        input_schema: objSchema(json!({}), &[]),
    },
    // —— 内核控制 6 ——
    ToolDef {
        name: "start_proxy",
        description: "启动代理内核",
        input_schema: objSchema(json!({}), &[]),
    },
    ToolDef {
        name: "stop_proxy",
        description: "停止代理内核",
        input_schema: objSchema(json!({}), &[]),
    },
    ToolDef {
        name: "restart_proxy",
        description: "重启代理内核",
        input_schema: objSchema(json!({}), &[]),
    },
    ToolDef {
        name: "set_capture_mode",
        description: "设置流量接管模式（off=关闭 / system=系统代理 / tun=虚拟网卡）",
        input_schema: objSchema(
            json!({"mode": {"type": "string", "enum": ["off", "system", "tun"]}}),
            &["mode"],
        ),
    },
    ToolDef {
        name: "set_outbound_mode",
        description: "设置出站模式（rule=规则 / global=全局 / direct=直连）",
        input_schema: objSchema(
            json!({"mode": {"type": "string", "enum": ["rule", "global", "direct"]}}),
            &["mode"],
        ),
    },
    ToolDef {
        name: "set_core_type",
        description: "切换内核（singbox / xray / mihomo），运行中会排队重启",
        input_schema: objSchema(
            json!({"core": {"type": "string", "enum": ["singbox", "xray", "mihomo"]}}),
            &["core"],
        ),
    },
    // —— 订阅 3 ——
    ToolDef {
        name: "add_subscription",
        description: "从 URL 添加订阅并导入节点",
        input_schema: objSchema(
            json!({
                "url": {"type": "string", "description": "订阅地址（http/https）"},
                "name": {"type": "string", "description": "订阅显示名（可选）"},
            }),
            &["url"],
        ),
    },
    ToolDef {
        name: "refresh_subscription",
        description: "刷新订阅拉取最新节点（不传 id 则刷新全部）",
        input_schema: objSchema(
            json!({"id": {"type": "string", "description": "订阅 id（可选，缺省刷新全部）"}}),
            &[],
        ),
    },
    ToolDef {
        name: "remove_subscription",
        description: "删除订阅及其节点",
        input_schema: objSchema(
            json!({"id": {"type": "string"}}),
            &["id"],
        ),
    },
    // —— 规则集 2 ——
    ToolDef {
        name: "set_rule_set_enabled",
        description: "启用/停用规则集",
        input_schema: objSchema(
            json!({
                "id": {"type": "string"},
                "enabled": {"type": "boolean"},
            }),
            &["id", "enabled"],
        ),
    },
    ToolDef {
        name: "set_rule_set_strategy",
        description: "切换规则集路由策略（proxy=代理 / direct=直连 / block=屏蔽 / smart=智能）",
        input_schema: objSchema(
            json!({
                "id": {"type": "string"},
                "strategy": {"type": "string", "enum": ["proxy", "direct", "block", "smart"]},
            }),
            &["id", "strategy"],
        ),
    },
    // —— PAC 1 ——
    ToolDef {
        name: "update_pac_list",
        description: "向 PAC 自定义域名名单添加或移除域名（后缀匹配）",
        input_schema: objSchema(
            json!({
                "action": {"type": "string", "enum": ["add", "remove"]},
                "domains": {"type": "array", "items": {"type": "string"}},
            }),
            &["action", "domains"],
        ),
    },
    // —— 设置 3 ——
    ToolDef {
        name: "update_proxy_settings",
        description: "修改代理核心设置（端口/局域网/QUIC/兜底路由），变更后自动排队重启",
        input_schema: objSchema(
            json!({
                "mixed_port": {"type": "integer", "description": "混合代理端口"},
                "api_port": {"type": "integer", "description": "内核 API 端口"},
                "allow_lan": {"type": "boolean"},
                "bypass_lan": {"type": "boolean"},
                "block_quic": {"type": "boolean"},
                "route_final": {"type": "string", "enum": ["proxy", "direct", "block"]},
            }),
            &[],
        ),
    },
    ToolDef {
        name: "update_app_prefs",
        description: "修改应用偏好（语言/主题/托盘/开机自启等）",
        input_schema: objSchema(
            json!({
                "locale": {"type": "string", "enum": ["zh", "en"]},
                "theme": {"type": "string", "enum": ["aerospace", "day"]},
                "silent_start": {"type": "boolean"},
                "auto_start_proxy": {"type": "boolean"},
                "close_to_tray": {"type": "boolean"},
                "launch_at_login": {"type": "boolean"},
            }),
            &[],
        ),
    },
    ToolDef {
        name: "set_tun_options",
        description: "修改 TUN 选项（栈类型/IPv6），变更后自动排队重启",
        input_schema: objSchema(
            json!({
                "tun_stack": {"type": "string", "enum": ["system", "gvisor", "mixed"]},
                "tun_ipv6_enabled": {"type": "boolean"},
            }),
            &[],
        ),
    },
    ]
}

/// `tools/list` 的响应体。
pub fn list() -> Value {
    Value::Array(
        tool_defs()
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "inputSchema": t.input_schema,
                })
            })
            .collect(),
    )
}

/// 解析 `tools/call` 的 params → (tool 名, 参数对象)。
pub fn parse_call(params: &Value) -> Result<(String, Value), String> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or("tools/call params.name missing")?
        .to_string();
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    Ok((name, args))
}

/// MCP tool result：文本 content（JSON 序列化）。
pub fn tool_result_text(payload: impl serde::Serialize, is_error: bool) -> Value {
    let text = serde_json::to_string(&payload).unwrap_or_else(|e| format!("{{\"serialize_error\":\"{e}\"}}"));
    json!({
        "content": [{"type": "text", "text": text}],
        "isError": is_error,
    })
}

/// 分发一个工具调用。同步阻塞——server.rs 已将其放入 spawn_blocking。
/// `Err(String)` 为业务失败（转 isError:true），`Ok(Value)` 为成功结果。
pub fn dispatch(app: &AppHandle, name: &str, args: Value) -> Result<Value, String> {
    let Some(state) = app.try_state::<AppState>() else {
        return Err("app state unavailable".into());
    };
    let state = state.inner();
    let resource_dir = app.path().resource_dir().ok();

    match name {
        // —— 只读 ——
        "get_status" => serde_json::to_value(state.proxy_status().map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string()),
        "get_settings" => state
            .with_store(|store| Ok(serde_json::to_value(store.settings.clone())?))
            .map_err(|e: AppError| e.to_string()),
        "list_subscriptions" => state
            .with_store(|store| Ok(serde_json::to_value(store.subscriptions.clone())?))
            .map_err(|e: AppError| e.to_string()),
        "list_nodes" => {
            let sub = args.get("subscription_id").and_then(|v| v.as_str());
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(100).min(1000) as usize;
            state
                .with_store(|store| {
                    let nodes: Vec<_> = store
                        .nodes
                        .iter()
                        .filter(|n| sub.map(|s| n.subscription_id == s).unwrap_or(true))
                        .take(limit)
                        .collect();
                    Ok(serde_json::to_value(nodes)?)
                })
                .map_err(|e: AppError| e.to_string())
        }
        "list_rule_sets" => state
            .with_store(|store| Ok(serde_json::to_value(store.rule_sets.clone())?))
            .map_err(|e: AppError| e.to_string()),
        "list_pools" => state
            .with_store(|store| Ok(serde_json::to_value(store.pools.clone())?))
            .map_err(|e: AppError| e.to_string()),
        "list_chains" => state
            .with_store(|store| Ok(serde_json::to_value(store.chains.clone())?))
            .map_err(|e: AppError| e.to_string()),
        "get_dns_settings" => state
            .with_store(|store| Ok(serde_json::to_value(store.dns.clone())?))
            .map_err(|e: AppError| e.to_string()),
        "get_pac_status" => state
            .pac_status()
            .map_err(|e| e.to_string())
            .and_then(|v| serde_json::to_value(v).map_err(|e| e.to_string())),
        "list_connections" => {
            let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(50).min(500) as usize;
            // live_connections 需要 &mut Runtime + &AppStore（tag 映射）。
            // 锁序规则：先 runtime 后 store —— 与命令层一致。
            let mut runtime = state.lock_runtime();
            let conns = state
                .with_store(|store| Ok(runtime.live_connections(store)))
                .map_err(|e: AppError| e.to_string())?;
            let conns: Vec<_> = conns.into_iter().take(limit).collect();
            serde_json::to_value(conns).map_err(|e| e.to_string())
        }
        "get_core_log_tail" => {
            let limit = args
                .get("lines")
                .and_then(|v| v.as_u64())
                .unwrap_or(50)
                .clamp(1, 1000) as usize;
            let runtime = state.lock_runtime();
            let tail = runtime.core.core_log_tail(limit);
            let value = match tail {
                Some((path, lines)) => json!({"path": path.display().to_string(), "lines": lines}),
                None => json!({"path": null, "lines": []}),
            };
            Ok(value)
        }

        // —— 出口切换 ——
        "switch_node" => {
            let node_id = require_str(&args, "node_id")?;
            let (_, restart_needed, _) =
                state.select_current_node_serialized(node_id, true, true).map_err(|e| e.to_string())?;
            if restart_needed {
                crate::rule_apply::request_restart(app.clone(), Vec::new());
            }
            serde_json::to_value(state.proxy_status().map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())
        }
        "select_pool" => {
            let pool_id = require_str(&args, "pool_id")?;
            let (applied, restart_needed) =
                state.select_pool_serialized(pool_id).map_err(|e| e.to_string())?;
            if restart_needed {
                crate::rule_apply::request_restart(app.clone(), Vec::new());
            }
            Ok(json!({"applied": applied, "restart_needed": restart_needed}))
        }
        "select_chain" => {
            let chain_id = require_str(&args, "chain_id")?;
            let (applied, restart_needed) =
                state.select_chain_serialized(chain_id).map_err(|e| e.to_string())?;
            if restart_needed {
                crate::rule_apply::request_restart(app.clone(), Vec::new());
            }
            Ok(json!({"applied": applied, "restart_needed": restart_needed}))
        }
        // smart_switch / add_subscription / refresh_subscription 是 async 工具，
        // 由 mod.rs 的 real_handler 分流到 dispatch_async，不进本同步分发。

        // —— 内核控制 ——
        "start_proxy" => {
            let status = state
                .start_proxy(resource_dir.as_deref(), false)
                .map_err(|e| e.to_string())?;
            serde_json::to_value(status).map_err(|e| e.to_string())
        }
        "stop_proxy" => {
            let status = state.stop_proxy().map_err(|e| e.to_string())?;
            serde_json::to_value(status).map_err(|e| e.to_string())
        }
        "restart_proxy" => {
            let status = state.restart_proxy(resource_dir.as_deref()).map_err(|e| e.to_string())?;
            serde_json::to_value(status).map_err(|e| e.to_string())
        }
        "set_capture_mode" => {
            let mode = require_str(&args, "mode")?;
            let status = state
                .set_capture_mode(mode, resource_dir.as_deref())
                .map_err(|e| e.to_string())?;
            serde_json::to_value(status).map_err(|e| e.to_string())
        }
        "set_outbound_mode" => {
            let mode = require_str(&args, "mode")?;
            let parsed = crate::domain::OutboundMode::parse(mode)
                .ok_or_else(|| format!("invalid outbound mode: {mode}"))?;
            let status = state
                .set_outbound_mode(parsed, resource_dir.as_deref())
                .map_err(|e| e.to_string())?;
            serde_json::to_value(status).map_err(|e| e.to_string())
        }
        "set_core_type" => {
            let kind = require_str(&args, "core")?;
            let parsed = crate::core::CoreKind::parse(kind);
            if parsed.as_str() != kind {
                return Err(format!("invalid core kind: {kind}"));
            }
            let running = state.is_core_running();
            let settings = state
                .with_store_mut(|store| {
                    store.settings.core_type = parsed.as_str().to_string();
                    store.settings.enforce_multi_core_scope();
                    Ok(store.settings.clone())
                })
                .map_err(|e: AppError| e.to_string())?;
            if running {
                crate::rule_apply::request_restart(app.clone(), Vec::new());
            }
            serde_json::to_value(settings).map_err(|e| e.to_string())
        }

        // —— 订阅（add/refresh 为 async 工具，走 dispatch_async）——
        "remove_subscription" => {
            let id = require_str(&args, "id")?;
            let before = crate::commands::subscription::enabled_flags(state);
            state
                .with_store_mut(|store| store.remove_subscription(id))
                .map_err(|e: AppError| e.to_string())?;
            crate::config::remove_custom_config(&state.app_data_dir, id);
            crate::config::remove_raw_subscription(&state.app_data_dir, id);
            crate::commands::subscription::queue_rebuild_if_enabled_set_changed(
                app, state, &before, "remove",
            );
            Ok(json!({"removed": id}))
        }

        // —— 规则集 ——
        "set_rule_set_enabled" => {
            let id = require_str(&args, "id")?;
            let enabled = args
                .get("enabled")
                .and_then(|v| v.as_bool())
                .ok_or("params.enabled must be boolean")?;
            let skip_restart = state
                .with_store_mut(|store| {
                    let boundary_before = crate::commands::rules::any_rule_set_enabled(store);
                    store.set_rule_set_enabled(id, enabled)?;
                    Ok(any_enabled_unchanged(
                        store,
                        id,
                        enabled,
                        boundary_before,
                    ))
                })
                .map_err(|e: AppError| e.to_string())?;
            if skip_restart {
                crate::rule_apply::emit_ready_without_restart(app, id, enabled);
            } else {
                crate::rule_apply::request_apply(app.clone(), id.to_string(), enabled);
            }
            Ok(json!({"id": id, "enabled": enabled}))
        }
        "set_rule_set_strategy" => {
            let id = require_str(&args, "id")?;
            let strategy = require_str(&args, "strategy")?;
            let parsed = parse_strategy(strategy)?;
            let set = state
                .with_store_mut(|store| {
                    let set = store
                        .rule_sets
                        .iter_mut()
                        .find(|s| s.id == id)
                        .ok_or_else(|| AppError::NotFound(id.to_string()))?;
                    if set.remote.is_some() && parsed == crate::domain::RuleSetStrategy::Smart {
                        return Err(AppError::Config(
                            "远程规则集没有单条规则，无法转为混合策略".into(),
                        ));
                    }
                    set.strategy = parsed;
                    Ok(set.clone())
                })
                .map_err(|e: AppError| e.to_string())?;
            crate::rule_apply::request_restart(app.clone(), Vec::new());
            serde_json::to_value(set).map_err(|e| e.to_string())
        }

        // —— PAC ——
        "update_pac_list" => {
            let action = require_str(&args, "action")?;
            let domains: Vec<String> = args
                .get("domains")
                .and_then(|v| v.as_array())
                .ok_or("params.domains must be an array")?
                .iter()
                .filter_map(|v| v.as_str().map(str::trim).map(str::to_string))
                .filter(|s| !s.is_empty())
                .collect();
            if domains.is_empty() {
                return Err("domains is empty".into());
            }
            let mut list = state.pac_list().map_err(|e| e.to_string())?;
            match action {
                "add" => {
                    for d in domains {
                        let d = d.trim_start_matches('.').to_ascii_lowercase();
                        if !list.domains.iter().any(|x| *x == d) {
                            list.domains.push(d);
                        }
                    }
                }
                "remove" => {
                    let lower: Vec<String> = domains
                        .iter()
                        .map(|d| d.trim_start_matches('.').to_ascii_lowercase())
                        .collect();
                    list.domains.retain(|x| !lower.contains(x));
                }
                other => return Err(format!("invalid action: {other}")),
            }
            let updated = state.set_pac_list(list).map_err(|e| e.to_string())?;
            serde_json::to_value(updated).map_err(|e| e.to_string())
        }

        // —— 设置 ——
        "update_proxy_settings" => update_proxy_settings(state, app, &args),
        "update_app_prefs" => update_app_prefs(state, app, &args),
        "set_tun_options" => set_tun_options(state, app, &args),

        other => Err(format!("unknown tool: {other}")),
    }
}

/// 需要 async 的工具（smart_switch / 订阅网络导入刷新）由 mod.rs 的
/// real_handler 直接 await —— dispatch 遇到这些 sentinel 时不会到达。
pub async fn dispatch_async(app: &AppHandle, name: &str, args: Value) -> Result<Value, String> {
    let Some(state) = app.try_state::<AppState>() else {
        return Err("app state unavailable".into());
    };
    let state = state.inner();
    match name {
        "smart_switch" => {
            let result = crate::smart_switch::select_best_now(state).await?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
        "add_subscription" => {
            let url = require_str(&args, "url")?.to_string();
            let name_arg = args.get("name").and_then(|v| v.as_str()).map(str::to_string);
            let fresh_id = crate::commands::subscription::fresh_subscription_id(
                state,
                crate::domain::SubscriptionSource::Url {
                    url: url.trim().to_string(),
                },
            )
            .map_err(|e| e.to_string())?;
            let mixed_port = state
                .with_store(|s| Ok(s.settings.mixed_port))
                .map_err(|e: AppError| e.to_string())?;
            let outcome =
                crate::services::import::import_from_url_with_id(name_arg, url, Some(fresh_id), false, Some(mixed_port), None)
                    .await
                    .map_err(|e| e.to_string())?;
            let result = crate::commands::subscription::persist_import(app, state, outcome)?;
            serde_json::to_value(result).map_err(|e| e.to_string())
        }
        "refresh_subscription" => {
            let id = args
                .get("id")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            match id {
                Some(id) => {
                    let result =
                        crate::commands::subscription::refresh_subscription_inner(app, state, id, None).await?;
                    serde_json::to_value(result).map_err(|e| e.to_string())
                }
                None => {
                    // 刷新全部：逐个 await（订阅刷新内部自带并发合流）。
                    let ids: Vec<String> = state
                        .with_store(|store| {
                            Ok(store
                                .subscriptions
                                .iter()
                                .filter(|s| s.enabled)
                                .map(|s| s.id.clone())
                                .collect())
                        })
                        .map_err(|e: AppError| e.to_string())?;
                    let mut results = Vec::new();
                    for id in ids {
                        match crate::commands::subscription::refresh_subscription_inner(
                            app, state, id.clone(), None,
                        )
                        .await
                        {
                            Ok(r) => results.push(serde_json::to_value(r).unwrap_or_default()),
                            Err(e) => results.push(json!({"id": id, "error": e})),
                        }
                    }
                    Ok(json!(results))
                }
            }
        }
        _ => Err(format!("unknown async tool: {name}")),
    }
}

fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("params.{key} must be a non-empty string"))
}

fn parse_strategy(s: &str) -> Result<crate::domain::RuleSetStrategy, String> {
    use crate::domain::RuleSetStrategy::*;
    match s {
        "proxy" => Ok(Proxy),
        "direct" => Ok(Direct),
        "block" => Ok(Block),
        "smart" => Ok(Smart),
        other => Err(format!("invalid strategy: {other}")),
    }
}

/// 复刻 commands/rules.rs::set_rule_set_enabled 的 skip_restart 判断：
/// 空集开关不跨界 → 不重启。
fn any_enabled_unchanged(
    store: &crate::storage::AppStore,
    _id: &str,
    _enabled: bool,
    boundary_before: bool,
) -> bool {
    crate::commands::rules::any_rule_set_enabled(store) == boundary_before
        && store
            .get_rule_set(_id)
            .is_some_and(crate::config::rule_set_is_empty_for_config)
}

/// update_proxy_settings：端口/局域网/QUIC/兜底路由（副作用对齐 update_settings）。
fn update_proxy_settings(state: &AppState, app: &AppHandle, args: &Value) -> Result<Value, String> {
    let mixed_port = args.get("mixed_port").and_then(|v| v.as_u64()).map(|v| v as u16);
    let api_port = args.get("api_port").and_then(|v| v.as_u64()).map(|v| v as u16);
    let allow_lan = args.get("allow_lan").and_then(|v| v.as_bool());
    let bypass_lan = args.get("bypass_lan").and_then(|v| v.as_bool());
    let block_quic = args.get("block_quic").and_then(|v| v.as_bool());
    let route_final = args.get("route_final").and_then(|v| v.as_str()).map(str::to_string);

    let mut route_final_changed = false;
    let mut bypass_lan_changed = false;
    let settings = state
        .with_store_mut(|store| {
            if let Some(p) = mixed_port {
                if p == 0 {
                    return Err(AppError::Config("端口无效".into()));
                }
                store.settings.mixed_port = p;
            }
            if let Some(p) = api_port {
                if p == 0 {
                    return Err(AppError::Config("端口无效".into()));
                }
                store.settings.api_port = p;
            }
            if let Some(v) = allow_lan {
                store.settings.allow_lan = v;
            }
            if let Some(v) = bypass_lan {
                if store.settings.bypass_lan != v {
                    bypass_lan_changed = true;
                    store.settings.bypass_lan = v;
                }
            }
            if let Some(v) = block_quic {
                store.settings.block_quic = v;
            }
            if let Some(rf) = route_final {
                let rf = rf.trim().to_ascii_lowercase();
                if matches!(rf.as_str(), "proxy" | "direct" | "block")
                    && store.settings.route_final != rf
                {
                    route_final_changed = true;
                    store.settings.route_final = rf;
                }
            }
            Ok(store.settings.clone())
        })
        .map_err(|e: AppError| e.to_string())?;

    if route_final_changed || bypass_lan_changed {
        crate::rule_apply::request_restart(app.clone(), Vec::new());
    }
    serde_json::to_value(settings).map_err(|e| e.to_string())
}

/// update_app_prefs：语言/主题/托盘/开机自启（副作用对齐 update_settings）。
fn update_app_prefs(state: &AppState, app: &AppHandle, args: &Value) -> Result<Value, String> {
    let locale = args.get("locale").and_then(|v| v.as_str()).map(str::to_string);
    let theme = args.get("theme").and_then(|v| v.as_str()).map(str::to_string);
    let silent_start = args.get("silent_start").and_then(|v| v.as_bool());
    let auto_start_proxy = args.get("auto_start_proxy").and_then(|v| v.as_bool());
    let close_to_tray = args.get("close_to_tray").and_then(|v| v.as_bool());
    let launch_at_login = args.get("launch_at_login").and_then(|v| v.as_bool());

    let mut launch_changed: Option<bool> = None;
    let mut theme_changed = false;
    let settings = state
        .with_store_mut(|store| {
            if let Some(loc) = &locale {
                let loc = loc.trim().to_ascii_lowercase();
                if matches!(loc.as_str(), "zh" | "en") {
                    store.settings.locale = loc;
                }
            }
            if let Some(th) = &theme {
                let th = th.trim().to_ascii_lowercase();
                if matches!(th.as_str(), "aerospace" | "day") {
                    theme_changed = store.settings.theme != th;
                    store.settings.theme = th;
                }
            }
            if let Some(v) = silent_start {
                store.settings.silent_start = v;
            }
            if let Some(v) = auto_start_proxy {
                store.settings.auto_start_proxy = v;
            }
            if let Some(v) = close_to_tray {
                store.settings.close_to_tray = v;
            }
            if let Some(v) = launch_at_login {
                if store.settings.launch_at_login != v {
                    launch_changed = Some(v);
                }
                store.settings.launch_at_login = v;
            }
            Ok(store.settings.clone())
        })
        .map_err(|e: AppError| e.to_string())?;

    if let Some(enabled) = launch_changed {
        crate::autostart::set_launch_at_login(enabled).map_err(|e| e.to_string())?;
    }
    crate::tray::refresh_icon(app);
    if theme_changed {
        crate::window_ctrl::apply_window_theme(app);
    }
    serde_json::to_value(settings).map_err(|e| e.to_string())
}

/// set_tun_options：TUN 栈/IPv6（副作用对齐 update_settings）。
fn set_tun_options(state: &AppState, app: &AppHandle, args: &Value) -> Result<Value, String> {
    let tun_stack = args.get("tun_stack").and_then(|v| v.as_str()).map(str::to_string);
    let tun_ipv6 = args.get("tun_ipv6_enabled").and_then(|v| v.as_bool());
    let mut changed = false;
    let settings = state
        .with_store_mut(|store| {
            if let Some(s) = &tun_stack {
                let s = s.trim().to_ascii_lowercase();
                if matches!(s.as_str(), "system" | "gvisor" | "mixed")
                    && store.settings.tun_stack != s
                {
                    changed = true;
                    store.settings.tun_stack = s;
                }
            }
            if let Some(v) = tun_ipv6 {
                if store.settings.tun_ipv6_enabled != v {
                    changed = true;
                    store.settings.tun_ipv6_enabled = v;
                }
            }
            Ok(store.settings.clone())
        })
        .map_err(|e: AppError| e.to_string())?;
    if changed {
        crate::rule_apply::request_restart(app.clone(), Vec::new());
    }
    serde_json::to_value(settings).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_unique_and_snake_case() {
        let mut seen = std::collections::HashSet::new();
        for t in tool_defs() {
            assert!(!t.name.is_empty(), "empty name");
            assert!(
                t.name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit()),
                "non snake_case: {}",
                t.name
            );
            assert!(seen.insert(t.name), "duplicate tool: {}", t.name);
        }
        assert_eq!(tool_defs().len(), 30);
    }

    #[test]
    fn schemas_are_valid_shapes() {
        for t in tool_defs() {
            assert_eq!(t.input_schema["type"], "object", "{}: schema.type", t.name);
            assert!(t.input_schema["properties"].is_object());
            if let Some(req) = t.input_schema["required"].as_array() {
                for r in req {
                    let key = r.as_str().expect("required entry is string");
                    assert!(
                        t.input_schema["properties"].get(key).is_some(),
                        "{}: required '{key}' not in properties",
                        t.name
                    );
                }
            }
        }
    }

    #[test]
    fn parse_call_extracts_name_and_arguments() {
        let (name, args) =
            parse_call(&json!({"name": "switch_node", "arguments": {"node_id": "abc"}})).unwrap();
        assert_eq!(name, "switch_node");
        assert_eq!(args["node_id"], "abc");
        let (name, args) = parse_call(&json!({"name": "get_status"})).unwrap();
        assert_eq!(name, "get_status");
        assert!(args.is_null() || args.is_object());
    }

    #[test]
    fn tool_result_text_shapes_content() {
        let v = tool_result_text(json!({"ok": true}), false);
        assert_eq!(v["content"][0]["type"], "text");
        assert_eq!(v["isError"], false);
        let text = v["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("\"ok\""));
        let v = tool_result_text("boom", true);
        assert_eq!(v["isError"], true);
    }
}
