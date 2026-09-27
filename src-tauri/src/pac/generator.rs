//! PAC 脚本生成器。
//!
//! 把 [`crate::pac::PacList`] 编译成一段 `FindProxyForURL` JavaScript，
//! 国内流量默认直连，命中名单的域名 / IP 段走代理。

use super::PacList;

/// 生成完整 PAC JavaScript 文本。
///
/// 生成的脚本结构：
/// ```js
/// function FindProxyForURL(url, host) {
///   var PROXY = "PROXY 127.0.0.1:2080; DIRECT";
///   if (dnsDomainIs(host, ".google.com")) return PROXY;
///   if (isInNet(host, "1.2.3.0", "255.255.255.0")) return PROXY;
///   return "DIRECT";
/// }
/// ```
///
/// 无法解析的 IP/CIDR 条目会被静默跳过；`proxy_host` / 名单内容由本应用
/// 内部维护，直接以字面量嵌入 JS 字符串（调用方应传入可信值）。
pub fn generate_pac(list: &PacList, proxy_host: &str, proxy_port: u16) -> String {
    let mut rules: Vec<String> = Vec::new();

    // 域名（后缀匹配）：统一去前导点、加一个点前缀。
    for domain in &list.domains {
        let d = normalize_dot_domain(domain);
        if !d.is_empty() {
            rules.push(format!(
                "    if (dnsDomainIs(host, \".{d}\")) return PROXY;"
            ));
        }
    }

    // IP / CIDR。
    for item in &list.ip_cidrs {
        if item.contains('/') {
            if let Some((ip, mask)) = split_cidr(item) {
                rules.push(format!(
                    "    if (isInNet(host, \"{ip}\", \"{mask}\")) return PROXY;"
                ));
            }
        } else if is_valid_ipv4(item.trim()) {
            let ip = item.trim();
            rules.push(format!("    if (host == \"{ip}\") return PROXY;"));
        }
        // 无法解析的条目跳过。
    }

    // 顶级后缀：.hk / .tw 之类。
    for suffix in &list.suffixes {
        let s = normalize_dot_domain(suffix);
        if !s.is_empty() {
            rules.push(format!(
                "    if (dnsDomainIs(host, \".{s}\")) return PROXY;"
            ));
        }
    }

    // 地区预置：展开为预置 IP 段逐个生成 isInNet。
    for region in &list.regions {
        for cidr in region_cidrs(region) {
            if let Some((ip, mask)) = split_cidr(cidr) {
                rules.push(format!(
                    "    if (isInNet(host, \"{ip}\", \"{mask}\")) return PROXY;"
                ));
            }
        }
    }

    let mut out = String::new();
    out.push_str("function FindProxyForURL(url, host) {\n");
    out.push_str(&format!(
        "  var PROXY = \"PROXY {proxy_host}:{proxy_port}; DIRECT\";\n"
    ));
    for rule in &rules {
        out.push_str(rule);
        out.push('\n');
    }
    out.push_str("  return \"DIRECT\";\n");
    out.push_str("}\n");
    out
}

/// 地区 → 预置 CIDR 段。
///
/// 内置少量代表性网段（非全量，仅作便捷预置）：香港 / 台湾。未知地区返回空切片。
pub fn region_cidrs(region: &str) -> &'static [&'static str] {
    match region {
        // 香港：PCCW / HKT / HGC 等代表性聚合段（非全量）。
        "hk" => &[
            "1.36.0.0/16",
            "1.64.0.0/14",
            "58.176.0.0/13",
            "113.252.0.0/14",
            "144.214.0.0/16",
            "203.80.0.0/12",
        ],
        // 台湾：HiNet / 中华电信 / 台湾大等代表性聚合段（非全量）。
        "tw" => &[
            "1.34.0.0/15",
            "1.160.0.0/12",
            "59.112.0.0/13",
            "111.240.0.0/13",
            "114.32.0.0/12",
            "140.112.0.0/16",
        ],
        _ => &[],
    }
}

/// 把 CIDR 拆成 (IP, 掩码)，无法解析（含前缀越界）返回 `None`。
pub fn split_cidr(cidr: &str) -> Option<(String, String)> {
    let (ip, prefix_str) = cidr.split_once('/')?;
    let ip = ip.trim();
    let prefix: u8 = prefix_str.trim().parse().ok()?;
    if prefix > 32 || !is_valid_ipv4(ip) {
        return None;
    }
    Some((ip.to_string(), cidr_to_mask(prefix)))
}

/// CIDR 前缀长度 → 点分十进制掩码。
///
/// 0-32 正常换算；越界（>32）按宽容处理，返回 `255.255.255.255`。
pub fn cidr_to_mask(prefix: u8) -> String {
    if prefix > 32 {
        return "255.255.255.255".to_string();
    }
    let bits: u32 = if prefix == 0 { 0 } else { u32::MAX << (32 - prefix) };
    format!(
        "{}.{}.{}.{}",
        (bits >> 24) & 0xFF,
        (bits >> 16) & 0xFF,
        (bits >> 8) & 0xFF,
        bits & 0xFF
    )
}

/// 去掉条目里可能的前导 `.`，返回裸域名/后缀（trim 后）。
fn normalize_dot_domain(s: &str) -> String {
    s.trim().trim_start_matches('.').to_string()
}

/// 校验是否为合法 IPv4 地址（四段、0-255）。
fn is_valid_ipv4(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 4 {
        return false;
    }
    parts.iter().all(|part| {
        !part.is_empty()
            && part.len() <= 3
            && part.bytes().all(|b| b.is_ascii_digit())
            && part.parse::<u16>().is_ok_and(|n| n <= 255)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pac::PacList;

    #[test]
    fn cidr_to_mask_converts_common_prefixes() {
        assert_eq!(cidr_to_mask(0), "0.0.0.0");
        assert_eq!(cidr_to_mask(8), "255.0.0.0");
        assert_eq!(cidr_to_mask(16), "255.255.0.0");
        assert_eq!(cidr_to_mask(24), "255.255.255.0");
        assert_eq!(cidr_to_mask(32), "255.255.255.255");
    }

    #[test]
    fn cidr_to_mask_clamps_out_of_range() {
        assert_eq!(cidr_to_mask(33), "255.255.255.255");
        assert_eq!(cidr_to_mask(255), "255.255.255.255");
    }

    #[test]
    fn split_cidr_parses_ip_and_mask() {
        assert_eq!(
            split_cidr("1.2.3.0/24"),
            Some(("1.2.3.0".to_string(), "255.255.255.0".to_string()))
        );
        assert_eq!(
            split_cidr("10.0.0.0/8"),
            Some(("10.0.0.0".to_string(), "255.0.0.0".to_string()))
        );
    }

    #[test]
    fn split_cidr_rejects_bad_entries() {
        assert_eq!(split_cidr("1.2.3.0/33"), None); // 前缀越界
        assert_eq!(split_cidr("not-an-ip/24"), None);
        assert_eq!(split_cidr("1.2.3.999/24"), None);
        assert_eq!(split_cidr("1.2.3.0/abc"), None);
        assert_eq!(split_cidr("no-slash"), None);
    }

    #[test]
    fn generate_pac_emits_dns_domain_rules() {
        let list = PacList {
            domains: vec![".google.com".into(), "youtube.com".into()],
            ..Default::default()
        };
        let pac = generate_pac(&list, "127.0.0.1", 2080);
        assert!(pac.contains("var PROXY = \"PROXY 127.0.0.1:2080; DIRECT\";"));
        assert!(pac.contains("if (dnsDomainIs(host, \".google.com\")) return PROXY;"));
        assert!(pac.contains("if (dnsDomainIs(host, \".youtube.com\")) return PROXY;"));
        assert!(pac.contains("return \"DIRECT\";"));
    }

    #[test]
    fn generate_pac_emits_is_in_net_for_cidrs() {
        let list = PacList {
            ip_cidrs: vec![
                "1.2.3.0/24".into(),
                "8.8.8.8".into(),
                "bad-entry".into(),
            ],
            ..Default::default()
        };
        let pac = generate_pac(&list, "127.0.0.1", 2080);
        assert!(pac.contains("if (isInNet(host, \"1.2.3.0\", \"255.255.255.0\")) return PROXY;"));
        assert!(pac.contains("if (host == \"8.8.8.8\") return PROXY;"));
        assert!(!pac.contains("bad-entry"));
    }

    #[test]
    fn generate_pac_emits_suffixes_and_regions() {
        let list = PacList {
            suffixes: vec![".hk".into(), "tw".into()],
            regions: vec!["hk".into()],
            ..Default::default()
        };
        let pac = generate_pac(&list, "127.0.0.1", 2080);
        assert!(pac.contains("if (dnsDomainIs(host, \".hk\")) return PROXY;"));
        assert!(pac.contains("if (dnsDomainIs(host, \".tw\")) return PROXY;"));
        // 地区 hk 展开出预置段。
        assert!(pac.contains("if (isInNet(host, \"1.36.0.0\", \"255.255.0.0\")) return PROXY;"));
        assert!(pac.contains("if (isInNet(host, \"1.64.0.0\", \"255.252.0.0\")) return PROXY;"));
    }

    #[test]
    fn generate_pac_ignores_unknown_region() {
        let list = PacList {
            regions: vec!["unknown-region".into()],
            ..Default::default()
        };
        let pac = generate_pac(&list, "127.0.0.1", 2080);
        assert!(!pac.contains("isInNet"));
    }

    #[test]
    fn empty_list_generates_minimal_pac() {
        let pac = generate_pac(&PacList::default(), "127.0.0.1", 2080);
        assert!(pac.starts_with("function FindProxyForURL(url, host) {"));
        assert!(pac.ends_with("}\n"));
        assert!(!pac.contains("dnsDomainIs"));
        assert!(!pac.contains("isInNet"));
    }
}
