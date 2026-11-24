//! The BlazeAPI relay: a phone reaches this machine from anywhere at
//! `https://m.blazeapi.org/r/<slot>/` with no setup.
//!
//! This machine polls the relay over HTTPS (`POST /connect/poll`, signed in
//! with its BlazeAPI key) and gets the phone's API requests; it answers each
//! with `POST /connect/reply`. Nothing listens on this machine for it. The
//! relay serves the phone app itself and forwards only `/api/*` calls, so no
//! computer can run script on the shared web origin. The relay (run by
//! BlazeAPI) can technically see this traffic, like Cloudflare can for the
//! tunnel options; the app says so.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::{ClientInfo, Server};

pub const DEFAULT_RELAY: &str = "https://m.blazeapi.org";
const MAX_BODY: usize = 4 * 1024 * 1024;

pub fn relay_base() -> String {
    #[cfg(debug_assertions)]
    if let Ok(v) = std::env::var("BLAZE_RELAY_URL") {
        if !v.trim().is_empty() {
            return v.trim_end_matches('/').to_string();
        }
    }
    DEFAULT_RELAY.to_string()
}

/// A new random slot id (lowercase letters and digits, 16 chars).
pub fn new_slot() -> String {
    let t = uuid::Uuid::new_v4().simple().to_string();
    t[..16].to_string()
}

pub fn phone_url(base: &str, slot: &str) -> String {
    format!("{}/r/{slot}/", base.trim_end_matches('/'))
}

#[derive(Deserialize, Debug)]
pub struct Incoming {
    pub id: String,
    pub method: String,
    /// Path after `/r/<slot>`, e.g. `/api/chats?root=...`.
    pub path: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub ip: String,
}

#[derive(Serialize, Debug)]
pub struct Reply {
    pub id: String,
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

/// Turn a relayed phone request into a router call and the answer into a reply.
pub async fn answer(server: &Arc<Server>, slot: &str, inc: Incoming) -> Reply {
    let b64 = base64::engine::general_purpose::STANDARD;
    let bad = |id: String| Reply { id, status: 400, headers: vec![], body: String::new() };
    if !inc.path.starts_with("/api/") && !inc.path.starts_with("/healthz") {
        return bad(inc.id);
    }
    let Ok(body) = b64.decode(inc.body.as_bytes()) else { return bad(inc.id) };
    let mut rb = Request::builder().method(inc.method.as_str()).uri(inc.path.as_str());
    for (k, v) in &inc.headers {
        // Only what the app's API needs.
        if matches!(k.to_ascii_lowercase().as_str(), "cookie" | "content-type" | "x-blaze" | "origin" | "host" | "x-forwarded-host" | "user-agent") {
            rb = rb.header(k.as_str(), v.as_str());
        }
    }
    let Ok(mut req) = rb.body(Body::from(body)) else { return bad(inc.id) };
    req.extensions_mut().insert(ClientInfo { ip: if inc.ip.is_empty() { "relay".into() } else { inc.ip.clone() }, https: true, prefix: format!("/r/{slot}") });
    let res = super::handle(server, req).await;
    let status = res.status().as_u16();
    let headers: Vec<(String, String)> = res
        .headers()
        .iter()
        .filter(|(k, _)| matches!(k.as_str(), "content-type" | "set-cookie" | "cache-control"))
        .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.as_str().to_string(), v.to_string())))
        .collect();
    let bytes = axum::body::to_bytes(res.into_body(), MAX_BODY).await.unwrap_or_default();
    Reply { id: inc.id, status, headers, body: b64.encode(&bytes) }
}

/// Connection state shown in Settings.
#[derive(Clone, Debug, PartialEq)]
pub enum RelayState {
    Connecting,
    Online,
    Error(String),
}

/// Keep polling the relay until `cancel`. `on_state` reports connection
/// changes. Two polls stay open so a request is picked up at once even while
/// another is being answered.
pub async fn run(http: reqwest::Client, base: String, key: String, slot: String, server: Arc<Server>, cancel: CancellationToken, on_state: Arc<dyn Fn(RelayState) + Send + Sync>) {
    let mut workers = Vec::new();
    for _ in 0..2 {
        let (http, base, key, slot, server, cancel, on_state) = (http.clone(), base.clone(), key.clone(), slot.clone(), server.clone(), cancel.clone(), on_state.clone());
        workers.push(tokio::spawn(async move {
            let mut backoff = 1u64;
            on_state(RelayState::Connecting);
            loop {
                if cancel.is_cancelled() {
                    break;
                }
                let poll = http
                    .post(format!("{base}/connect/poll"))
                    .bearer_auth(&key)
                    .header("X-Blaze-Slot", &slot)
                    .header("X-Blaze-Name", server.host().label())
                    .timeout(Duration::from_secs(40))
                    .send();
                let r = tokio::select! {
                    r = poll => r,
                    _ = cancel.cancelled() => break,
                };
                match r {
                    Ok(resp) if resp.status() == StatusCode::OK => {
                        backoff = 1;
                        on_state(RelayState::Online);
                        let list: Vec<Incoming> = resp.json().await.unwrap_or_default();
                        for inc in list {
                            let (http, base, key, slot, server) = (http.clone(), base.clone(), key.clone(), slot.clone(), server.clone());
                            tokio::spawn(async move {
                                let reply = answer(&server, &slot, inc).await;
                                let _ = http.post(format!("{base}/connect/reply")).bearer_auth(&key).header("X-Blaze-Slot", &slot).json(&reply).timeout(Duration::from_secs(30)).send().await;
                            });
                        }
                    }
                    Ok(resp) => {
                        let status = resp.status().as_u16();
                        let msg = resp.json::<serde_json::Value>().await.ok().and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string)).unwrap_or_else(|| format!("The relay answered HTTP {status}"));
                        on_state(RelayState::Error(msg));
                        // Auth problems will not fix themselves quickly.
                        backoff = if status == 401 || status == 403 { 60 } else { (backoff * 2).min(30) };
                        tokio::select! { _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}, _ = cancel.cancelled() => break }
                    }
                    Err(e) => {
                        on_state(RelayState::Error(format!("Cannot reach the relay: {e}")));
                        backoff = (backoff * 2).min(30);
                        tokio::select! { _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}, _ = cancel.cancelled() => break }
                    }
                }
            }
        }));
    }
    for w in workers {
        let _ = w.await;
    }
}
