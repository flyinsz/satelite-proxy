//! PAC 自动代理模块。
//!
//! 为「反向 PAC」方案提供基础能力：根据名单生成 PAC 脚本
//! （[`generator`]）、解析/拉取 gfwlist 域名（[`gfwlist`]）、
//! 一个本地最小 HTTP 服务托管 `proxy.pac`（[`server`]），
//! 以及按间隔自动拉取名单的后台任务（[`auto`]）。

pub mod auto;
pub mod generator;
pub mod gfwlist;
pub mod server;

use serde::{Deserialize, Serialize};

/// 内置 gfwlist 更新地址预设。
///
/// 上游在 GitHub raw，国内直连常失败，因此同时内置 jsDelivr / Fastly
/// 两个 CDN 镜像，供 UI 一键切换。
pub const PAC_SOURCE_PRESETS: &[(&str, &str, &str)] = &[
    (
        "github",
        "GitHub raw",
        "https://raw.githubusercontent.com/gfwlist/gfwlist/master/gfwlist.txt",
    ),
    (
        "jsdelivr",
        "jsDelivr CDN",
        "https://cdn.jsdelivr.net/gh/gfwlist/gfwlist@master/gfwlist.txt",
    ),
    (
        "fastly",
        "Fastly CDN",
        "https://fastly.jsdelivr.net/gh/gfwlist/gfwlist@master/gfwlist.txt",
    ),
];

/// 默认更新地址（GitHub raw）。
pub fn default_source_url() -> String {
    PAC_SOURCE_PRESETS[0].2.to_string()
}

/// 内置「中国直连」数据源（反向模式「大陆以外」用）——纯文本，每行一条。
///
/// 两个源都由 fcshark-org/route-list 每日自动更新，走 jsDelivr CDN 保证
/// 国内可访问：
///   - 中国 IPv4 段（chnroute CIDR）
///   - 中国域名列表（每行一个域名）
pub const CHINA_IPV4_URL: &str =
    "https://cdn.jsdelivr.net/gh/fcshark-org/route-list@release/china_ipv4.txt";
pub const CHINA_DOMAINS_URL: &str =
    "https://cdn.jsdelivr.net/gh/fcshark-org/route-list@release/china_list.txt";

/// 镜像预设条目（`list_pac_source_presets` 的返回体）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PacSourcePreset {
    pub id: String,
    pub label: String,
    pub url: String,
}

/// 全部内置更新地址预设。
pub fn source_presets() -> Vec<PacSourcePreset> {
    PAC_SOURCE_PRESETS
        .iter()
        .map(|(id, label, url)| PacSourcePreset {
            id: (*id).to_string(),
            label: (*label).to_string(),
            url: (*url).to_string(),
        })
        .collect()
}

/// 一个可独立启用的名单分组（对应规则页的「规则集」）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PacGroup {
    /// 分组唯一 ID（前端生成，形如 group-<ts>）。
    pub id: String,
    /// 显示名。
    pub name: String,
    /// 是否参与 PAC 生成。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 条目（后缀域名列表，语义同 `domains`）。
    #[serde(default)]
    pub items: Vec<String>,
    /// 远程来源 URL（可选；`None` 表示纯本地分组，只能手改条目）。
    #[serde(default)]
    pub remote_url: Option<String>,
    /// 是否对该分组自动更新（仅当有远程地址时有效）。
    #[serde(default)]
    pub auto_update: bool,
    /// 自动更新间隔（小时），`None` 用默认值。
    #[serde(default)]
    pub update_interval_hours: Option<u32>,
}

fn default_true() -> bool {
    true
}

/// PAC 名单。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PacList {
    /// 用户自定义域名（后缀匹配）：google.com / .google.com 均按后缀处理。
    /// 刷新 gfwlist 不会覆盖这里的条目。
    pub domains: Vec<String>,
    /// gfwlist 上游同步来的域名。与 `domains` 分开放，刷新时整体替换，
    /// 既避免反复刷新导致名单无限膨胀，也保证不冲掉用户手改内容。
    #[serde(default)]
    pub gfwlist_domains: Vec<String>,
    /// IP/CIDR：1.2.3.0/24、1.2.3.4。
    pub ip_cidrs: Vec<String>,
    /// 顶级后缀：.hk / .tw。
    pub suffixes: Vec<String>,
    /// 地区预置（如 hk / tw）——解析为预置 IP 段参与匹配。
    pub regions: Vec<String>,
    /// 自定义分组（每个分组独立启用，可增删改名）。
    #[serde(default)]
    pub groups: Vec<PacGroup>,
    /// 中国直连域名（后缀匹配，如 `cn` / `.com.cn`）。反向模式
    /// （「大陆以外」）下命中即直连。
    #[serde(default)]
    pub china_domains: Vec<String>,
    /// 中国 IP 段（chnroute，CIDR）。反向模式下命中即直连。
    #[serde(default)]
    pub china_ip_cidrs: Vec<String>,
}

impl PacList {
    /// 某内置分类是否启用：由 `groups` 中同 id 的条目决定；缺失视为启用
    /// （兼容旧存储没有 `groups` 字段的情况）。
    pub fn group_enabled(&self, id: &str) -> bool {
        self.groups
            .iter()
            .find(|g| g.id == id)
            .map(|g| g.enabled)
            .unwrap_or(true)
    }

    /// 参与生成 PAC 脚本的全部域名（自定义 + 上游 + 启用的自定义分组，保持顺序去重）。
    ///
    /// 内置 `domains` / `gfwlist_domains` 两类的开关也在此生效：关闭后对应
    /// 条目不再进入脚本，与 ip_cidrs / suffixes / regions 行为一致。
    pub fn all_domains(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut out: Vec<String> = Vec::new();

        if self.group_enabled("domains") {
            for d in &self.domains {
                let d = d.trim();
                if !d.is_empty() && seen.insert(d.to_string()) {
                    out.push(d.to_string());
                }
            }
        }
        if self.group_enabled("gfwlist_domains") {
            for d in &self.gfwlist_domains {
                let d = d.trim();
                if !d.is_empty() && seen.insert(d.to_string()) {
                    out.push(d.to_string());
                }
            }
        }
        for g in self.groups.iter().filter(|g| g.enabled) {
            for d in &g.items {
                let d = d.trim();
                if !d.is_empty() && seen.insert(d.to_string()) {
                    out.push(d.to_string());
                }
            }
        }
        out
    }

    /// 生成 PAC 规则时用的总域名数。
    pub fn domain_count(&self) -> usize {
        self.all_domains().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_domains_merges_custom_and_upstream_without_duplicates() {
        let list = PacList {
            domains: vec!["custom.com".into(), "shared.com".into(), "  ".into()],
            gfwlist_domains: vec!["shared.com".into(), "upstream.com".into()],
            ..PacList::default()
        };
        assert_eq!(
            list.all_domains(),
            vec!["custom.com", "shared.com", "upstream.com"]
        );
        assert_eq!(list.domain_count(), 3);
    }

    #[test]
    fn legacy_store_without_gfwlist_field_loads() {
        // 旧 store 只有 domains，新字段必须能默认加载。
        let legacy = r#"{"domains":["a.com"],"ip_cidrs":[],"suffixes":[],"regions":[]}"#;
        let list: PacList = serde_json::from_str(legacy).unwrap();
        assert!(list.gfwlist_domains.is_empty());
        assert_eq!(list.all_domains(), vec!["a.com"]);
    }

    #[test]
    fn all_domains_respects_builtin_enable_flags() {
        // 关闭 domains / gfwlist_domains 内置分类后，对应条目不再参与生成。
        let list = PacList {
            domains: vec!["custom.com".into()],
            gfwlist_domains: vec!["upstream.com".into()],
            groups: vec![
                PacGroup {
                    id: "domains".into(),
                    name: "domains".into(),
                    enabled: false,
                    items: vec![],
                    remote_url: None,
                    auto_update: false,
                    update_interval_hours: None,
                },
                PacGroup {
                    id: "gfwlist_domains".into(),
                    name: "gfwlist_domains".into(),
                    enabled: false,
                    items: vec![],
                    remote_url: None,
                    auto_update: false,
                    update_interval_hours: None,
                },
            ],
            ..PacList::default()
        };
        assert_eq!(list.all_domains(), Vec::<String>::new());

        // 缺 groups 字段（旧存储）时默认启用，兼容不破坏。
        let legacy = PacList {
            domains: vec!["custom.com".into()],
            ..PacList::default()
        };
        assert_eq!(legacy.all_domains(), vec!["custom.com"]);
    }

    #[test]
    fn source_presets_are_unique_and_https() {
        let presets = source_presets();
        assert!(presets.len() >= 3);
        for preset in &presets {
            assert!(preset.url.starts_with("https://"), "{}", preset.url);
        }
        assert_eq!(presets[0].url, default_source_url());
    }
}