//! Local dev-server proxy for the live preview (`blazeweb:` protocol).
//!
//! A page on `http://localhost:PORT` is shown as
//!   blazeweb://localhost/localhost:PORT/<path>        (macOS / Linux)
//!   http://blazeweb.localhost/localhost:PORT/<path>   (Windows)
//! so the IDE can inject the preview helpers (element picker, the agent's
//! browser: console, click, screenshots) into its HTML, exactly like project
//! files served by `blazemedia:`. Root-relative URLs inside the page
//! ("/assets/app.js") arrive without the host segment; they are routed to the
//! server named in the Referer, else to the last one used.
//!
//! Only loopback targets are allowed (localhost, 127.0.0.1, [::1]).

use std::sync::Mutex;
use std::time::Duration;

/// One request from the webview, already split from its URL.
pub struct ProxyReq {
    pub method: String,
    /// Path + query of the blazeweb URL, e.g. "/localhost:8000/index.html?x=1".
    pub path: String,
    pub referer: Option<String>,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

pub struct ProxyResp {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl ProxyResp {
    fn text(status: u16, msg: &str) -> Self {
        ProxyResp { status, headers: vec![("content-type".into(), "text/plain; charset=utf-8".into())], body: msg.as_bytes().to_vec() }
    }
}

/// "localhost:8000" / "127.0.0.1:5173" / "[::1]:3000" -> normalised target, else None.
pub fn parse_target(seg: &str) -> Option<String> {
    let (host, port) = if let Some(rest) = seg.strip_prefix("[::1]") {
        ("[::1]", rest.strip_prefix(':')?)
    } else {
        seg.rsplit_once(':')?
    };
    if !matches!(host.to_ascii_lowercase().as_str(), "localhost" | "127.0.0.1" | "[::1]") {
        return None;
    }
    let p: u16 = port.parse().ok()?;
    if p == 0 {
        return None;
    }
    Some(format!("{}:{p}", host.to_ascii_lowercase()))
}

/// Split "/localhost:8000/a/b?q" into ("localhost:8000", "/a/b?q").
pub fn split_target(path: &str) -> Option<(String, String)> {
    let p = path.strip_prefix('/').unwrap_or(path);
    let (first, rest) = match p.find(['/', '?']) {
        Some(i) => (&p[..i], &p[i..]),
        None => (p, ""),
    };
    let target = parse_target(first)?;
    let rest = if rest.is_empty() { "/".to_string() } else if rest.starts_with('?') { format!("/{rest}") } else { rest.to_string() };
    Some((target, rest))
}

/// The path part of a blazeweb URL (works for both URL shapes).
pub fn path_of(url: &str) -> Option<String> {
    let after = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let i = after.find('/')?;
    Some(after[i..].to_string())
}

const HOP: &[&str] = &["host", "connection", "content-length", "accept-encoding", "origin", "referer", "upgrade", "keep-alive", "te", "trailer", "transfer-encoding"];
const DROP_RESP: &[&str] = &["content-security-policy", "content-security-policy-report-only", "x-frame-options", "content-length", "content-encoding", "transfer-encoding", "connection", "strict-transport-security", "cross-origin-opener-policy", "cross-origin-embedder-policy", "cross-origin-resource-policy"];

/// Forward one request to the loopback dev server. `inject` puts the preview
/// helper scripts into HTML responses.
pub async fn handle(http: &reqwest::Client, req: ProxyReq, last: &Mutex<Option<String>>, inject: impl Fn(&mut String)) -> ProxyResp {
    let (target, rest) = match split_target(&req.path) {
        Some(tr) => tr,
        None => {
            // Root-relative URL from inside the page: use the page's server.
            let from_ref = req.referer.as_deref().and_then(path_of).and_then(|p| split_target(&p)).map(|(t, _)| t);
            let t = from_ref.or_else(|| last.lock().ok().and_then(|g| g.clone()));
            match t {
                Some(t) => (t, if req.path.is_empty() { "/".into() } else { req.path.clone() }),
                None => return ProxyResp::text(404, "Open a page like http://localhost:3000 first."),
            }
        }
    };
    if let Ok(mut g) = last.lock() {
        *g = Some(target.clone());
    }
    let url = format!("http://{target}{rest}");
    let method = reqwest::Method::from_bytes(req.method.as_bytes()).unwrap_or(reqwest::Method::GET);
    let mut rb = http.request(method, &url).timeout(Duration::from_secs(30)).header("accept-encoding", "identity");
    for (k, v) in &req.headers {
        if !HOP.contains(&k.to_ascii_lowercase().as_str()) {
            rb = rb.header(k.as_str(), v.as_str());
        }
    }
    if !req.body.is_empty() {
        rb = rb.body(req.body);
    }
    let resp = match rb.send().await {
        Ok(r) => r,
        Err(e) => {
            return ProxyResp::text(502, &format!("Nothing answered at http://{target} ({}). Is the dev server running?", if e.is_connect() { "connection refused" } else { "request failed" }));
        }
    };
    let status = resp.status().as_u16();
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut is_html = false;
    for (k, v) in resp.headers() {
        let name = k.as_str().to_ascii_lowercase();
        if DROP_RESP.contains(&name.as_str()) {
            continue;
        }
        let Ok(val) = v.to_str() else { continue };
        if name == "content-type" && val.to_ascii_lowercase().starts_with("text/html") {
            is_html = true;
        }
        if name == "location" {
            headers.push((name, rewrite_location(val, &target)));
            continue;
        }
        headers.push((name, val.to_string()));
    }
    let bytes = resp.bytes().await.map(|b| b.to_vec()).unwrap_or_default();
    let body = if is_html {
        let mut s = String::from_utf8_lossy(&bytes).into_owned();
        inject(&mut s);
        s.into_bytes()
    } else {
        bytes
    };
    ProxyResp { status, headers, body }
}

/// Keep redirects inside the proxy: "http://localhost:8000/x" and "/x" become
/// "/localhost:8000/x" (relative to the blazeweb origin); other URLs unchanged.
pub fn rewrite_location(loc: &str, target: &str) -> String {
    for scheme in ["http://", "https://"] {
        if let Some(rest) = loc.strip_prefix(scheme) {
            let (host, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
            if let Some(t) = parse_target(host) {
                return format!("/{t}{}", if path.is_empty() { "/" } else { path });
            }
            return loc.to_string();
        }
    }
    if loc.starts_with('/') && !loc.starts_with("//") {
        return format!("/{target}{loc}");
    }
    loc.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_are_loopback_only() {
        assert_eq!(parse_target("localhost:8000").as_deref(), Some("localhost:8000"));
        assert_eq!(parse_target("127.0.0.1:5173").as_deref(), Some("127.0.0.1:5173"));
        assert_eq!(parse_target("[::1]:3000").as_deref(), Some("[::1]:3000"));
        assert_eq!(parse_target("example.com:80"), None);
        assert_eq!(parse_target("10.0.0.1:80"), None);
        assert_eq!(parse_target("localhost"), None);
        assert_eq!(parse_target("assets"), None);
        assert_eq!(split_target("/localhost:8000/a/b?q=1"), Some(("localhost:8000".into(), "/a/b?q=1".into())));
        assert_eq!(split_target("/localhost:8000"), Some(("localhost:8000".into(), "/".into())));
        assert_eq!(split_target("/localhost:8000?x"), Some(("localhost:8000".into(), "/?x".into())));
        assert_eq!(split_target("/assets/app.js"), None);
        assert_eq!(path_of("blazeweb://localhost/localhost:8000/index.html").as_deref(), Some("/localhost:8000/index.html"));
        assert_eq!(path_of("http://blazeweb.localhost/127.0.0.1:9/x").as_deref(), Some("/127.0.0.1:9/x"));
        assert_eq!(rewrite_location("http://localhost:8000/login", "localhost:8000"), "/localhost:8000/login");
        assert_eq!(rewrite_location("/login", "localhost:8000"), "/localhost:8000/login");
        assert_eq!(rewrite_location("https://example.com/x", "localhost:8000"), "https://example.com/x");
    }

    #[tokio::test]
    async fn proxies_injects_and_routes_root_relative_urls() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            for _ in 0..2 {
                let (mut s, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 8192];
                let n = s.read(&mut buf).await.unwrap();
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let (ctype, body) = if req.starts_with("GET /app.js") { ("text/javascript", "console.log(1)") } else { ("text/html", "<html><head><title>t</title></head><body>hi</body></html>") };
                let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: {ctype}\r\nX-Frame-Options: DENY\r\nContent-Security-Policy: script-src 'self'\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                s.write_all(resp.as_bytes()).await.unwrap();
            }
        });
        let http = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
        let last = Mutex::new(None);
        let inject = |s: &mut String| { let i = s.find("<head>").map(|i| i + 6).unwrap_or(0); s.insert_str(i, "<script>HELPER</script>"); };
        let page = handle(&http, ProxyReq { method: "GET".into(), path: format!("/127.0.0.1:{port}/"), referer: None, headers: vec![], body: vec![] }, &last, inject).await;
        assert_eq!(page.status, 200);
        let html = String::from_utf8(page.body).unwrap();
        assert!(html.starts_with("<html><head><script>HELPER</script>"), "{html}");
        assert!(!page.headers.iter().any(|(k, _)| k == "x-frame-options" || k == "content-security-policy"));
        // "/app.js" requested from that page: routed by the Referer, not injected
        let js = handle(&http, ProxyReq { method: "GET".into(), path: "/app.js".into(), referer: Some(format!("blazeweb://localhost/127.0.0.1:{port}/")), headers: vec![], body: vec![] }, &last, inject).await;
        assert_eq!(String::from_utf8(js.body).unwrap(), "console.log(1)");
        // nothing listening -> a clear 502
        let dead = handle(&http, ProxyReq { method: "GET".into(), path: "/127.0.0.1:9/".into(), referer: None, headers: vec![], body: vec![] }, &last, inject).await;
        assert_eq!(dead.status, 502);
    }
}
