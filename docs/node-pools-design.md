# 节点池（Node Pools）设计文档

> 目标：把「节点池」从「设置 → 链」迁移到「节点」板块，统一为「节点池」概念，
> 内置默认节点池，并让「节点选择」作为主路由组，取代现有全局 `auto_select` 三模式。

## 1. 背景与现状

### 1.1 现有「节点池 / 代理链」的位置

- `NodePool`（节点池）和 `ProxyChain`（多跳链）的数据模型在 `src-tauri/src/domain/chain.rs`。
- 前端管理页是 `ChainPage`（`src/pages/ChainPage.tsx`），当前嵌入在 **设置（SettingsPage）的 `chain` tab**。
- 规则集（`RuleSet`）在 **设置（SettingsPage）的 `rules` tab**（`RulesPage`）。
- 节点板块是顶级导航 `nodes`（`NodesPage`），当前只有「节点列表」（分组：平铺 / 订阅 / 协议 / 国家）。

### 1.2 现有「自动选择」三模式

`AutoSelectMode`（`domain/settings.rs`）= `Off` | `Smart` | `Kernel`，决定**主出站 `proxy`** 的行为：

| 模式 | 内核层生成 | app 层行为 |
|---|---|---|
| `Off` | `selector`（手动，`current_node_id` 作 default） | 无 |
| `Kernel` | `urltest`（周期测速选最快） | 无 |
| `Smart` | `selector` | `smart_switch.rs` 被动监控 + 降级切换 + 逐出失败节点 |

### 1.3 上轮已做的基础（可直接复用）

- `NodePool` 已新增 `strategy: PoolStrategy`（`Select`/`UrlTest`）、`probe_url`、`interval`、`tolerance`。
- `config/builder.rs` 的 `build_pool_selectors` 已按 `strategy` 生成 `urltest`/`selector`。
- 已支持从 clash 订阅导入 `proxy-groups`（解析 → 转 `NodePool`）。

## 2. 核心概念统一

现有 `auto_select` 三模式与「节点池策略」是同一概念的不同切法，统一方案：

**主路由组（内置「节点选择」）的策略 = 内核级策略枚举**：
`手动选择`（select）/ `自动测速`（url-test）/ `负载均衡`（load-balance）/ `故障转移`（fallback）。

**映射**：
- `off` → 主路由组策略 = 手动选择
- `kernel` → 主路由组策略 = 自动测速
- `smart` → 主路由组策略 = 手动选择 + 独立 `smart` 开关开启

**`smart` 的定位**：app 级增强，独立于内核策略。仅在「手动选择」策略下有意义（给手动组加后台自动切换）。保留 `smart_switch.rs` 不动，仅把触发开关从 `auto_select == Smart` 改为「主路由组策略 == 手动 && smart 开关」。

## 3. UI 方案

### 3.1 节点板块（NodesPage）加标签

```
节点板块（nodes）
├── 节点        ← 现有节点列表（平铺/订阅/协议/国家分组）
└── 节点池      ← 新增：节点池管理（从「设置→链」搬来）
```

### 3.2 内置默认节点池（首次启动 seed）

| 节点池 | 策略 | 底层 |
|---|---|---|
| 节点选择 | 手动（默认） | 主路由组，等价现有 `proxy` selector |
| 自动选择 | 自动测速 | urltest |
| 负载均衡 | 负载均衡 | mihomo 原生 / sing-box 近似 urltest |
| 故障转移 | 故障转移 | mihomo 原生 / sing-box 近似 urltest |
| 手动选择 | 手动 | 用户手动挑，普通 selector |
| 香港节点 | 手动 | `PoolMode::Keyword { include: ["香港","HK"] }` |
| 新加坡节点 | 手动 | `PoolMode::Keyword { include: ["新加坡","SG"] }` |
| 美国节点 | 手动 | `PoolMode::Keyword { include: ["美国","US"] }` |
| 其他节点 | 手动 | `PoolMode::Keyword`（排除上述关键词） |

地区组用**关键词池**（`PoolMode::Keyword`），订阅刷新后成员自动更新。

### 3.3 节点池可指定为路由

规则 / 规则集的路由目标新增 `pool` 类型（`RuleTarget::Pool`），可把「某规则 → 路由到「香港节点」组」。

## 4. 数据模型改动

### 4.1 `NodePool`（`domain/chain.rs`）

已有 `strategy`/`probe_url`/`interval`/`tolerance`。需补充：

- `builtin: bool`（标记内置默认组，删除时保护 / Reset 恢复）。
- `PoolStrategy` 扩展 `LoadBalance`、`Fallback`（`from_clash_kind` 已把 fallback/load-balance 归到 UrlTest，需拆分）。

### 4.2 `RuleTarget`（`domain/rule.rs`）

新增 `Pool { pool_id: String }` 变体，规则/规则集路由到指定节点池。

### 4.3 `AppSettings`

`auto_select: AutoSelectMode` 迁移为「主路由组策略」+「smart 开关」两个字段（见 6.2 迁移）。

## 5. 后端改动

1. `storage/store.rs`：seed 内置默认节点池；`create_pool` 支持 `strategy`/`builtin`。
2. `config/builder.rs`：主组生成改为「读主路由组的策略」；`build_pool_selectors` 支持 LoadBalance/Fallback（sing-box 落到 urltest，mihomo 走原生）。
3. `config/mihomo.rs`：mihomo 内核按 PoolStrategy 生成 url-test/fallback/load-balance/select。
4. `domain/rule.rs` + builder：`RuleTarget::Pool` 路由到节点池 outbound tag。
5. `smart_switch.rs`：触发条件从 `auto_select == Smart` 改为「主路由组策略 == 手动 && smart 开关」。

## 6. 迁移与兼容

### 6.1 前端

- `ChainPage` 拆出节点池部分 → `NodesPage` 的「节点池」标签。
- 代理链（`ProxyChain`）暂留「设置 → 链」，后续再议。

### 6.2 存量设置迁移

`auto_select` 旧值 → 新模型：
- `Off` → 主路由组策略 = 手动，smart = off
- `Kernel` → 主路由组策略 = 自动测速
- `Smart` → 主路由组策略 = 手动，smart = on

## 7. 分期实施

| 阶段 | 内容 |
|---|---|
| P0 | `NodePool` 加 `builtin` + `PoolStrategy` 扩展 LoadBalance/Fallback；seed 内置节点池 |
| P1 | NodesPage 加「节点池」标签，节点池管理搬入；内置组展示 |
| P2 | 主路由组 = 「节点选择」，策略切手动/自动/负载均衡/故障转移；`auto_select` 迁移 |
| P3 | `RuleTarget::Pool` 路由到节点池；mihomo 完整 fallback/load-balance |

## 8. 权衡

- **fallback/load-balance 在 sing-box 下近似 urltest**：完整语义仅 mihomo 原生支持，方向 A 先近似，切 mihomo 内核时走完整。
- **代理链归属**：多跳链与节点池是两回事，本轮暂留设置，避免混淆。
- **命名统一**：现有「节点池」统一改叫「节点池」，与用户心智一致。
