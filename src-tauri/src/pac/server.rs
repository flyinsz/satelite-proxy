//! 本地 PAC 脚本托管服务。
//!
//! 一个绑定 `127.0.0.1` 的最小 HTTP 服务，只响应 `GET /proxy.pac`，
//! 返回 `application/x-ns-proxy-autoconfig`。系统代理的 PAC 模式
//! 指向 [`PacServer::url`]。

use std::net::SocketAddr;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;

const PAC_PATH: &str = "/proxy.pac";
const CONTENT_TYPE: &str = "application/x-ns-proxy-autoconfig";

/// 本地 PAC 服务句柄。
///
/// 创建即占用端口，析构 / `stop` 后服务停止。同一端口同一时刻只能有一个实例。
pub struct PacServer {
    port: u16,
    stop_tx: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl PacServer {
    /// 绑定 `127.0.0.1:port` 并启动服务循环。
    ///
    /// 端口被占用等绑定失败返回 `Err(String)`。成功返回后服务已在监听，
    /// `url()` 即可被系统代理使用。
    pub async fn start(port: u16, content: String) -> Result<Self, String> {
        let addr: SocketAddr = format!("127.0.0.1:{port}")
            .parse()
            .map_err(|error| format!("invalid pac bind address: {error}"))?;
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|error| format!("bind pac server {addr}: {error}"))?;

        let (stop_tx, mut stop_rx) = watch::channel(false);
        let task = tokio::spawn(async move {
            // 断开所有存量连接的状态由 accept 循环自行处理：
            // 每个连接独立读写，不共享状态。
            loop {
                tokio::select! {
                    changed = stop_rx.changed() => {
                        // 发送端被 drop 或信号置 true —— 均表示停止。
                        let _ = changed;
                        break;
                    }
                    accepted = listener.accept() => {
                        match accepted {
                            Ok((mut stream, _peer)) => {
                                let content = content.clone();
                                tokio::spawn(async move {
                                    if let Err(error) = handle_connection(&mut stream, &content).await {
                                        let _ = error;
                                    }
                                });
                            }
                            // 监听器被关闭（理论上只发生在 task 退出时）。
                            Err(_) => break,
                        }
                    }
                }
            }
        });

        Ok(PacServer {
            port,
            stop_tx,
            task,
        })
    }

    /// 该服务对外暴露的 PAC 脚本 URL。
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/proxy.pac", self.port)
    }

    /// 停止服务并等待 accept 循环退出。
    pub async fn stop(self) {
        let _ = self.stop_tx.send(true);
        let _ = self.task.await;
    }
}

/// 处理单个连接：读请求行，若为 `GET /proxy.pac` 则返回 PAC 内容，否则 404。
async fn handle_connection(stream: &mut TcpStream, content: &str) -> std::io::Result<()> {
    // 逐行读取请求行（TCP 分包时 read_line 会等完整的一行）。
    let mut reader = BufReader::new(stream);
    let mut request_line = String::new();
    let n = reader.read_line(&mut request_line).await?;
    if n == 0 {
        // 客户端未发任何数据就断开。
        return Ok(());
    }

    let is_pac = request_line
        .split_whitespace()
        .nth(1)
        .is_some_and(|path| path == PAC_PATH);

    if is_pac {
        let body = content.as_bytes();
        let header = format!(
            "HTTP/1.1 200 OK\r\n\
             Content-Type: {CONTENT_TYPE}\r\n\
             Content-Length: {}\r\n\
             Connection: close\r\n\
             \r\n",
            body.len()
        );
        reader.write_all(header.as_bytes()).await?;
        reader.write_all(body).await?;
    } else {
        let not_found = "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        reader.write_all(not_found.as_bytes()).await?;
    }
    reader.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn serves_pac_script_and_404s_other_paths() {
        let content = "function FindProxyForURL(url, host) { return \"DIRECT\"; }\n";
        // 测试用高位端口，避免与真实运行的 pac_port 冲突。
        let server = PacServer::start(2099, content.to_string())
            .await
            .expect("start pac server");

        // GET /proxy.pac → 200 + 正确的 Content-Type + 完整 body。
        let resp = reqwest::get(server.url()).await.expect("request proxy.pac");
        assert_eq!(resp.status(), 200);
        assert_eq!(
            resp.headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok()),
            Some("application/x-ns-proxy-autoconfig")
        );
        let body = resp.text().await.expect("read body");
        assert_eq!(body, content);

        // 其他路径 → 404。
        let not_found = reqwest::get(format!("http://127.0.0.1:{}/nope", 2099))
            .await
            .expect("request unknown path");
        assert_eq!(not_found.status(), 404);

        server.stop().await;
    }

    #[tokio::test]
    async fn bind_conflict_returns_error() {
        let server = PacServer::start(2098, "x".into()).await.expect("first bind");
        let second = PacServer::start(2098, "y".into()).await;
        assert!(second.is_err(), "same-port bind must fail");
        server.stop().await;
    }
}
