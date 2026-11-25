//! Blaze IDE core. Everything the desktop shell needs that is not UI: the
//! project sandbox, files, search, terminal, file watching, BlazeAPI sign-in,
//! models and AI streaming. No Tauri types here, so all of it builds and is
//! unit-tested on any OS; `src-tauri` is a thin command layer on top.

pub mod agent;
pub mod ai;
pub mod auth;
pub mod bots;
pub mod chatfile;
pub mod chats;
pub mod edit;
pub mod error;
pub mod extensions;
pub mod files;
pub mod media_store;
pub mod mediagen;
pub mod models;
pub mod paths;
pub mod presence;
pub mod secret_file;
pub mod prompts;
pub mod providers;
pub mod remote;
pub mod pty;
pub mod store;
pub mod tools;
pub mod watch;
pub mod webproxy;

pub use error::{Error, Result};
/// Re-exported so the shell uses exactly the same HTTP client type.
pub use reqwest;

/// Production BlazeAPI origin. Debug builds may point elsewhere with
/// BLAZE_IDE_API_BASE (for a local backend); release builds never read it, so a
/// tampered environment cannot redirect a user's key to another host.
pub fn api_base() -> String {
    #[cfg(debug_assertions)]
    if let Ok(v) = std::env::var("BLAZE_IDE_API_BASE") {
        if !v.trim().is_empty() {
            return v.trim_end_matches('/').to_string();
        }
    }
    "https://blazeapi.org".to_string()
}

/// Base for the OpenAI-style API (`/paid/v1/...`: models, chat, images, usage).
/// Production serves it from its own host; `/api/...` (sign-in, analytics)
/// stays on [`api_base`]. A debug override applies to both.
pub fn v1_base() -> String {
    #[cfg(debug_assertions)]
    if std::env::var("BLAZE_IDE_API_BASE").map(|v| !v.trim().is_empty()).unwrap_or(false) {
        return api_base();
    }
    "https://api.blazeapi.org".to_string()
}

/// One HTTP client for the whole app: rustls (no OpenSSL to ship), HTTPS
/// only in release builds, sane timeouts.
pub fn http_client() -> reqwest::Client {
    let b = reqwest::Client::builder()
        .user_agent(concat!("BlazeIDE/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(15))
        // Short idle reuse + TCP keepalive: servers and proxies (Cloudflare,
        // Surplus) drop idle connections, and reusing a dead one fails with
        // "error sending request" before anything is sent.
        .pool_idle_timeout(std::time::Duration::from_secs(20))
        .tcp_keepalive(std::time::Duration::from_secs(30));
    #[cfg(not(debug_assertions))]
    let b = b.https_only(true);
    b.build().expect("http client")
}

/// Client for user-added providers. Same as `http_client` but allows plain
/// http, because a local Ollama / LM Studio has no TLS. The URLs it is used
/// with are validated by `providers::validate_base_url` (https, or http to
/// this machine only), and reqwest drops the key on cross-origin redirects.
pub fn custom_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("BlazeIDE/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(15))
        .pool_idle_timeout(std::time::Duration::from_secs(20))
        .tcp_keepalive(std::time::Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("custom http client")
}
