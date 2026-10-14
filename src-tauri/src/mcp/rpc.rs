//! MCP JSON-RPC 2.0 编解码。
//!
//! 仅覆盖 Satelite MCP server 需要的子集：请求（带/不带 id 的
//! notification）、成功响应、错误响应与标准错误码。MCP 的消息体就是
//! JSON-RPC 2.0，`id` 缺失即 notification（不应产生响应）。

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const ERR_PARSE: i64 = -32700;
pub const ERR_INVALID_REQUEST: i64 = -32600;
pub const ERR_METHOD_NOT_FOUND: i64 = -32601;
pub const ERR_INVALID_PARAMS: i64 = -32602;
pub const ERR_INTERNAL: i64 = -32603;

/// 入站 JSON-RPC 请求。`id: None` ⇒ notification。
#[derive(Debug, Deserialize)]
pub struct RpcRequest {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

impl RpcRequest {
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

/// 出站 JSON-RPC 响应。`result` 与 `error` 二选一。
#[derive(Debug, Serialize)]
pub struct RpcResponse {
    pub jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl RpcResponse {
    pub fn success(id: Option<Value>, result: Value) -> Self {
        RpcResponse {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Option<Value>, code: i64, message: impl Into<String>) -> Self {
        RpcResponse {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(RpcError {
                code,
                message: message.into(),
            }),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

/// 解析一段 body 为单个请求。批量请求（JSON array）本 server 不支持
/// （MCP 客户端单发单收），返回 `ERR_INVALID_REQUEST` 由调用方处理。
pub enum ParsedRequest {
    Single(RpcRequest),
    ParseError(String),
    Batch,
}

pub fn parse_request(body: &[u8]) -> ParsedRequest {
    let text = match std::str::from_utf8(body) {
        Ok(t) => t,
        Err(e) => return ParsedRequest::ParseError(format!("body is not utf-8: {e}")),
    };
    let trimmed = text.trim_start();
    if trimmed.starts_with('[') {
        return ParsedRequest::Batch;
    }
    match serde_json::from_str::<RpcRequest>(text) {
        Ok(req) => ParsedRequest::Single(req),
        Err(e) => ParsedRequest::ParseError(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_request_with_id() {
        let body = br#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#;
        match parse_request(body) {
            ParsedRequest::Single(req) => {
                assert_eq!(req.method, "tools/list");
                assert!(!req.is_notification());
                assert_eq!(req.id, Some(json!(1)));
            }
            _ => panic!("expected single request"),
        }
    }

    #[test]
    fn parses_notification_missing_id() {
        let body = br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        match parse_request(body) {
            ParsedRequest::Single(req) => assert!(req.is_notification()),
            _ => panic!("expected single request"),
        }
    }

    #[test]
    fn params_default_to_null_when_absent() {
        let body = br#"{"jsonrpc":"2.0","id":7,"method":"ping"}"#;
        match parse_request(body) {
            ParsedRequest::Single(req) => assert!(req.params.is_null()),
            _ => panic!("expected single request"),
        }
    }

    #[test]
    fn batch_is_detected_and_rejected() {
        let body = br#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#;
        assert!(matches!(parse_request(body), ParsedRequest::Batch));
    }

    #[test]
    fn malformed_json_yields_parse_error() {
        let body = b"{not json";
        match parse_request(body) {
            ParsedRequest::ParseError(_) => {}
            _ => panic!("expected parse error"),
        }
    }

    #[test]
    fn success_response_serializes_without_nulls() {
        let resp = RpcResponse::success(Some(json!(1)), json!({}));
        let text = serde_json::to_string(&resp).unwrap();
        assert_eq!(text, r#"{"jsonrpc":"2.0","id":1,"result":{}}"#);
    }

    #[test]
    fn error_response_carries_code_and_message() {
        let resp = RpcResponse::error(Some(json!(1)), ERR_METHOD_NOT_FOUND, "nope");
        let text = serde_json::to_string(&resp).unwrap();
        assert!(text.contains(r#""code":-32601"#));
        assert!(text.contains(r#""message":"nope""#));
        assert!(!text.contains("result"));
    }

    #[test]
    fn notification_response_has_null_id() {
        // notification 永不回 RPC error —— 服务端直接 202，这里仅验证
        // 序列化时 id=None 能正确省略。
        let resp = RpcResponse::success(None, json!({}));
        let text = serde_json::to_string(&resp).unwrap();
        assert!(!text.contains("id"));
    }
}
