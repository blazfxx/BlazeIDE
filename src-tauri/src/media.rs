//! `blazemedia:` protocol: serves local files to the webview (custom
//! backgrounds, ambient audio, image tabs and the live web preview).
//!
//! URL shape (built by the UI's `mediaUrl`):
//!   macOS/Linux: blazemedia://localhost/<absolute path, slashes kept>
//!   Windows:     http://blazemedia.localhost/C:/path/to/file
//! Slashes are kept (not %2F-encoded) so relative links inside a previewed
//! page (./style.css) resolve to sibling files.
//!
//! Only files inside an open project, or a file the user picked in a dialog,
//! are served. Everything else is a 404, so a previewed page cannot read
//! arbitrary files from the disk.

use std::io::{Read, Seek, SeekFrom};

use blaze_core::paths::Sandbox;
use percent_encoding::percent_decode_str;
use tauri::http::{header, Request, Response, StatusCode};

const PREVIEW_SCRIPT: &str = include_str!("preview_select.js");
const MAX_WHOLE_FILE: u64 = 64 * 1024 * 1024;

fn path_from_uri(uri: &str) -> Option<String> {
    let u = url::Url::parse(uri).ok()?;
    let decoded = percent_decode_str(u.path()).decode_utf8().ok()?.into_owned();
    #[cfg(windows)]
    {
        // "/C:/x/y" -> "C:/x/y"
        let p = decoded.trim_start_matches('/');
        if p.len() >= 2 && p.as_bytes()[1] == b':' {
            return Some(p.replace('/', "\\"));
        }
        None
    }
    #[cfg(not(windows))]
    {
        if decoded.starts_with('/') && decoded.len() > 1 { Some(decoded) } else { None }
    }
}

fn not_found() -> Response<Vec<u8>> {
    Response::builder().status(StatusCode::NOT_FOUND).header(header::CONTENT_TYPE, "text/plain").body(b"Not found".to_vec()).expect("response")
}

fn parse_range(h: &str, len: u64) -> Option<(u64, u64)> {
    let spec = h.strip_prefix("bytes=")?.split(',').next()?.trim();
    let (a, b) = spec.split_once('-')?;
    let (start, end) = if a.is_empty() {
        let n: u64 = b.parse().ok()?;
        (len.saturating_sub(n), len.saturating_sub(1))
    } else {
        let s: u64 = a.parse().ok()?;
        let e: u64 = if b.is_empty() { len.saturating_sub(1) } else { b.parse().ok()? };
        (s, e.min(len.saturating_sub(1)))
    };
    (start <= end && start < len).then_some((start, end))
}

pub fn handle(sandbox: &Sandbox, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let Some(raw) = path_from_uri(&req.uri().to_string()) else { return not_found() };
    let Ok(path) = sandbox.require_readable(&raw) else { return not_found() };
    let Ok(meta) = std::fs::metadata(&path) else { return not_found() };
    if !meta.is_file() {
        return not_found();
    }
    let mime = mime_guess::from_path(&path).first_or_octet_stream().essence_str().to_string();
    let len = meta.len();
    let base = || {
        Response::builder()
            .header(header::CONTENT_TYPE, mime.as_str())
            .header(header::CACHE_CONTROL, "no-store")
            .header("X-Content-Type-Options", "nosniff")
            .header(header::ACCEPT_RANGES, "bytes")
    };

    if mime == "text/html" {
        let Ok(bytes) = std::fs::read(&path) else { return not_found() };
        let mut body = String::from_utf8_lossy(&bytes).into_owned();
        let tag = format!("<script>{PREVIEW_SCRIPT}</script>");
        match body.rfind("</body>") {
            Some(i) => body.insert_str(i, &tag),
            None => body.push_str(&tag),
        }
        return base().header(header::CONTENT_TYPE, "text/html; charset=utf-8").body(body.into_bytes()).expect("response");
    }

    // Range requests let <video>/<audio> seek and stream large files.
    if let Some((start, end)) = req.headers().get(header::RANGE).and_then(|v| v.to_str().ok()).and_then(|h| parse_range(h, len)) {
        let Ok(mut f) = std::fs::File::open(&path) else { return not_found() };
        let count = (end - start + 1).min(8 * 1024 * 1024);
        let mut buf = vec![0u8; count as usize];
        if f.seek(SeekFrom::Start(start)).is_err() || f.read_exact(&mut buf).is_err() {
            return not_found();
        }
        return base()
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_RANGE, format!("bytes {start}-{}/{len}", start + count - 1))
            .body(buf)
            .expect("response");
    }
    if len > MAX_WHOLE_FILE {
        return Response::builder().status(StatusCode::PAYLOAD_TOO_LARGE).body(Vec::new()).expect("response");
    }
    match std::fs::read(&path) {
        Ok(bytes) => base().body(bytes).expect("response"),
        Err(_) => not_found(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_range("bytes=0-99", 1000), Some((0, 99)));
        assert_eq!(parse_range("bytes=500-", 1000), Some((500, 999)));
        assert_eq!(parse_range("bytes=-100", 1000), Some((900, 999)));
        assert_eq!(parse_range("bytes=2000-", 1000), None);
        assert_eq!(parse_range("items=0-1", 1000), None);
    }

    #[cfg(not(windows))]
    #[test]
    fn uri_to_path() {
        assert_eq!(path_from_uri("blazemedia://localhost/Users/me/my%20site/index.html").as_deref(), Some("/Users/me/my site/index.html"));
        assert_eq!(path_from_uri("blazemedia://localhost/"), None);
    }

    #[test]
    fn serves_only_sandboxed_files() {
        let dir = tempfile_dir();
        let sb = Sandbox::new();
        let root = sb.grant_root(&dir).unwrap();
        std::fs::write(root.join("page.html"), "<html><body>hi</body></html>").unwrap();
        let url = |p: &std::path::Path| {
            let s = p.display().to_string().replace('\\', "/");
            if cfg!(windows) { format!("http://blazemedia.localhost/{s}") } else { format!("blazemedia://localhost{s}") }
        };
        let req = Request::builder().uri(url(&root.join("page.html"))).body(Vec::new()).unwrap();
        let resp = handle(&sb, &req);
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(String::from_utf8_lossy(resp.body()).contains("void:set-selecting"));
        let outside = std::env::temp_dir().join("blaze-media-outside.txt");
        std::fs::write(&outside, "secret").unwrap();
        let req = Request::builder().uri(url(&outside)).body(Vec::new()).unwrap();
        assert_eq!(handle(&sb, &req).status(), StatusCode::NOT_FOUND);
    }

    fn tempfile_dir() -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("blaze-media-test-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }
}
