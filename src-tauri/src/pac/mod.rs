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
}

impl PacList {
    /// 参与生成 PAC 脚本的全部域名（自定义 + 上游，保持顺序去重）。
    pub fn all_domains(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        self.domains
            .iter()
            .chain(self.gfwlist_domains.iter())
            .filter_map(|d| {
                let d = d.trim();
                if d.is_empty() || !seen.insert(d.to_string()) {
                    None
                } else {
                    Some(d.to_string())
                }
            })
            .collect()
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
    fn source_presets_are_unique_and_https() {
        let presets = source_presets();
        assert!(presets.len() >= 3);
        for preset in &presets {
            assert!(preset.url.starts_with("https://"), "{}", preset.url);
        }
        assert_eq!(presets[0].url, default_source_url());
    }
}