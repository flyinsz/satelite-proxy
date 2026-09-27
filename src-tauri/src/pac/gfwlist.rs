//! gfwlist 解析与拉取。
//!
//! 纯函数 [`parse_gfwlist_text`] 把 gfwlist 规则文本转成域名列表；
//! [`fetch_gfwlist`] 负责远程拉取（gfwlist 本体是 base64 编码）并复用解析。

use base64::{engine::general_purpose::STANDARD, Engine as _};
use std::collections::HashSet;

/// 解析 gfwlist 规则文本，提取域名列表。
///
/// 规则优先级：
/// - `@@...` 白名单 → 跳过，不加入结果；
/// - `||domain` → 取 `domain` 部分（去掉 `||`、路径 `/...`、`^` 等尾部标记）；
/// - `|http(s)://host/...` → 取 URL 的 host；
/// - `.domain`（前导点）→ 去掉前导点；
/// - 其他含通配 / 正则的复杂行 → 尽力提取域名，失败则跳过。
///
/// 结果保持输入顺序、自动去重。
pub fn parse_gfwlist_text(text: &str) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        // 注释 / 白名单直接跳过。
        if line.starts_with('!') || line.starts_with("@@") {
            continue;
        }

        if let Some(domain) = extract_domain(line) {
            if seen.insert(domain.clone()) {
                out.push(domain);
            }
        }
    }
    out
}

/// 从单条规则行里尽力提取一个裸域名（不含端口 / 路径）。
///
/// 返回 `None` 表示该行无法解析（跳过）。
fn extract_domain(line: &str) -> Option<String> {
    // 去掉通配符标记后的候选串。
    let candidate = if let Some(rest) = line.strip_prefix("||") {
        rest
    } else if let Some(rest) = line.strip_prefix('|') {
        // |http://host/... 形式 —— 剥掉协议。
        rest.strip_prefix("http://")
            .or_else(|| rest.strip_prefix("https://"))?
    } else if line.starts_with('.') {
        // .domain 形式 —— 前导点会在最终整理时去掉。
        line
    } else {
        line
    };

    // 去掉从第一个「非域名字符」开始的尾部内容。
    let end = candidate
        .find(|c: char| {
            !(c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_')
        })
        .unwrap_or(candidate.len());
    let head = &candidate[..end];

    // 去掉尾部多余的点和保留的首部点。
    let trimmed = head.trim_end_matches('.');
    let domain = trimmed.trim_start_matches('.');

    if is_plausible_domain(domain) {
        Some(domain.to_string())
    } else {
        None
    }
}

/// 粗略判定候选串像一个域名（至少两段、以字母结尾，且不含明显非域名内容）。
fn is_plausible_domain(s: &str) -> bool {
    if s.is_empty() || s.len() > 253 {
        return false;
    }
    if !s.contains('.') {
        return false;
    }
    // 去掉通配符残留（例：`*.google.com` 里的 `*` 已由外层处理，这里兜底）。
    if s.contains('*') || s.contains('^') || s.contains('/') || s.contains(':') {
        return false;
    }
    // 末段至少一个字母（避免 `1.2.3` 这类被当作域名）。
    let last_label = s.rsplit('.').next().unwrap_or("");
    last_label.bytes().any(|b| b.is_ascii_alphabetic())
}

/// 拉取 gfwlist（base64 编码）并解析为域名列表。
///
/// 出错返回 `Err(String)`，含网络 / 解码错误信息。
pub async fn fetch_gfwlist(url: &str) -> Result<Vec<String>, String> {
    let body = reqwest::get(url)
        .await
        .map_err(|error| format!("fetch gfwlist: {error}"))?
        .text()
        .await
        .map_err(|error| format!("read gfwlist body: {error}"))?;

    let decoded = STANDARD
        .decode(body.trim())
        .map_err(|error| format!("base64 decode gfwlist: {error}"))?;

    let text = String::from_utf8(decoded)
        .map_err(|error| format!("gfwlist is not valid utf-8: {error}"))?;

    Ok(parse_gfwlist_text(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_double_pipe_domains() {
        let result = parse_gfwlist_text("||google.com\n||youtube.com\n");
        assert_eq!(result, vec!["google.com", "youtube.com"]);
    }

    #[test]
    fn parses_absolute_url_rules() {
        let result = parse_gfwlist_text(
            "|https://example.com/x\n|http://foo.example.org/path?q=1\n",
        );
        assert_eq!(result, vec!["example.com", "foo.example.org"]);
    }

    #[test]
    fn parses_dot_prefixed_domains() {
        let result = parse_gfwlist_text(".foo.com\n..bar.com\n");
        assert_eq!(result, vec!["foo.com", "bar.com"]);
    }

    #[test]
    fn skips_whitelist_and_comments() {
        let result = parse_gfwlist_text(
            "@@||bar.com\n@@|https://allow.example.org\n! a comment\n||keep.com\n",
        );
        assert_eq!(result, vec!["keep.com"]);
    }

    #[test]
    fn strips_paths_and_markers() {
        let result = parse_gfwlist_text("||google.com/video/foo^\n||example.org^\n");
        assert_eq!(result, vec!["google.com", "example.org"]);
    }

    #[test]
    fn deduplicates_while_keeping_order() {
        let result = parse_gfwlist_text("||a.com\n||b.com\n||a.com\n");
        assert_eq!(result, vec!["a.com", "b.com"]);
    }

    #[test]
    fn drops_unparseable_lines() {
        let result = parse_gfwlist_text(
            "some regex /^https?:\\/\\/example\\.com.*/\n||good.com\n",
        );
        assert_eq!(result, vec!["good.com"]);
    }
}
