//! A tiny local proxy that all of the agent browser's traffic goes through,
//! so the same public-addresses-only rule holds for everything a page loads:
//! redirects, scripts, images, fetches and WebSockets included.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use url::Url;

use super::guard::Policy;

const MAX_HEAD: usize = 64_000;

/// Stops listening when dropped.
pub struct Proxy {
    pub port: u16,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub async fn start(policy: Policy) -> Result<Proxy, String> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let task = tokio::spawn(async move {
        loop {
            let Ok((conn, _)) = listener.accept().await else { break };
            let policy = policy.clone();
            tokio::spawn(async move {
                let _ = serve(conn, policy).await;
            });
        }
    });
    Ok(Proxy { port, task })
}

async fn connect(policy: &Policy, host: &str, port: u16) -> Option<TcpStream> {
    for addr in policy.resolve(host, port).await.ok()? {
        if let Ok(Ok(s)) = tokio::time::timeout(std::time::Duration::from_secs(10), TcpStream::connect(addr)).await {
            return Some(s);
        }
    }
    None
}

async fn refuse(mut conn: TcpStream) -> std::io::Result<()> {
    conn.write_all(b"HTTP/1.1 403 Forbidden\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await
}

async fn serve(mut conn: TcpStream, policy: Policy) -> std::io::Result<()> {
    let mut head = Vec::new();
    let mut buf = [0u8; 4096];
    let end = loop {
        let n = conn.read(&mut buf).await?;
        if n == 0 || head.len() > MAX_HEAD {
            return Ok(());
        }
        head.extend_from_slice(&buf[..n]);
        if let Some(i) = head.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let text = String::from_utf8_lossy(&head[..end]).into_owned();
    let mut lines = text.split("\r\n");
    let request = lines.next().unwrap_or("");
    let mut it = request.split(' ');
    let (method, target) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
    if method == "CONNECT" {
        let Some((host, port)) = target.rsplit_once(':').and_then(|(h, p)| Some((h, p.parse::<u16>().ok()?))) else { return refuse(conn).await };
        let Some(mut up) = connect(&policy, host, port).await else { return refuse(conn).await };
        conn.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await?;
        up.write_all(&head[end..]).await?;
        tokio::io::copy_bidirectional(&mut conn, &mut up).await?;
        return Ok(());
    }
    // Plain http: "GET http://host/path HTTP/1.1".
    let Ok(url) = Url::parse(target) else { return refuse(conn).await };
    if url.scheme() != "http" {
        return refuse(conn).await;
    }
    let Some(host) = url.host_str() else { return refuse(conn).await };
    let Some(mut up) = connect(&policy, host, url.port_or_known_default().unwrap_or(80)).await else { return refuse(conn).await };
    let path = &url[url::Position::BeforePath..url::Position::AfterQuery];
    // One request per connection: a second one on the same socket could
    // name a different (unchecked) server.
    let upgrade = lines.clone().any(|l| l.to_ascii_lowercase().starts_with("upgrade:"));
    let mut forward = format!("{method} {path} HTTP/1.1\r\n");
    for l in lines.filter(|l| !l.is_empty()) {
        let low = l.to_ascii_lowercase();
        if low.starts_with("proxy-") || (!upgrade && (low.starts_with("connection:") || low.starts_with("keep-alive:"))) {
            continue;
        }
        forward.push_str(l);
        forward.push_str("\r\n");
    }
    if !upgrade {
        forward.push_str("Connection: close\r\n");
    }
    forward.push_str("\r\n");
    up.write_all(forward.as_bytes()).await?;
    up.write_all(&head[end..]).await?;
    tokio::io::copy_bidirectional(&mut conn, &mut up).await?;
    Ok(())
}
