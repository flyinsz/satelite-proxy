# Satelite 内置 MCP Server — 设计文档

> 功能：应用内嵌一个 MCP (Model Context Protocol) server，让本机 AI agent（WorkBuddy / Claude Desktop 等）查询与修改 Satelite 配置。
> 状态：已实现并部署（commit `8a0914d`，v1.0.4）。

## 1. 动机

用户希望在 agent（如 WorkBuddy）里直接说「切到家宽链」「把某规则集关掉」——过去这些操作只能在 UI 里点。MCP 是 agent 侧的标准工具协议，应用内嵌 server 后无需任何中间脚本。

## 2. 端点契约

```
POST http://127.0.0.1:{mcp_port}/mcp     （默认 2086，仅绑定 127.0.0.1）
Content-Type: application/json
Body: JSON-RPC 2.0 单请求（不支持 batch）
```

| method | 行为 |
|---|---|
| `initialize` | 回 `protocolVersion`（支持集 {2025-03-26, 2025-06-18} 内回显，否则取 2025-06-18）+ `capabilities.tools` + `serverInfo{name:"satelite-proxy", version}` |
| `notifications/*`（无 id） | HTTP 202，空体 |
| `tools/list` | `result.tools` = 30 个描述符（name/description/inputSchema，JSON Schema 2020-12） |
| `tools/call` | 执行，回 `result.content[0].text`（JSON 字符串）+ `isError`；**业务失败用 isError:true**（MCP 惯例，agent 读中文消息重试），协议错误才用 JSON-RPC error |
| `ping` | `{}` |
| 其他 | `-32601`；GET /mcp → 405；未知路径 → 404 |

无状态：不实现 `Mcp-Session-Id`；每连接单请求 `Connection: close`。不实现 SSE（无服务器推送需求；MCP 规范允许 `application/json` 非流式响应）。

## 3. 工具清单（30 个）

**只读 11**：`get_status` `get_settings` `list_subscriptions` `list_nodes(subscription_id?,limit?)` `list_rule_sets` `list_pools` `list_chains` `get_dns_settings` `get_pac_status` `list_connections(limit?)` `get_core_log_tail(lines?)`

**出口切换 4**：`switch_node(node_id)` `select_pool(pool_id)` `select_chain(chain_id)` `smart_switch()` — 走 state 层 `select_*_serialized`（热切+持久化），`restart_needed → rule_apply::request_restart`

**内核控制 6**：`start_proxy` `stop_proxy` `restart_proxy` `set_capture_mode(off|system|tun)` `set_outbound_mode(rule|global|direct)` `set_core_type(singbox|xray|mihomo)`

**订阅 3**：`add_subscription(url,name?)` `refresh_subscription(id?)` `remove_subscription(id)`

**规则集 2**：`set_rule_set_enabled(id,enabled)` `set_rule_set_strategy(id,strategy)`

**PAC 1**：`update_pac_list(action: add|remove, domains[])` — 自定义域名定向增删

**设置 3**（update_settings 30+ 参数按领域拆分）：`update_proxy_settings(mixed_port?/api_port?/allow_lan?/bypass_lan?/block_quic?/route_final?)` `update_app_prefs(locale?/theme?/silent_start?/auto_start_proxy?/close_to_tray?/launch_at_login?)` `set_tun_options(tun_stack?/tun_ipv6_enabled?)`

> `mcp_enabled` / `mcp_port` 刻意**不进**工具面——防止 agent 关掉自己的连接。只能由设置页 UI 修改。

## 4. 代码结构

```
src-tauri/src/mcp/
├── mod.rs     spawn(app)（setup 挂载）+ real_handler 分发 + initialize
├── rpc.rs     JSON-RPC 2.0 类型/编解码/错误码（-32700/-32600/-32601/-32602/-32603）
├── server.rs  McpServer{start/stop/url/port}：tokio TcpListener + 手写 HTTP/1.1；
│              RpcHandler 站点解耦（传输层可独立单测）；
│              header 16KB / body 2MB 上限
└── tools.rs   ToolDef 注册表（OnceLock 惰性构建）+ dispatch（同步）/dispatch_async（async 工具）
```

**复用而非重写**：`State<'_, AppState>` 只是 `&AppState` 包装，MCP handler 拿 `AppHandle` 后 `try_state::<AppState>()` 即可调用全部 state 层方法。为让 MCP handler 复用命令层业务闭包，以下私有函数提升为 `pub(crate)`：
- `commands/subscription.rs`：`persist_import` / `refresh_subscription_inner` / `enabled_flags` / `queue_rebuild_if_enabled_set_changed` / `fresh_subscription_id`（后者签名 `&State`→`&AppState`）
- `commands/rules.rs`：`any_rule_set_enabled`
- `commands/mod.rs`：`mod rules` → `pub(crate) mod rules`

## 5. 生命周期与设置

| 项 | 值 |
|---|---|
| 设置字段 | `mcp_enabled`（serde default **false**）、`mcp_port`（default 2086） |
| 启动 | lib.rs setup → `mcp::spawn(app)`：独立线程读设置，enabled 才 start |
| 设置变更 | `update_settings` 检测变更 → `state.restart_mcp_from_settings(app)`（stop 旧 + start 新；失败仅 app_log::warn 不回滚） |
| 端口校验 | ≠0、≠mixed_port、≠api_port；sidecar reserved 集合纳入 `mcp_port` |
| UI | 设置 → 端口：「MCP 服务」开关 + 端口输入 + 运行状态（5s 轮询 `get_mcp_status`） |
| 状态查询 | `get_mcp_status` Tauri 命令 → `{enabled, running, port, url}` |

## 6. 并发与安全

- **并发写 store**：全部走 `with_store_mut`（内部 `store_persistence` 串行化 + 原子落盘），与 Tauri command 同一入口；切换类复用 `*_serialized`（内部先 `begin_core_transition` 互斥、锁序先 runtime 后 store）
- **长任务**：`tools::dispatch` 在 `spawn_blocking` 执行，不占 tokio worker；内核控制类同步等到完成，transition 期间并发调用得 busy 错误
- **暴露面**：仅 127.0.0.1 + 默认关闭（用户知情选择无鉴权）。未来可加 `Authorization: Bearer` 校验（MCP 规范原生支持，留扩展点）
- **防呆**：mcp 自身设置不进工具面；`remove_subscription` 保留内置规则引用 guard；`set_rule_set_strategy` 拒绝远程集转 smart

## 7. 测试记录

- 单测 19 个：rpc 编解码 8；server 传输层 7（reqwest 打真实端口：initialize 往返 / notification 202 / GET 405 / batch 拒绝 / bind 冲突 / url 形状）；tools 4（名字唯一 snake_case、schema 结构、parse_call、tool_result_text）
- 全量 `cargo test`：617 passed / 4 failed（既有环境测试：zip matcher、process rss ×2、runtime ports——与本功能无关）
- 端到端（部署后 curl 实测 2086）：initialize ✅ / tools/list=30 ✅ / tools/call get_status（core_state:running, system_proxy:true）✅ / list_chains（家宽链两条）✅ / 未知工具 isError ✅ / notification→202 ✅ / GET→405 ✅
- 实现中修复：`tools/list` 最初直接返回数组，不符合 MCP 规范的 `result.tools` 包裹——curl 实测暴露后修正

## 8. 接入方式

WorkBuddy（`~/.workbuddy/mcp.json`）：
```json
{ "mcpServers": { "satelite": { "url": "http://127.0.0.1:2086/mcp" } } }
```
Claude Desktop 同理（Streamable HTTP）。前提：应用设置 → 端口 → 打开「MCP 服务」。客户端添加后需手动 Trust/授权。
