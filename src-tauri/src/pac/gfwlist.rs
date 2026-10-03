//! gfwlist 解析与拉取。
//!
//! 纯函数 [`parse_gfwlist_text`] 把 gfwlist 规则文本转成域名列表；
//! [`fetch_gfwlist`] 负责远程拉取（gfwlist 本体是 base64 编码）并复用解析。

use base64::{
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
    Engine as _,
};
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

/// 解码 gfwlist 响应体（base64 → 规则文本）。
///
/// 上游 `gfwlist.txt` 是 base64 后再**每 64 字符插入换行**，直接
/// `STANDARD.decode` 会在第 64 字符处报 `Invalid symbol 10`（`\n`）。
/// 这里先折叠所有空白再解码，并依次容忍 URL-safe 字母表与缺失 padding。
/// 部分镜像直接返回明文规则，检测到非 base64 字符时按明文透传。
pub fn decode_gfwlist_body(raw: &str) -> Result<String, String> {
    let body = raw.trim_start_matches('\u{feff}').trim();
    if body.is_empty() {
        return Err("gfwlist 内容为空".into());
    }
    // 镜像 404 / 风控拦截会返回 HTML，给出可读提示而不是 base64 噪音。
    if body.starts_with('<') {
        return Err("gfwlist 地址返回了 HTML 页面（地址失效或被拦截）".into());
    }
    if !looks_like_base64(body) {
        return Ok(body.to_string());
    }

    let compact: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    let unpadded = compact.trim_end_matches('=');
    let decoded = STANDARD
        .decode(&compact)
        .or_else(|_| STANDARD_NO_PAD.decode(unpadded))
        .or_else(|_| URL_SAFE.decode(&compact))
        .or_else(|_| URL_SAFE_NO_PAD.decode(unpadded))
        .map_err(|error| format!("base64 decode gfwlist: {error}"))?;

    // 上游是 UTF-8；出现脏字节时用 lossy 兜底，避免整份名单拉取失败。
    Ok(String::from_utf8_lossy(&decoded).into_owned())
}

/// 粗略判断一段文本是否全由 base64 字母表组成。
///
/// 规则文本必然含 `|` / `!` / `@` / `.` / `*` 等字符，所以只要出现一个
/// 非字母表字符就判定为明文；只看前 4KB 足够，避免大文件全量扫描。
fn looks_like_base64(s: &str) -> bool {
    let mut checked = 0usize;
    for c in s.chars() {
        if c.is_whitespace() {
            continue;
        }
        if !(c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=' || c == '-' || c == '_') {
            return false;
        }
        checked += 1;
        if checked > 4096 {
            break;
        }
    }
    checked > 0
}

/// 拉取 gfwlist 并解析为域名列表。
///
/// 出错返回 `Err(String)`，含网络 / 解码错误信息。
pub async fn fetch_gfwlist(url: &str) -> Result<Vec<String>, String> {
    let body = reqwest::get(url)
        .await
        .map_err(|error| format!("fetch gfwlist: {error}"))?
        .text()
        .await
        .map_err(|error| format!("read gfwlist body: {error}"))?;

    Ok(parse_gfwlist_text(&decode_gfwlist_body(&body)?))
}

/// 拉取纯文本列表（每行一条，如 chnroute 的 IP 段 / 中国域名列表）。
///
/// 与 gfwlist 的 adblock 语法不同，这里只做「去空行、去注释（#/!）、trim」。
pub async fn fetch_plain_list(url: &str) -> Result<Vec<String>, String> {
    let body = reqwest::get(url)
        .await
        .map_err(|error| format!("fetch list: {error}"))?
        .text()
        .await
        .map_err(|error| format!("read list body: {error}"))?;

    Ok(body
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty() && !line.starts_with('#') && !line.starts_with('!')
        })
        .map(str::to_string)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 上游 gfwlist.txt 的真实形态：base64 后每 64 字符换行。
    fn encode_wrapped(text: &str, width: usize) -> String {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let encoded = STANDARD.encode(text.as_bytes());
        encoded
            .as_bytes()
            .chunks(width)
            .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn decodes_base64_wrapped_at_64_columns() {
        // 回归：换行符曾让 STANDARD.decode 报 `Invalid symbol 10, offset 64`。
        // 用 8 列折行以在小 fixture 上复现同样的「换行夹在 base64 中」形态。
        let rules = "!comment\n||google.com\n||youtube.com\n";
        let body = encode_wrapped(rules, 8);
        assert!(body.contains('\n'), "fixture 必须包含换行才有效");
        // 真实上游按 64 列折行；这里再确认 64 列同样可解。
        assert!(encode_wrapped(&rules.repeat(8), 64).contains('\n'));

        let decoded = decode_gfwlist_body(&body).unwrap();
        assert_eq!(decoded, rules);
        assert_eq!(
            parse_gfwlist_text(&decoded),
            vec!["google.com", "youtube.com"]
        );
    }

    #[test]
    fn decodes_real_upstream_shape_dropping_the_error() {
        // 逐字节复刻报错现场：base64 第 64 个字符处是 `\n`。
        let rules = "||google.com\n".repeat(12);
        let body = encode_wrapped(&rules, 64);
        assert_eq!(body.as_bytes()[64], b'\n', "第 64 字符应为换行");
        assert_eq!(decode_gfwlist_body(&body).unwrap(), rules);
    }

    #[test]
    fn decodes_crlf_and_surrounding_whitespace() {
        let rules = "||example.org\n";
        let body = format!("\r\n  {}\r\n", encode_wrapped(rules, 20).replace('\n', "\r\n"));
        assert_eq!(decode_gfwlist_body(&body).unwrap(), rules);
    }

    #[test]
    fn decodes_unpadded_and_url_safe_variants() {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        // 去掉 padding 的 base64。
        let rules = "||a.com\n||b.com\n";
        let encoded = STANDARD.encode(rules.as_bytes());
        let unpadded = encoded.trim_end_matches('=');
        assert_eq!(decode_gfwlist_body(unpadded).unwrap(), rules);

        // URL-safe 字母表（`+` → `-`、`/` → `_`）。
        let url_safe = encoded.replace('+', "-").replace('/', "_");
        assert_eq!(decode_gfwlist_body(&url_safe).unwrap(), rules);
    }

    #[test]
    fn passes_through_plaintext_and_strips_bom() {
        // 部分镜像直接给明文规则，不该当成 base64 失败。
        let plain = "||plain.example.com\n";
        let decoded = decode_gfwlist_body(plain).unwrap();
        assert_eq!(decoded.trim(), plain.trim());
        let with_bom = decode_gfwlist_body(&format!("\u{feff}{plain}")).unwrap();
        assert_eq!(with_bom.trim(), plain.trim());
    }

    #[test]
    fn rejects_empty_and_html_bodies() {
        assert!(decode_gfwlist_body("   \n").is_err());
        let html = decode_gfwlist_body("<!DOCTYPE html><html>404</html>").unwrap_err();
        assert!(html.contains("HTML"), "错误信息应点明返回了 HTML：{html}");
    }

    #[test]
    fn parses_double_pipe_domains() {
        let result = parse_gfwlist_text("||google.com\n||youtube.com\n");
        assert_eq!(result, vec!["google.com", "youtube.com"]);
    }

    /// 真实网络拉取（`cargo test -- --ignored` 手动跑）。
    ///
    /// 覆盖上游「base64 + 64 列折行」的真实形态——单测 fixture 只能模拟，
    /// 这里确认端到端能拿到上千条域名。
    #[test]
    #[ignore = "requires network"]
    fn fetches_real_upstream_gfwlist() {
        let domains = tauri_block_on(async {
            fetch_gfwlist("https://raw.githubusercontent.com/gfwlist/gfwlist/master/gfwlist.txt")
                .await
        })
        .expect("fetch gfwlist from upstream");
        assert!(
            domains.len() > 1000,
            "应按域名解析出上千条，实际 {}",
            domains.len()
        );
        assert!(domains.contains(&"google.com".to_string()));
        assert!(domains.iter().all(|d| !d.contains(char::is_whitespace)));
    }

    /// 在非 tokio worker 线程上跑 async（复用应用里的 block_on 方式）。
    fn tauri_block_on<F: std::future::Future>(future: F) -> F::Output {
        tauri::async_runtime::block_on(future)
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
