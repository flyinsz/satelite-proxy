//! PAC 脚本生成器。
//!
//! 把 [`crate::pac::PacList`] 编译成一段 `FindProxyForURL` JavaScript，
//! 国内流量默认直连，命中名单的域名 / IP 段走代理。

use super::PacList;

/// 生成完整 PAC JavaScript 文本。
///
/// 两种模式：
///  - **白名单**（`china_direct == false`，默认）：命中名单的域名 / IP 段走代理，
///    其余直连。
///  - **反向「大陆以外」**（`china_direct == true`）：内网 / 回环 / 中国域名 /
///    中国 IP 段直连，其余**默认走代理**。
///
/// 生成的脚本结构（白名单）：
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
pub fn generate_pac(
    list: &PacList,
    proxy_host: &str,
    proxy_port: u16,
    china_direct: bool,
) -> String {
    if china_direct {
        return generate_china_direct_pac(list, proxy_host, proxy_port);
    }

    let mut rules: Vec<String> = Vec::new();

    // 域名（后缀匹配）：统一去前导点、加一个点前缀。
    // 自定义域名与 gfwlist 上游域名合并去重后再生成，避免重复规则。
    //
    // 关键：PAC 的 `dnsDomainIs(host, ".d")` 只匹配「子域」（如
    // "www.d"），不匹配「裸域名」（"d"）。直接访问裸域名（如
    // `https://github.com`）会被漏判。故每条规则同时做裸域名精确匹配
    // `host == "d"`，与子域后缀匹配一起覆盖。
    for domain in list.all_domains() {
        let d = normalize_dot_domain(&domain);
        if !d.is_empty() {
            rules.push(format!(
                "    if (host == \"{d}\" || dnsDomainIs(host, \".{d}\")) return PROXY;"
            ));
        }
    }

    // IP / CIDR。
    if list.group_enabled("ip_cidrs") {
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
    }

    // 顶级后缀：.hk / .tw 之类。
    if list.group_enabled("suffixes") {
        for suffix in &list.suffixes {
            let s = normalize_dot_domain(suffix);
            if !s.is_empty() {
                rules.push(format!(
                    "    if (dnsDomainIs(host, \".{s}\")) return PROXY;"
                ));
            }
        }
    }

    // 地区预置：展开为预置 IP 段逐个生成 isInNet。
    if list.group_enabled("regions") {
        for region in &list.regions {
            for cidr in region_cidrs(region) {
                if let Some((ip, mask)) = split_cidr(cidr) {
                    rules.push(format!(
                        "    if (isInNet(host, \"{ip}\", \"{mask}\")) return PROXY;"
                    ));
                }
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

/// 反向「大陆以外」PAC：内网 / 回环 / 中国域名 / 中国 IP 段直连，其余默认代理。
fn generate_china_direct_pac(list: &PacList, proxy_host: &str, proxy_port: u16) -> String {
    let mut out = String::new();
    out.push_str("function FindProxyForURL(url, host) {\n");
    out.push_str(&format!(
        "  var PROXY = \"PROXY {proxy_host}:{proxy_port}; DIRECT\";\n"
    ));

    // 内网 / 回环直连：先于中国规则，避免代理自身（127.0.0.1:端口）成环。
    out.push_str("  if (isPlainHostName(host)) return \"DIRECT\";\n");
    out.push_str("  if (isInNet(host, \"127.0.0.0\", \"255.0.0.0\")) return \"DIRECT\";\n");
    out.push_str("  if (isInNet(host, \"10.0.0.0\", \"255.0.0.0\")) return \"DIRECT\";\n");
    out.push_str("  if (isInNet(host, \"172.16.0.0\", \"255.240.0.0\")) return \"DIRECT\";\n");
    out.push_str("  if (isInNet(host, \"192.168.0.0\", \"255.255.0.0\")) return \"DIRECT\";\n");
    out.push_str("  if (isInNet(host, \"169.254.0.0\", \"255.255.0.0\")) return \"DIRECT\";\n");

    // 中国 IP 段直连（chnroute CIDR）。
    //
    // 刻意不枚举 `china_domains`（十万量级）——那会让 PAC 膨胀到数 MB，
    // 超出浏览器可执行的 PAC 体积上限（实测 Chrome > ~1MB 即放弃 PAC
    // 回退直连），反而导致代理完全失效。
    //
    // 中国域名的直连判定靠 `isInNet`：PAC 引擎的 isInNet 会对**域名**做
    // DNS 解析后再比对 IP 段（实测 Chrome 对 baidu.com 能命中 183.0.0.0/8
    // 而 DIRECT）。因此国内域名仍在 PAC 层被 IP 段拦截直连、不进内核，
    // 节点故障不影响国内访问——与「国内流量不进内核」的初衷一致。
    for item in &list.china_ip_cidrs {
        if let Some((ip, mask)) = split_cidr(item) {
            out.push_str(&format!(
                "  if (isInNet(host, \"{ip}\", \"{mask}\")) return \"DIRECT\";\n"
            ));
        }
    }

    out.push_str("  return PROXY;\n");
    out.push_str("}\n");
    out
}

/// 统计名单会生成多少条 PAC 匹配规则。
///
/// 与 [`generate_pac`] 的入口口径一致：域名合并去重、IP/CIDR 只算能解析的、
/// 地区按展开后的 CIDR 条数计。反向模式则统计内网（固定 6 条）+ 中国 IP 段。
/// 供 UI 展示「规则数」用。
pub fn count_pac_rules(list: &PacList, china_direct: bool) -> usize {
    if china_direct {
        // 内网/回环固定 6 条（isPlainHostName + 5 个网段）。
        // 反向模式不再枚举 `china_domains`：十万量级会让 PAC 超过浏览器
        // 可执行体积上限（Chrome > ~1MB 放弃 PAC 回退直连），中国域名直连
        // 交给 `isInNet` 对域名的 DNS 解析 + 中国 IP 段（浏览器 PAC 引擎
        // 的 isInNet 会解析域名）。此处仅计 IP 段。
        let mut count = 6;
        count += list
            .china_ip_cidrs
            .iter()
            .filter(|cidr| split_cidr(cidr).is_some())
            .count();
        return count;
    }

    let mut count = list.all_domains().len();

    if list.group_enabled("ip_cidrs") {
        for item in &list.ip_cidrs {
            let valid = if item.contains('/') {
                split_cidr(item).is_some()
            } else {
                is_valid_ipv4(item.trim())
            };
            if valid {
                count += 1;
            }
        }
    }

    if list.group_enabled("suffixes") {
        for suffix in &list.suffixes {
            if !normalize_dot_domain(suffix).is_empty() {
                count += 1;
            }
        }
    }

    if list.group_enabled("regions") {
        for region in &list.regions {
            count += region_cidrs(region)
                .iter()
                .filter(|cidr| split_cidr(cidr).is_some())
                .count();
        }
    }

    count
}

/// 地区 → 预置 CIDR 段。
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
    fn gfwlist_domains_are_emitted_and_deduplicated_against_custom() {
        let list = PacList {
            domains: vec!["shared.com".into()],
            gfwlist_domains: vec!["shared.com".into(), "google.com".into()],
            ..PacList::default()
        };
        let script = generate_pac(&list, "127.0.0.1", 2080, false);
        // shared.com 只出现一次（自定义与上游重复）。
        assert_eq!(script.matches("dnsDomainIs(host, \".shared.com\")").count(), 1);
        // 裸域名 + 子域都覆盖。
        assert!(script.contains("host == \"google.com\" || dnsDomainIs(host, \".google.com\")"));
        assert_eq!(count_pac_rules(&list, false), 2);
    }

    #[test]
    fn count_pac_rules_matches_generated_rule_lines() {
        let list = PacList {
            domains: vec!["a.com".into(), "  ".into()],
            gfwlist_domains: vec!["b.com".into()],
            ip_cidrs: vec!["1.2.3.4".into(), "10.0.0.0/8".into(), "bad/garbage".into()],
            suffixes: vec![".githubusercontent.com".into(), String::new()],
            regions: vec!["hk".into(), "unknown".into()],
            ..PacList::default()
        };
        let script = generate_pac(&list, "127.0.0.1", 2080, false);
        let emitted = script
            .lines()
            .filter(|line| line.trim_start().starts_with("if ("))
            .count();
        assert_eq!(
            count_pac_rules(&list, false),
            emitted,
            "计数必须与实际生成的规则行数一致"
        );
    }

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
        let pac = generate_pac(&list, "127.0.0.1", 2080, false);
        assert!(pac.contains("var PROXY = \"PROXY 127.0.0.1:2080; DIRECT\";"));
        // 裸域名 + 子域都覆盖（修复 dnsDomainIs 漏判裸域名）。
        assert!(pac.contains("host == \"google.com\" || dnsDomainIs(host, \".google.com\")"));
        assert!(pac.contains("host == \"youtube.com\" || dnsDomainIs(host, \".youtube.com\")"));
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
        let pac = generate_pac(&list, "127.0.0.1", 2080, false);
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
        let pac = generate_pac(&list, "127.0.0.1", 2080, false);
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
        let pac = generate_pac(&list, "127.0.0.1", 2080, false);
        assert!(!pac.contains("isInNet"));
    }

    #[test]
    fn empty_list_generates_minimal_pac() {
        let pac = generate_pac(&PacList::default(), "127.0.0.1", 2080, false);
        assert!(pac.starts_with("function FindProxyForURL(url, host) {"));
        assert!(pac.ends_with("}\n"));
        assert!(!pac.contains("dnsDomainIs"));
        assert!(!pac.contains("isInNet"));
    }

    #[test]
    fn china_direct_mode_defaults_to_proxy() {
        // 反向模式：默认走代理（return PROXY 而非 DIRECT）。
        let pac = generate_pac(&PacList::default(), "127.0.0.1", 2080, true);
        assert!(pac.contains("return PROXY;"));
        assert!(!pac.contains("return \"DIRECT\";\n}"));
        // 内网 / 回环直连（含代理自身 127.0.0.1，避免成环）。
        assert!(pac.contains("if (isPlainHostName(host)) return \"DIRECT\";"));
        assert!(pac.contains("if (isInNet(host, \"127.0.0.0\", \"255.0.0.0\")) return \"DIRECT\";"));
        assert!(pac.contains("if (isInNet(host, \"10.0.0.0\", \"255.0.0.0\")) return \"DIRECT\";"));
        assert!(pac.contains("if (isInNet(host, \"172.16.0.0\", \"255.240.0.0\")) return \"DIRECT\";"));
        assert!(pac.contains("if (isInNet(host, \"192.168.0.0\", \"255.255.0.0\")) return \"DIRECT\";"));
        assert!(pac.contains("if (isInNet(host, \"169.254.0.0\", \"255.255.0.0\")) return \"DIRECT\";"));
    }

    #[test]
    fn china_direct_mode_emits_china_cidrs_only_skipping_domains() {
        // 反向模式刻意不枚举 china_domains：十万量级会让 PAC 撑到数 MB，
        // 超过浏览器可执行上限（Chrome > ~1MB 放弃 PAC 回退直连），
        // 中国域名直连改由内核 `GEOSITE,cn,DIRECT` 兜底。仅生成 IP 段。
        let list = PacList {
            china_domains: vec!["cn".into(), ".com.cn".into()],
            china_ip_cidrs: vec!["1.0.1.0/24".into(), "bad/garbage".into()],
            ..Default::default()
        };
        let pac = generate_pac(&list, "127.0.0.1", 2080, true);
        // 域名规则不再出现（体积上限兜底）。
        assert!(!pac.contains("dnsDomainIs"));
        // IP 段仍生成，非法条目跳过。
        assert!(pac.contains("if (isInNet(host, \"1.0.1.0\", \"255.255.255.0\")) return \"DIRECT\";"));
        assert!(!pac.contains("bad/garbage"));
        assert!(pac.contains("return PROXY;"));
        // 计数与实际规则行一致：6 内网 + 1 合法 CIDR（域名不计、非法跳过）。
        assert_eq!(count_pac_rules(&list, true), 7);
    }

    #[test]
    fn china_direct_mode_ignores_whitelist_fields() {
        // 反向模式下 gfwlist / 自定义域名不参与（默认已走代理）。
        let list = PacList {
            domains: vec!["custom.com".into()],
            gfwlist_domains: vec!["google.com".into()],
            suffixes: vec!["hk".into()],
            regions: vec!["hk".into()],
            ..Default::default()
        };
        let pac = generate_pac(&list, "127.0.0.1", 2080, true);
        assert!(!pac.contains("custom.com"));
        assert!(!pac.contains("google.com"));
        assert_eq!(count_pac_rules(&list, true), 6);
    }

    #[test]
    fn china_direct_pac_stays_under_browser_size_limit() {
        // 回归护栏：反向 PAC 必须远小于浏览器可执行上限。
        // 实测 Chrome 对 > ~1MB 的 PAC 会放弃执行并回退直连（代理完全失效），
        // 根因是旧实现枚举了十万量级 china_domains。此处用真实量级名单断言
        // 输出体积不会失控。
        let list = PacList {
            // 模拟线上量级：11 万中国域名 + 6.7 千 CIDR。
            china_domains: (0..110_000).map(|i| format!("d{i}.example.cn")).collect(),
            china_ip_cidrs: (0..6_700)
                .map(|i| format!("1.{}.{}.0/24", i / 256, i % 256))
                .collect(),
            ..Default::default()
        };
        let pac = generate_pac(&list, "127.0.0.1", 2080, true);
        // 域名不再进 PAC —— 体积应远小于 1MB（约 500KB 量级，纯 IP 段）。
        assert!(
            pac.len() < 1_000_000,
            "反向 PAC 体积 {} 字节超过浏览器 ~1MB 上限（会导致 Chrome 放弃 PAC 直连）",
            pac.len()
        );
        assert!(!pac.contains("dnsDomainIs"));
    }
}
