//! MCP 传输层：绑定 127.0.0.1 的最小 HTTP server，仅响应 `POST /mcp`。
//!
//! 镜像 [`crate::pac::server::PacServer`] 的手写 HTTP/1.1 模式（零 HTTP
//! 框架依赖），但支持 POST + Content-Length body。MCP Streamable HTTP
//! 规范允许 server 对请求返回 `application/json` 单个响应（无需 SSE）；
//! 本 server 无服务器主动推送需求，故不实现 SSE 与会话（无状态）。
//!
//! 每个连接处理单个请求后 `Connection: close`，无 keep-alive 状态机。
//! RPC 分发逻辑抽为 [`RpcHandler`]，使传输层可独立于 Tauri 单测。

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;

use super::rpc::{self, ParsedRequest, RpcRequest, RpcResponse};

const MCP_PATH: &str = "/mcp";
const MAX_HEADER_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// 异步 RPC 处理站点：给定 JSON-RPC 请求，产出响应。
/// 生产实现见 [`super::real_handler`]（含 AppHandle/tools/call 分发）。
pub type RpcHandler =
    Arc<dyn Fn(RpcRequest) -> Pin<Box<dyn Future<Output = RpcResponse> + Send>> + Send + Sync>;

/// 本地 MCP 服务句柄。创建即占用端口，`stop` 后服务停止。
pub struct McpServer {
    port: u16,
    stop_tx: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl McpServer {
    /// 绑定 `127.0.0.1:port` 并启动 accept 循环，所有 RPC 经 `handler` 分发。
    pub async fn start(port: u16, handler: RpcHandler) -> Result<Self, String> {
        let addr: SocketAddr = format!("127.0.0.1:{port}")
            .parse()
            .map_err(|e| format!("invalid mcp bind address: {e}"))?;
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|e| format!("bind mcp server {addr}: {e}"))?;

        let (stop_tx, mut stop_rx) = watch::channel(false);
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    changed = stop_rx.changed() => {
                        let _ = changed;
                        break;
                    }
                    accepted = listener.accept() => {
                        match accepted {
                            Ok((mut stream, _peer)) => {
                                let handler = handler.clone();
                                tokio::spawn(async move {
                                    let _ = handle_connection(&mut stream, handler).await;
                                });
                            }
                            Err(_) => break,
                        }
                    }
                }
            }
        });

        Ok(McpServer {
            port,
            stop_tx,
            task,
        })
    }

    /// 对外暴露的 MCP 端点 URL（配置到客户端 mcp servers 里）。
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{port}{MCP_PATH}", port = self.port)
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// 停止服务并等待 accept 循环退出。
    pub async fn stop(self) {
        let _ = self.stop_tx.send(true);
        let _ = self.task.await;
    }
}

/// 处理单个连接：解析一个 HTTP/1.1 请求 → 分发 → 单个响应 → close。
async fn handle_connection(stream: &mut TcpStream, handler: RpcHandler) -> std::io::Result<()> {
    stream.set_nodelay(true).ok();

    // —— 请求行 + headers（上限保护）——
    let mut reader = BufReader::new(stream);
    let (method, path, content_length) = match read_request_head(&mut reader).await {
        Ok(head) => head,
        Err(HeadError::Io(e)) => return Err(e),
        Err(HeadError::BadRequest) | Err(HeadError::TooLarge) => {
            return write_simple(&mut reader, 400).await;
        }
    };

    // 路由：仅 POST /mcp。GET → 405（不支持 SSE GET 流），其余 → 404。
    if path != MCP_PATH {
        return write_simple(&mut reader, 404).await;
    }
    if method != "POST" {
        return write_simple(&mut reader, 405).await;
    }

    // —— body ——
    let content_length = content_length.unwrap_or(0);
    if content_length > MAX_BODY_BYTES {
        return write_simple(&mut reader, 413).await;
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body).await?;
    }

    // —— JSON-RPC 解析与分发 ——
    let (status, payload): (&str, Option<String>) = match rpc::parse_request(&body) {
        ParsedRequest::ParseError(msg) => (
            "200 OK",
            Some(
                serde_json::to_string(&RpcResponse::error(
                    None,
                    rpc::ERR_PARSE,
                    format!("Parse error: {msg}"),
                ))
                .unwrap_or_default(),
            ),
        ),
        ParsedRequest::Batch => (
            "200 OK",
            Some(
                serde_json::to_string(&RpcResponse::error(
                    None,
                    rpc::ERR_INVALID_REQUEST,
                    "Batch requests are not supported",
                ))
                .unwrap_or_default(),
            ),
        ),
        ParsedRequest::Single(req) => {
            if req.is_notification() {
                // notification：无响应体（HTTP 202 Accepted）。
                ("202 Accepted", None)
            } else {
                let response = (handler)(req).await;
                (
                    "200 OK",
                    Some(serde_json::to_string(&response).unwrap_or_default()),
                )
            }
        }
    };

    write_response(&mut reader, status, payload.as_deref()).await
}

enum HeadError {
    Io(std::io::Error),
    BadRequest,
    TooLarge,
}

/// 读请求行与 headers，返回 (method, path, content-length)。
async fn read_request_head(
    reader: &mut BufReader<&mut TcpStream>,
) -> Result<(String, String, Option<usize>), HeadError> {
    let mut total = 0usize;
    let mut method = String::new();
    let mut path = String::new();
    let mut content_length: Option<usize> = None;

    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await.map_err(HeadError::Io)?;
        total += n;
        if total > MAX_HEADER_BYTES {
            return Err(HeadError::TooLarge);
        }
        if n == 0 {
            return Err(HeadError::BadRequest);
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            // headers 结束
            if method.is_empty() {
                return Err(HeadError::BadRequest);
            }
            return Ok((method, path, content_length));
        }
        if method.is_empty() {
            // 请求行：METHOD SP PATH SP VERSION
            let mut parts = trimmed.split_whitespace();
            method = parts.next().unwrap_or("").to_ascii_uppercase();
            path = parts.next().unwrap_or("").to_string();
            if method.is_empty() || path.is_empty() {
                return Err(HeadError::BadRequest);
            }
        } else if let Some((name, value)) = trimmed.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse::<usize>().ok();
            }
        }
        // 慢速/恶意客户端最多占住一个已 spawn 的连接 task，不影响 accept。
    }
}

async fn write_simple(stream: &mut BufReader<&mut TcpStream>, status: u16) -> std::io::Result<()> {
    let reason = match status {
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        _ => "Error",
    };
    write_response(stream, &format!("{status} {reason}"), None).await
}

async fn write_response(
    stream: &mut BufReader<&mut TcpStream>,
    status: &str,
    body: Option<&str>,
) -> std::io::Result<()> {
    let (head, payload) = match body {
        Some(text) => {
            let bytes = text.as_bytes();
            (
                format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    bytes.len()
                ),
                Some(bytes),
            )
        }
        None => (
            format!(
                "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            ),
            None,
        ),
    };
    stream.write_all(head.as_bytes()).await?;
    if let Some(bytes) = payload {
        stream.write_all(bytes).await?;
    }
    stream.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn mock_handler(respond: impl Fn(RpcRequest) -> RpcResponse + Send + Sync + 'static) -> RpcHandler {
        Arc::new(move |req| {
            let resp = respond(req);
            Box::pin(async move { resp })
        })
    }

    async fn post(server: &McpServer, body: &str) -> reqwest::Response {
        reqwest::Client::new()
            .post(server.url())
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .expect("post")
    }

    #[tokio::test]
    async fn initialize_roundtrip_via_post() {
        let handler = mock_handler(|req| {
            assert_eq!(req.method, "initialize");
            RpcResponse::success(req.id, json!({"protocolVersion": "2025-06-18"}))
        });
        let server = McpServer::start(27990, handler).await.expect("start");
        let resp = post(&server, r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#).await;
        assert_eq!(resp.status(), 200);
        let v: serde_json::Value = resp.json().await.expect("json");
        assert_eq!(v["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(v["id"], 1);
        server.stop().await;
    }

    #[tokio::test]
    async fn notification_gets_202_empty_body() {
        let handler = mock_handler(|_| unreachable!("notification must not reach handler"));
        let server = McpServer::start(27989, handler).await.expect("start");
        let resp = post(
            &server,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        )
        .await;
        assert_eq!(resp.status(), 202);
        assert_eq!(resp.text().await.expect("body"), "");
        server.stop().await;
    }

    #[tokio::test]
    async fn get_returns_405_and_unknown_path_404() {
        let handler = mock_handler(|_| unreachable!());
        let server = McpServer::start(27988, handler).await.expect("start");
        let base = "http://127.0.0.1:27988";
        let get = reqwest::get(format!("{base}/mcp")).await.expect("get");
        assert_eq!(get.status(), 405);
        let other = reqwest::get(format!("{base}/nope")).await.expect("404");
        assert_eq!(other.status(), 404);
        server.stop().await;
    }

    #[tokio::test]
    async fn malformed_json_yields_parse_error_response() {
        let handler = mock_handler(|_| unreachable!());
        let server = McpServer::start(27987, handler).await.expect("start");
        let resp = post(&server, "{not json").await;
        assert_eq!(resp.status(), 200);
        let v: serde_json::Value = resp.json().await.expect("json");
        assert_eq!(v["error"]["code"], rpc::ERR_PARSE);
        server.stop().await;
    }

    #[tokio::test]
    async fn batch_request_rejected() {
        let handler = mock_handler(|_| unreachable!());
        let server = McpServer::start(27986, handler).await.expect("start");
        let resp = post(&server, r#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#).await;
        let v: serde_json::Value = resp.json().await.expect("json");
        assert_eq!(v["error"]["code"], rpc::ERR_INVALID_REQUEST);
        server.stop().await;
    }

    #[tokio::test]
    async fn bind_conflict_returns_error() {
        let handler = mock_handler(|_| unreachable!());
        let a = McpServer::start(27985, handler.clone()).await.expect("first");
        let b = McpServer::start(27985, handler).await;
        assert!(b.is_err(), "same-port bind must fail");
        a.stop().await;
    }

    #[tokio::test]
    async fn url_shape_is_correct() {
        let handler = mock_handler(|_| unreachable!());
        let server = McpServer::start(27984, handler).await.expect("start");
        assert_eq!(server.url(), "http://127.0.0.1:27984/mcp");
        assert_eq!(server.port(), 27984);
        server.stop().await;
    }
}
