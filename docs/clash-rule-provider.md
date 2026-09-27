# 从 Clash 订阅拉取规则集（rule-providers → sing-box source）

> 功能设计文档。记录订阅解析记录 `rule-providers`、clash 规则集转换、下载接入与前端一键导入的完整链路。

## 1. 背景与问题

### 1.1 订阅解析只提取 `proxies`

在实现本功能之前，`parse_clash_yaml`（`src-tauri/src/subscription/clash.rs`）只从 Clash YAML 正文中提取 `proxies:` 列表并解析为节点，`rule-providers:` 部分被完全忽略、丢弃。

很多机场订阅（尤其带分流规则的 Clash 配置）除了节点外还携带 `rule-providers`——一组以远程 URL 形式引用的规则集。丢弃它们意味着：

- 用户无法把订阅里现成的广告拦截 / 国内直连 / OpenAI 等规则集导入为 sing-box 规则集；
- 用户必须手动去订阅里复制 URL，再到「新建规则集」对话框里粘贴，体验割裂且容易出错。

### 1.2 clash 规则集格式与 sing-box 不兼容

`rule-providers` 指向的远端文件是 **clash 规则集格式**，与 app 现有远程规则集基础设施支持的两种 sing-box 格式互不兼容：

| 格式 | 特征 | 本 app 是否支持 |
| --- | --- | --- |
| sing-box `source` JSON | `{"version":3,"rules":[…]}` | 是（`validate_source`） |
| sing-box 二进制 `.srs` | 以 `SRS` 魔数开头 | 是（`srs::parse`） |
| clash 规则集 | 纯文本行 `TYPE,payload,target` 或 YAML wrapper（`payload:` 列表） | 否（转换前） |

clash 规则集有三种 `behavior`（由 provider 声明，但仅描述主导类型，每行自带类型 token）：

- `domain` → 文本行 `DOMAIN,example.com` / `DOMAIN-SUFFIX,…` / `GEOSITE,…`
- `ipcidr` → 文本行 `IP-CIDR,1.2.3.0/24` / `GEOIP,…`
- `classical` → 文本行 `TYPE,payload,target`（target 列被忽略）

部分 provider 还以 YAML wrapper（`payload:\n  - DOMAIN-SUFFIX,…`）而非裸行提供内容。此外 clash 规则行中的 `GEOSITE` / `GEOIP` 引用 geo 数据库，sing-box 的 `source` 规则集无法表达。

## 2. 方案概览

分四个阶段落地，每一阶段都建立在上一阶段之上：

- **P0 解析**：`parse_clash_yaml` 逐文档收集 `rule-providers`，从 `rules:` 中的 `RULE-SET,<name>,<target>` 行推断每个 provider 的建议路由目标，产出一个新的 `ParseResult.rule_providers` 字段。
- **P1 转换器**：新增 `clash_ruleset.rs`，把 clash 规则集正文（裸行或 YAML wrapper）归一化成 sing-box `source` JSON；GEO 条目跳过并计数。
- **P2 下载接入**：在 `remote_rule_auto::refresh_inner` 的下载路径里用 `looks_like_clash_ruleset` 探测 clash 格式，命中则先 `convert_clash_ruleset` 转成 source JSON 再走既有的 source/二进制校验与缓存逻辑——完全复用现有远程规则集基础设施（下载、代理回退、缓存、`remote-rule-set-status` 事件、自动刷新）。
- **P3 前端一键导入**：规则页新增「从订阅导入」入口，弹出 picker 列出所有订阅携带的 provider；点击即「建规则集 → 下载 → 转换 → 缓存 → 启用」。

## 3. 数据流

```
订阅正文（clash YAML）
  └─ parse_clash_yaml                      src-tauri/src/subscription/clash.rs
       ├─ merge_rule_providers              （多文档合并，同名后者胜）
       └─ extract_rule_providers            （含 extract_rule_set_targets 推断目标）
            ↓
  ParseResult.rule_providers: Vec<ClashRuleProvider>      src-tauri/src/domain/node.rs:682
            ↓（导入持久化）
  Subscription.rule_providers               src-tauri/src/domain/subscription.rs:305 / :380
            ↓（跳过 custom 配置类订阅）
  list_subscription_rule_providers           src-tauri/src/commands/subscription.rs:133
            ↓
  前端「从订阅导入」picker                  src/pages/RulesPage.tsx
       openImportProviders → importProvider
            ↓
  create_rule_set(name, url, target, interval)          src-tauri/src/commands/rules.rs:656
            ↓
  refresh_remote_rule_set → remote_rule_auto::refresh   src-tauri/src/commands/rules.rs:846
            ↓
  refresh_inner：looks_like_clash_ruleset               src-tauri/src/remote_rule_auto.rs:292
       ├─ 是 → convert_clash_ruleset                    （clash → sing-box source）
       │        ├─ skipped_geo > 0 → app_log::warn 记录
       │        └─ 转出的 source JSON 喂给 validate_source
       └─ 否 → 原样走 source / .srs 校验
            ↓
  缓存到 <app-data>/remote-rule-sets/<id>-<ts>.json
            ↓
  set_rule_set_enabled(id, true)            src/pages/RulesPage.tsx → commands/rules.rs:460
            ↓
  规则集启用，进入内核配置
```

关键点：

- `Subscription.rule_providers` 随订阅持久化，UI 无需重新拉取订阅正文即可列出 provider。
- `list_subscription_rule_providers` 过滤掉 `custom` 配置类订阅（它们不是节点订阅，`contributes_nodes()` 为 false）。
- 前端 `importProvider` 用一个 provider 名做忙碌态去重；导入成功后从 picker 中移除该项（`RulesPage.tsx:1357`）。
- clash 的 `suggested_target`（`proxy`/`direct`/`reject`）在前端映射为 `RuleTarget`（`reject → block`）；`interval`（秒）映射为 app 的粗粒度更新周期桶（`disabled`/`1h`/`12h`/`24h`，`RulesPage.tsx:1330`）。

## 4. 格式映射表

`convert_clash_ruleset`（`src-tauri/src/clash_ruleset.rs:32`）逐行按类型 token 归类，同类条目聚合成一个 source 规则对象：

| clash 类型 token | sing-box source 字段 | 说明 |
| --- | --- | --- |
| `DOMAIN` | `domain` | 精确域名 |
| `DOMAIN-SUFFIX` | `domain_suffix` | 域名后缀 |
| `DOMAIN-KEYWORD` | `domain_keyword` | 域名关键字 |
| `IP-CIDR` / `IP-CIDR6` | `ip_cidr` | CIDR（含 IPv6） |
| `GEOSITE` / `GEOIP` | —（跳过） | geo 数据库引用，source 规则集无法表达，计入 `skipped_geo` |
| `MATCH` / `FINAL` / `DOMAIN-REGEX` / `PROCESS-NAME` 等 | —（忽略） | source 规则集无对应表达，直接丢弃 |

输出形如：

```json
{"version":3,"rules":[{"domain":["example.com"]},{"domain_suffix":["google.com"]},{"ip_cidr":["1.2.3.0/24","2001:db8::/32"]}]}
```

映射细节：

- 规则类型判断只看每行第一个逗号前的 token（大写归一），不依赖 provider 的 `behavior`——`behavior` 只描述主导类型，不代表每一行。
- 行 payload 为空则跳过。
- 若转换后一个可转换条目都没有：全部是 GEO → 报错「该规则集只包含 GEO 规则，sing-box 规则集无法表达」；否则报错「规则集没有可转换的条目」。
- `normalize_body` 处理 YAML wrapper：正文含 `payload:` 时尝试按 YAML 解析取出 `payload` 序列，元素为字符串直接取用，非字符串 `serde_yaml::to_string` 后 trim；解析失败或没有 `payload` 键则回退到按行拆分。

## 5. GEO 规则处理

`GEOSITE` / `GEOIP` 引用的 geo 数据库（`geosite.dat` / `geoip.dat`）无法被 sing-box `source` 规则集引用，因此：

1. **跳过并计数**：`convert_clash_ruleset` 返回 `ConvertedRuleSet.skipped_geo`，GEO 行不进入输出 JSON。
2. **app_log 记录**：下载路径中若 `skipped_geo > 0`，以 `warn` 级别写入 `remote_rules` 日志：

   ```
   rule set {id} converted from clash: {rule_count} matchers, {skipped_geo} GEO entries skipped
   ```
   （`remote_rule_auto.rs:296`）
3. **全 GEO 时拒绝**：若一个规则集只含 GEO 规则（`rule_count == 0` 且 `skipped_geo > 0`），转换返回错误，下载失败并冒泡给 UI。
4. **前端提示（TODO）**：目前 GEO 跳过仅在 app_log 可见，尚未在导入 picker / 规则集卡片上向用户提示「该规则集包含 N 条被跳过的 GEO 规则」。

## 6. 涉及文件清单

**后端（Rust，`src-tauri/src/`）**

| 文件 | 作用 |
| --- | --- |
| `clash_ruleset.rs` | 新增：clash → sing-box source 转换器（`convert_clash_ruleset` / `looks_like_clash_ruleset` / `normalize_body`） |
| `subscription/clash.rs` | `parse_clash_yaml` 收集 `rule-providers`；`extract_rule_providers` / `extract_rule_set_targets` / `merge_rule_providers` |
| `domain/subscription.rs` | `ClashRuleProvider` 结构（name / behavior / url / interval / suggested_target）；`Subscription.rule_providers` 持久化字段 |
| `domain/node.rs` | `ParseResult.rule_providers` 字段 |
| `remote_rule_auto.rs` | 下载路径接入：`looks_like_clash_ruleset` 探测 + `convert_clash_ruleset` 转换 + GEO 日志 |
| `commands/subscription.rs` | `list_subscription_rule_providers` 命令（`SubscriptionRuleProvider` 视图） |
| `commands/rules.rs` | `create_rule_set` 命令（远端规则集创建，URL 校验 + 更新周期归一） |

**前端（`src/`）**

| 文件 | 作用 |
| --- | --- |
| `pages/RulesPage.tsx` | 「从订阅导入」入口（`openImportProviders`）、picker 弹窗、`importProvider`（建 → 下载 → 启用） |
| `types.ts` | `ClashRuleProvider` / `SubscriptionRuleProvider` 类型 |
| `api.ts` | `listSubscriptionRuleProviders` / `createRuleSet` / `refreshRemoteRuleSet` / `setRuleSetEnabled` 封装 |

## 7. 测试覆盖

**`src-tauri/src/clash_ruleset.rs`（转换器，6 个单测）**

| 测试 | 覆盖点 |
| --- | --- |
| `converts_domain_behavior_and_skips_geo` | DOMAIN / DOMAIN-SUFFIX / DOMAIN-KEYWORD 归类 + GEOSITE 跳过计数 |
| `converts_ipcidr_behavior` | IP-CIDR / IP-CIDR6 归类 + GEOIP 跳过计数 |
| `converts_classical_ignoring_target_column` | classical 行忽略 target 列（`PROXY`、`DIRECT,no-resolve`） |
| `normalizes_yaml_payload_wrapper` | YAML wrapper（`payload:` 列表）归一化 |
| `rejects_geo_only_and_empty` | 全 GEO / 空正文 / 纯注释 → 转换错误 |
| `detector_distinguishes_clash_text_from_json` | `looks_like_clash_ruleset` 区分 clash 文本与 JSON / 空串 |

**`src-tauri/src/subscription/clash.rs`（rule-provider 解析，2 个单测）**

| 测试 | 覆盖点 |
| --- | --- |
| `extracts_rule_providers_with_inferred_targets` | 提取 name / behavior / url / interval；`RULE-SET,ads,REJECT` → `suggested_target="reject"`；无 RULE-SET 引用 → 默认 `"proxy"` |
| `rule_providers_absent_for_plain_proxy_lists` | 纯节点列表（无 `rule-providers`）→ 空列表 |

## 8. 后续 TODO

1. **GEO 规则的 UI 提示**：转换结果里的 `skipped_geo` 目前只进 app_log，可在规则集卡片 / 导入 picker 上展示「跳过 N 条 GEO 规则」。
2. **classical target 列的精细化**：目前 classical 行的 target 列被忽略（导入时以整集策略为准）；未来可考虑将每行 target 带进转换（需要单条规则粒度表达，超出当前 source 规则集模型）。
3. **YAML wrapper 的更多变体**：`normalize_body` 只识别 `payload:` 顶层键；部分 provider 可能使用 `payload` 之外的包裹结构或带 `---` 的多文档正文，需要实测补充。
4. **sing-box 编译期展开 geosite/geoip 的可选方案**：sing-box 支持在编译期将 geosite/geoip 规则展开进规则集（`sing-box rule-set compile` + geodata 参数），可作为替代「跳过 GEO」的后续方向——但会引入 geodata 依赖与体积、更新策略问题，需单独评估。
