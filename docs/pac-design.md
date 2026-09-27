# PAC 反向代理模式 — 二开设计文档

> 目标：在 Satelite 的「流量接管（Capture）」中，为「系统代理（system）」新增「PAC 自动代理」子模式。
> 核心价值：国内流量直连（不经过代理内核），被墙域名走代理。内核挂了国内流量零影响。

## P0 摸底结论（已确认）

| 关注点 | 现状 | 位置 |
|---|---|---|
| CaptureMode 枚举 | `off / system / tun`（serde snake_case） | `src-tauri/src/domain/settings.rs:20` |
| 前端 capture_mode 类型 | `"off" \| "system" \| "tun"` | `src/types.ts:526` |
| 系统代理实现 | `networksetup -setwebproxy/-setsecurewebproxy` 固定端口 | `src-tauri/src/proxy/macos.rs` |
| 系统代理 trait | `enable / disable / detect_owned` | `src-tauri/src/proxy/mod.rs:28` |
| 运行时切换 | `runtime.set_system_proxy(store, enabled)` → `system_proxy.enable("127.0.0.1", port)` | `src-tauri/src/runtime.rs:2206` |
| capture 协调 | `state.set_capture_mode` 里 `want_sys = mode == System` | `src-tauri/src/state.rs:1639` |
| IPC 命令 | `set_capture_mode` 等 | `src-tauri/src/commands/proxy.rs:82` |
| 前端切换 hook | `CaptureMode = "off" \| "system" \| "tun"` | `src/hooks/useCaptureModeSwitch.ts:6` |
| 前端 api | `setCaptureMode(mode)` | `src/api.ts:760` |
| 依赖 | reqwest / ureq / base64 / tokio(net) / serde_json | `src-tauri/Cargo.toml` |

**结论**：PAC 归属「流量接管」层，是 `system` 的子类型。**不改 `CaptureMode` 三值枚举**（避免破坏 serde 存量数据、tray/status 语义），新增正交字段 `system_proxy_kind` 区分 `manual`（现状）与 `pac`（新增）。

## 接口契约

### 1. Rust：设置扩展（`domain/settings.rs`）

```rust
/// system 模式下系统代理的实现方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SystemProxyKind {
    /// 手动代理：固定指向 mixed 端口（现状）。
    #[default]
    Manual,
    /// PAC 自动代理：指向本地 PAC 脚本 URL。
    Pac,
}
impl SystemProxyKind {
    pub fn as_str(self) -> &'static str;   // "manual" / "pac"
    pub fn parse(s: &str) -> Option<Self>;
}
```

`AppSettings` 新增字段（均带 `#[serde(default)]` 或 default fn，保证旧 store 平滑加载）：

```rust
#[serde(default)]
pub system_proxy_kind: SystemProxyKind,        // 默认 Manual
#[serde(default = "default_pac_port")]
pub pac_port: u16,                              // 默认 2085
#[serde(default)]
pub pac_list: crate::pac::PacList,              // PAC 名单（见下）
```

`Default for AppSettings` 补上这三个字段。

### 2. Rust：PAC 模块（`src-tauri/src/pac/`，纯新增，不碰现有文件）

`src-tauri/src/pac/mod.rs`：

```rust
pub mod generator;
pub mod gfwlist;
pub mod server;

/// PAC 名单。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PacList {
    /// 域名（后缀匹配）：google.com / .google.com 均按后缀处理。
    pub domains: Vec<String>,
    /// IP/CIDR：1.2.3.0/24、1.2.3.4。
    pub ip_cidrs: Vec<String>,
    /// 顶级后缀：.hk / .tw。
    pub suffixes: Vec<String>,
    /// 地区预置（如 hk / tw）——解析为预置 IP 段参与匹配。
    pub regions: Vec<String>,
}
```

- `generator.rs`：
  - `pub fn generate_pac(list: &PacList, proxy_host: &str, proxy_port: u16) -> String` — 返回完整 PAC JS。
  - `pub fn region_cidrs(region: &str) -> &'static [&'static str]` — 地区 → 预置 CIDR（内置 hk/tw 少量段）。
- `gfwlist.rs`：
  - `pub fn parse_gfwlist_text(text: &str) -> Vec<String>` — 解析 gfwlist 文本 → 域名列表（纯函数，可单测；处理 `||domain`、`|url`、`.domain`、`@@` 白名单）。
  - `pub async fn fetch_gfwlist(url: &str) -> Result<Vec<String>, String>` — 拉取 + base64 解码 + parse（用 reqwest，可经代理）。
- `server.rs`：
  - `pub struct PacServer { .. }`
  - `impl PacServer { pub async fn start(port: u16, content: String) -> Result<Self, String>; pub fn url(&self) -> String; pub async fn stop(self); }`
  - 用 `tokio::net::TcpListener` 手写最小 HTTP 响应（`GET /proxy.pac` → `200 application/x-ns-proxy-autoconfig` + content）。绑定 `127.0.0.1`。

PAC JS 模板（`FindProxyForURL`）：

```js
function FindProxyForURL(url, host) {
  var PROXY = "PROXY 127.0.0.1:2080; DIRECT";
  // 域名后缀
  if (dnsDomainIs(host, ".google.com")) return PROXY;
  // IP/CIDR
  if (isInNet(host, "1.2.3.0", "255.255.255.0")) return PROXY;
  // 顶级后缀
  if (dnsDomainIs(host, ".hk")) return PROXY;
  return "DIRECT";
}
```

### 3. Rust：系统代理 trait 扩展（`proxy/mod.rs` + 各平台）

```rust
pub trait SystemProxy: Send + Sync {
    fn enable(&self, host: &str, port: u16) -> AppResult<SystemProxySnapshot>;
    fn disable(&self, snapshot: Option<&SystemProxySnapshot>) -> AppResult<()>;
    fn detect_owned(&self, host: &str, port: u16) -> AppResult<Option<SystemProxySnapshot>>;
    /// PAC 自动代理：url 为本地 PAC 脚本 URL。
    fn enable_pac(&self, url: &str) -> AppResult<SystemProxySnapshot>;
    fn disable_pac(&self, snapshot: Option<&SystemProxySnapshot>) -> AppResult<()>;
}
```

- `macos.rs`：
  - `enable_pac`：`networksetup -setautoproxyurl <svc> <url>` + `-setautoproxystate <svc> on`（复用 `services()` 遍历）。
  - `disable_pac`：`networksetup -setautoproxystate <svc> off`。
- `stub.rs`：返回 Ok（空 snapshot）。
- `windows.rs`：最低限度实现（写注册表 `AutoConfigURL`）或返回明确的「未实现」错误；不 panic。

### 4. Rust：命令层（新 `commands/pac.rs` + `lib.rs` 注册）

```rust
#[tauri::command] pub async fn set_system_proxy_kind(app, kind: String) -> Result<ProxyStatus, String>;
#[tauri::command] pub async fn get_pac_list(state) -> Result<PacList, String>;
#[tauri::command] pub async fn update_pac_list(state, list: PacList) -> Result<PacList, String>;
#[tauri::command] pub async fn refresh_gfwlist(app, state) -> Result<PacList, String>;
#[tauri::command] pub async fn get_pac_status(state) -> Result<PacStatus, String>;
```

`PacStatus`：

```rust
#[derive(Serialize)]
pub struct PacStatus {
    pub kind: String,            // "manual" | "pac"
    pub enabled: bool,           // PAC 系统代理当前是否开启
    pub url: String,             // http://127.0.0.1:<pac_port>/proxy.pac
    pub domain_count: usize,     // 名单域名数
    pub last_update: Option<u64>, // gfwlist 上次刷新时间戳
}
```

`AppState` 新增方法（供命令调用，内部走现有锁）：
- `set_system_proxy_kind(kind: SystemProxyKind, resource_dir)` — 切换 kind；若 system 代理开启且切到 pac，则启动 PacServer + `enable_pac`；切回 manual 则停 PAC + `enable`。
- `pac_list()` / `set_pac_list(list)` / `refresh_gfwlist()` / `pac_status()`。

### 5. Rust：runtime/state 集成

- `runtime.rs`：`Runtime` 增加 PAC 服务持有字段（`pac_server: Option<PacServer>`、`pac_content: Option<String>`）。
  - `set_system_proxy` 分支：`enabled==true` 时按 `store.settings.system_proxy_kind` 选择 `enable`（manual）或 `enable_pac`（pac，需先启动 PacServer）。
  - `stop_proxy` / `shutdown` 时停 PAC 服务 + `disable_pac`。
  - `status()` 填 `ProxyStatus.system_proxy_kind`。
- `state.rs`：`set_capture_mode` 的 system 分支沿用现有 `want_sys` 逻辑，内部 `runtime.set_system_proxy` 已按 kind 分发。PAC 服务在 system 关时停止。
- `runtime.rs` `ProxyStatus` 新增字段：`pub system_proxy_kind: Option<String>`。

### 6. TS：类型 + api + UI

`src/types.ts`：

```ts
export type SystemProxyKind = "manual" | "pac";
export interface PacList { domains: string[]; ip_cidrs: string[]; suffixes: string[]; regions: string[]; }
export interface PacStatus { kind: SystemProxyKind; enabled: boolean; url: string; domain_count: number; last_update: number | null; }
// AppSettings 增： system_proxy_kind?: SystemProxyKind; pac_port?: number;
// ProxyStatus 增： system_proxy_kind?: string;
```

`src/api.ts` 新增：

```ts
export function setSystemProxyKind(kind: SystemProxyKind) { ... invoke("set_system_proxy_kind", { kind }) ... }
export function getPacList() { ... invoke("get_pac_list") ... }
export function updatePacList(list: PacList) { ... invoke("update_pac_list", { list }) ... }
export function refreshGfwlist() { ... invoke("refresh_gfwlist") ... }
export function getPacStatus() { ... invoke("get_pac_status") ... }
```

UI：
- 流量接管「系统代理」下加 manual/pac 子选择（DashboardPage / 现有 Capture 切换 UI）。
- 新增 PAC 名单设置入口（可放 SettingsPage 或独立页）：域名 / IP/CIDR / 后缀 / 地区 列表编辑 + gfwlist 刷新 + 状态展示。
- i18n：`messages.ts` 补充文案。

## 分阶段映射

- **P1** → §2（pac 模块）
- **P2** → §1 + §3（设置 + proxy trait）
- **P3** → §4 + §5（命令 + runtime/state）
- **P4** → §6（前端）

## 验证方式

- 后端：`cd src-tauri && cargo test -p satelite-proxy --lib pac::`（纯函数单测）+ `cargo check`。
- 前端：`tsc --noEmit`（package.json `build` 脚本前置 `tsc`）。
