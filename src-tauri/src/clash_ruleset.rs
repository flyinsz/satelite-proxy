//! Convert clash-format rule sets to sing-box `source` rule-set JSON.
//!
//! Clash `rule-providers` point at three shapes (`behavior`):
//! - `domain`   → text lines `DOMAIN,example.com` / `DOMAIN-SUFFIX,…` / `GEOSITE,…`
//! - `ipcidr`   → text lines `IP-CIDR,1.2.3.0/24` / `GEOIP,…`
//! - `classical`→ text lines `TYPE,payload,target` (target column is ignored)
//!
//! Some providers serve a YAML wrapper (`payload:\n  - DOMAIN-SUFFIX,…`) instead
//! of bare lines; [`convert_clash_ruleset`] normalizes both. GEO references
//! (`GEOSITE` / `GEOIP`) cannot be represented in a sing-box `source` rule set,
//! so they are skipped and counted for the UI.

use serde_json::json;

/// Outcome of converting one clash rule-set body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvertedRuleSet {
    /// sing-box source JSON body (`{"version":3,"rules":[…]}`).
    pub json: String,
    /// Number of individual matcher entries converted (before array merging).
    pub rule_count: usize,
    /// GEO entries (`GEOSITE` / `GEOIP`) skipped because sing-box source
    /// rule sets cannot reference geo databases.
    pub skipped_geo: usize,
}

/// Convert a clash rule-set body to sing-box source JSON.
///
/// Each clash rule line carries its own type token (`DOMAIN` / `IP-CIDR` /
/// `GEOSITE` …), so the provider's `behavior` is not needed here — it only
/// describes the *dominant* type, never the actual per-line matcher.
pub fn convert_clash_ruleset(body: &str) -> Result<ConvertedRuleSet, String> {
    let lines = normalize_body(body);

    let mut domain: Vec<String> = Vec::new();
    let mut domain_suffix: Vec<String> = Vec::new();
    let mut domain_keyword: Vec<String> = Vec::new();
    let mut ip_cidr: Vec<String> = Vec::new();
    let mut rule_count = 0usize;
    let mut skipped_geo = 0usize;

    for line in &lines {
        let mut parts = line.split(',');
        let kind = parts.next().unwrap_or("").trim().to_ascii_uppercase();
        let payload = parts.next().unwrap_or("").trim();
        if payload.is_empty() {
            continue;
        }
        match kind.as_str() {
            "DOMAIN" => {
                domain.push(payload.to_string());
                rule_count += 1;
            }
            "DOMAIN-SUFFIX" => {
                domain_suffix.push(payload.to_string());
                rule_count += 1;
            }
            "DOMAIN-KEYWORD" => {
                domain_keyword.push(payload.to_string());
                rule_count += 1;
            }
            "IP-CIDR" | "IP-CIDR6" => {
                ip_cidr.push(payload.to_string());
                rule_count += 1;
            }
            "GEOSITE" | "GEOIP" => skipped_geo += 1,
            // MATCH / FINAL / DOMAIN-REGEX / PROCESS-NAME etc. have no sing-box
            // source equivalent — ignore them.
            _ => {}
        }
    }

    if rule_count == 0 {
        return Err(if skipped_geo > 0 {
            "该规则集只包含 GEO 规则，sing-box 规则集无法表达".into()
        } else {
            "规则集没有可转换的条目".into()
        });
    }

    let mut rules: Vec<serde_json::Value> = Vec::new();
    if !domain.is_empty() {
        rules.push(json!({ "domain": domain }));
    }
    if !domain_suffix.is_empty() {
        rules.push(json!({ "domain_suffix": domain_suffix }));
    }
    if !domain_keyword.is_empty() {
        rules.push(json!({ "domain_keyword": domain_keyword }));
    }
    if !ip_cidr.is_empty() {
        rules.push(json!({ "ip_cidr": ip_cidr }));
    }

    let doc = json!({ "version": 3, "rules": rules });
    let json = serde_json::to_string(&doc).map_err(|e| format!("serialize rule set: {e}"))?;
    Ok(ConvertedRuleSet {
        json,
        rule_count,
        skipped_geo,
    })
}

/// Does this body look like a clash rule list (not sing-box JSON, not SRS)?
///
/// Used by the download path to decide whether to run the clash converter
/// before the existing sing-box `source` validation.
pub fn looks_like_clash_ruleset(body: &str) -> bool {
    let head = body.trim_start();
    if head.starts_with('{') || head.starts_with('[') {
        return false;
    }
    body.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("//"))
        .take(10)
        .any(|line| {
            let kind = line.split(',').next().unwrap_or("").trim().to_ascii_uppercase();
            matches!(
                kind.as_str(),
                "DOMAIN"
                    | "DOMAIN-SUFFIX"
                    | "DOMAIN-KEYWORD"
                    | "IP-CIDR"
                    | "IP-CIDR6"
                    | "GEOSITE"
                    | "GEOIP"
                    | "DOMAIN-REGEX"
                    | "MATCH"
                    | "FINAL"
            )
        })
}

/// Normalize a raw provider body into bare `TYPE,payload,…` lines.
///
/// Handles the YAML wrapper some providers use (`payload:` list of strings).
fn normalize_body(body: &str) -> Vec<String> {
    let trimmed = body.trim();
    if trimmed.contains("payload:") {
        if let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(trimmed) {
            if let Some(payload) = value.get("payload").and_then(serde_yaml::Value::as_sequence) {
                let mut lines = Vec::new();
                for item in payload {
                    match item {
                        serde_yaml::Value::String(s) => lines.push(s.clone()),
                        other => {
                            if let Ok(s) = serde_yaml::to_string(other) {
                                lines.push(s.trim_end().to_string());
                            }
                        }
                    }
                }
                return lines;
            }
        }
    }
    body.lines().map(str::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn domain_of(json_text: &str, key: &str) -> Vec<String> {
        let value: serde_json::Value = serde_json::from_str(json_text).unwrap();
        value["rules"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r.get(key))
            .flat_map(|v| v.as_array().unwrap().iter())
            .map(|v| v.as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn converts_domain_behavior_and_skips_geo() {
        let body = "DOMAIN,example.com\nDOMAIN-SUFFIX,google.com\nGEOSITE,cn\nDOMAIN-KEYWORD,ads\n";
        let out = convert_clash_ruleset(body).unwrap();
        assert_eq!(out.rule_count, 3);
        assert_eq!(out.skipped_geo, 1);
        assert_eq!(domain_of(&out.json, "domain"), vec!["example.com"]);
        assert_eq!(domain_of(&out.json, "domain_suffix"), vec!["google.com"]);
        assert_eq!(domain_of(&out.json, "domain_keyword"), vec!["ads"]);
    }

    #[test]
    fn converts_ipcidr_behavior() {
        let body = "IP-CIDR,1.2.3.0/24\nIP-CIDR6,2001:db8::/32\nGEOIP,cn\n";
        let out = convert_clash_ruleset(body).unwrap();
        assert_eq!(out.rule_count, 2);
        assert_eq!(out.skipped_geo, 1);
        assert_eq!(
            domain_of(&out.json, "ip_cidr"),
            vec!["1.2.3.0/24", "2001:db8::/32"]
        );
    }

    #[test]
    fn converts_classical_ignoring_target_column() {
        let body = "DOMAIN-SUFFIX,openai.com,PROXY\nIP-CIDR,10.0.0.0/8,DIRECT,no-resolve\n";
        let out = convert_clash_ruleset(body).unwrap();
        assert_eq!(out.rule_count, 2);
        assert_eq!(domain_of(&out.json, "domain_suffix"), vec!["openai.com"]);
        assert_eq!(domain_of(&out.json, "ip_cidr"), vec!["10.0.0.0/8"]);
    }

    #[test]
    fn normalizes_yaml_payload_wrapper() {
        let body = "payload:\n  - DOMAIN-SUFFIX,a.com\n  - DOMAIN-SUFFIX,b.com\n";
        let out = convert_clash_ruleset(body).unwrap();
        assert_eq!(out.rule_count, 2);
        assert_eq!(
            domain_of(&out.json, "domain_suffix"),
            vec!["a.com", "b.com"]
        );
    }

    #[test]
    fn rejects_geo_only_and_empty() {
        assert!(convert_clash_ruleset("GEOSITE,cn\nGEOIP,cn\n").is_err());
        assert!(convert_clash_ruleset("").is_err());
        assert!(convert_clash_ruleset("# only a comment\n").is_err());
    }

    #[test]
    fn detector_distinguishes_clash_text_from_json() {
        assert!(looks_like_clash_ruleset("DOMAIN-SUFFIX,google.com\n"));
        assert!(looks_like_clash_ruleset("IP-CIDR,1.2.3.0/24\n"));
        assert!(!looks_like_clash_ruleset("{\"version\":3,\"rules\":[]}"));
        assert!(!looks_like_clash_ruleset(""));
    }
}
