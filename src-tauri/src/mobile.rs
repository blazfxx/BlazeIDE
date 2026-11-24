//! Continue on mobile inside Blaze IDE: runs the phone server from
//! blaze_core::remote while the app is open, with the ways in the user turned
//! on (Wi-Fi, quick tunnel, own domain, BlazeAPI relay).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use blaze_core::agent::{AgentConfig, Approval, Backend};
use blaze_core::models::ModelEntry;
use blaze_core::remote::{self, auth, relay, tunnel, Host, Server};
use blaze_core::store::Store;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};
use tokio_util::sync::CancellationToken;

use crate::{secrets, AppState};

pub const TUNNEL_TOKEN: &str = "mobile-tunnel-token";

pub struct IdeHost {
    app: AppHandle,
}

fn hostname() -> String {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        let mut c = std::process::Command::new("hostname");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x0800_0000);
        }
        c.output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().trim_end_matches(".local").to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "this computer".into())
    })
    .clone()
}

impl Host for IdeHost {
    fn store(&self) -> &Store {
        &self.app.state::<AppState>().inner().store
    }
    fn backend(&self) -> Backend {
        let st = self.app.state::<AppState>();
        let models = st.models.read().expect("models lock").clone();
        let backend = Backend {
            http: st.http.clone(),
            http_custom: st.http_custom.clone(),
            v1_base: st.v1_base.clone(),
            key: st.api_key(),
            routing: crate::commands::routing_for(&st),
            providers: st.store.providers(),
            provider_key: Arc::new(secrets::load_provider_key),
            models,
        };
        backend
    }
    fn agent_config(&self, root: &Path, model: &str, mode: &str) -> AgentConfig {
        let st = self.app.state::<AppState>();
        let s = st.store.settings();
        let str_of = |k: &str| s.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let bool_of = |k: &str| s.get(k).and_then(Value::as_bool).unwrap_or(false);
        let mut c = AgentConfig::new(root.to_path_buf(), model, mode);
        c.approval = Approval::parse(&str_of("mobileApproval")).unwrap_or(Approval::Edits);
        c.image_gen = bool_of("aiMediaGen");
        c.video_gen = bool_of("aiVideoGen");
        c.image_model = str_of("imageModel");
        c.video_model = str_of("videoModel");
        let effort = str_of("reasoningEffort");
        c.reasoning_effort = Some(if effort.is_empty() { "medium".into() } else { effort });
        c.extra_system = Some("This turn was sent from the user's phone (Continue on mobile). There is no editor or preview browser for this turn; keep replies short enough to read on a phone.".into());
        c
    }
    fn projects(&self) -> Vec<PathBuf> {
        self.store().recent_projects().into_iter().map(PathBuf::from).collect()
    }
    fn models(&self) -> Vec<ModelEntry> {
        let st = self.app.state::<AppState>();
        let list = st.models.read().expect("models lock").clone();
        list
    }
    fn default_model(&self) -> String {
        let m = self.store().settings().get("selectedModel").and_then(Value::as_str).unwrap_or("").to_string();
        if m.is_empty() { blaze_core::chatfile::DEFAULT_MODEL.into() } else { m }
    }
    fn label(&self) -> String {
        format!("Blaze IDE on {}", hostname())
    }
    fn bots(&self) -> Option<blaze_core::bots::runner::BotCtx> {
        Some(crate::blbots::ctx(&self.app))
    }
}

/// The running phone server and its ways in.
pub struct Runtime {
    server: Arc<Server>,
    _running: remote::Running,
    _tunnels: Vec<tunnel::Tunnel>,
    cancel: CancellationToken,
    urls: Vec<(String, String)>,
    relay_state: Arc<Mutex<String>>,
    error: Option<String>,
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

pub static RUNTIME: Mutex<Option<Runtime>> = Mutex::new(None);
static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);

fn set_error(e: Option<String>) {
    *LAST_ERROR.lock().expect("mobile") = e;
}

/// Start, restart or stop the phone server to match the saved settings.
pub async fn apply(app: &AppHandle) {
    // Stop whatever runs now (drops tunnels, cancels the relay, frees the port).
    let old = RUNTIME.lock().expect("mobile").take();
    drop(old);
    set_error(None);
    let st = app.state::<AppState>();
    let cfg = auth::load(&st.store);
    if !cfg.enabled || cfg.password_hash.is_none() {
        return;
    }
    match start(app, cfg).await {
        Ok(rt) => *RUNTIME.lock().expect("mobile") = Some(rt),
        Err(e) => set_error(Some(e)),
    }
}

async fn start(app: &AppHandle, cfg: auth::MobileConfig) -> Result<Runtime, String> {
    let st = app.state::<AppState>();
    let host = Arc::new(IdeHost { app: app.clone() });
    let server = Server::new(host);
    let running = remote::serve(server.clone(), cfg.port, cfg.lan).await.map_err(|e| e.to_string())?;
    let port = running.addr.port();
    let cancel = CancellationToken::new();
    let mut urls = Vec::new();
    let mut tunnels = Vec::new();
    let mut error = None;
    if cfg.lan {
        for u in remote::lan_urls(port) {
            urls.push(("Same Wi-Fi".to_string(), u));
        }
    }
    if cfg.quick_tunnel || (cfg.domain.is_some() && cfg.domain_tunnel) {
        let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
        match tunnel::ensure_cloudflared(&st.http, &dir).await {
            Ok(bin) => {
                if cfg.quick_tunnel {
                    match tunnel::quick(&bin, port).await {
                        Ok(t) => {
                            if let Some(u) = &t.url {
                                urls.push(("Quick tunnel".into(), u.clone()));
                            }
                            tunnels.push(t);
                        }
                        Err(e) => error = Some(e.to_string()),
                    }
                }
                if cfg.domain.is_some() && cfg.domain_tunnel {
                    match secrets::load_secret(TUNNEL_TOKEN) {
                        Some(token) => match tunnel::named(&bin, &token).await {
                            Ok(t) => tunnels.push(t),
                            Err(e) => error = Some(e.to_string()),
                        },
                        None => error = Some("Paste your Cloudflare tunnel token to use your domain.".into()),
                    }
                }
            }
            Err(e) => error = Some(e.to_string()),
        }
    }
    if let Some(d) = cfg.domain.as_deref() {
        let d = d.trim_end_matches('/').to_string();
        if cfg.domain_tunnel {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
        match tunnel::verify(&st.http, &d, &server.server_id).await {
            Ok(()) => urls.push(("Your domain".into(), format!("{d}/"))),
            Err(e) => error = Some(format!("{e}. Point it at http://localhost:{port}.")),
        }
    }
    let relay_state = Arc::new(Mutex::new(String::new()));
    if cfg.relay {
        match (st.api_key(), cfg.relay_slot.clone()) {
            (Some(key), Some(slot)) => {
                let base = relay::relay_base();
                urls.push(("BlazeAPI relay".into(), relay::phone_url(&base, &slot)));
                let rs = relay_state.clone();
                let cb: Arc<dyn Fn(relay::RelayState) + Send + Sync> = Arc::new(move |s| {
                    *rs.lock().expect("relay state") = match s {
                        relay::RelayState::Connecting => "connecting".into(),
                        relay::RelayState::Online => "online".into(),
                        relay::RelayState::Error(e) => e,
                    };
                });
                tauri::async_runtime::spawn(relay::run(st.http.clone(), base, key, slot, server.clone(), cancel.clone(), cb));
            }
            (None, _) => error = Some("Sign in to BlazeAPI to use the relay.".into()),
            (_, None) => error = Some("The relay address was not set up; turn the relay off and on again.".into()),
        }
    }
    Ok(Runtime { server, _running: running, _tunnels: tunnels, cancel, urls, relay_state, error })
}

pub fn status(app: &AppHandle) -> Value {
    let st = app.state::<AppState>();
    let cfg = auth::load(&st.store);
    let rt = RUNTIME.lock().expect("mobile");
    let urls: Vec<Value> = rt.as_ref().map(|r| r.urls.iter().map(|(l, u)| json!({ "label": l, "url": u })).collect()).unwrap_or_default();
    let error = rt.as_ref().and_then(|r| r.error.clone()).or_else(|| LAST_ERROR.lock().expect("mobile").clone());
    let devices: Vec<Value> = cfg.devices.iter().map(|d| json!({ "id": d.id, "name": d.name, "lastSeen": d.last_seen, "created": d.created })).collect();
    json!({
        "enabled": cfg.enabled,
        "hasPassword": cfg.password_hash.is_some(),
        "running": rt.is_some(),
        "port": cfg.port,
        "lan": cfg.lan,
        "quickTunnel": cfg.quick_tunnel,
        "relay": cfg.relay,
        "domain": cfg.domain,
        "domainTunnel": cfg.domain_tunnel,
        "hasTunnelToken": secrets::load_secret(TUNNEL_TOKEN).is_some(),
        "urls": urls,
        "relayState": rt.as_ref().map(|r| r.relay_state.lock().expect("relay state").clone()).unwrap_or_default(),
        "activeRuns": rt.as_ref().map(|r| r.server.active_runs()).unwrap_or(0),
        "devices": devices,
        "error": error,
    })
}

/// A QR (SVG text) that signs a phone in without the password (5 minutes).
pub fn pair_qr(url: &str) -> Result<Value, String> {
    let rt = RUNTIME.lock().expect("mobile");
    let r = rt.as_ref().ok_or("Continue on mobile is not running")?;
    if !r.urls.iter().any(|(_, u)| u == url) {
        return Err("Unknown address".into());
    }
    let link = format!("{url}#pair={}", r.server.pair_code());
    Ok(json!({ "link": link, "svg": remote::qr::svg(&link) }))
}
