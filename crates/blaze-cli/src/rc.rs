//! Remote connect (`/rc` in the chat): run the phone server inside this
//! process so a phone can continue chats, approve actions and talk to BlBots.
//!
//! "auto" uses the BlazeAPI relay when signed in and falls back to a free
//! Cloudflare quick tunnel if the relay is not reachable within a few seconds.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use blaze_core::remote::{self, auth, relay, tunnel, Server};
use tokio_util::sync::CancellationToken;

use crate::config;
use crate::serve::CliHost;
use crate::session::Session;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Way {
    Auto,
    Relay,
    Tunnel,
    Lan,
}

impl Way {
    pub fn parse(s: &str) -> Option<Way> {
        match s.trim() {
            "" | "auto" | "on" => Some(Way::Auto),
            "relay" => Some(Way::Relay),
            "tunnel" | "quick" => Some(Way::Tunnel),
            "lan" | "wifi" | "ip" => Some(Way::Lan),
            _ => None,
        }
    }
}

pub struct Remote {
    pub server: Arc<Server>,
    /// (what, url), best first.
    pub urls: Vec<(String, String)>,
    pub _relay_state: Arc<Mutex<String>>,
    _running: remote::Running,
    _tunnels: Vec<tunnel::Tunnel>,
    cancel: CancellationToken,
}

impl Drop for Remote {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl Remote {
    pub fn best(&self) -> Option<&str> {
        self.urls.first().map(|(_, u)| u.as_str())
    }
    /// A sign-in link for a QR (one-time code, 5 minutes).
    pub fn pair_link(&self) -> Option<String> {
        self.best().map(|u| format!("{u}#pair={}", self.server.pair_code()))
    }
}

/// Start remote connect. `log` gets progress lines for the UI.
pub async fn start(s: &Session, way: Way, log: impl Fn(String) + Send + Sync + 'static) -> Result<Remote, String> {
    let host = Arc::new(CliHost::new(s));
    let store = blaze_core::store::Store::open(&config::config_dir());
    let cfg = auth::update(&store, |c| {
        c.enabled = true;
        if c.relay_slot.is_none() {
            c.relay_slot = Some(relay::new_slot());
        }
        Ok(c.clone())
    })
    .map_err(|e| e.to_string())?;
    if cfg.password_hash.is_none() {
        return Err("Set a phone password first.".into());
    }
    let lan = way == Way::Lan;
    // A random local port unless the phone connects directly over Wi-Fi
    // (Blaze IDE may already use the default port for its own phone server).
    let port = if lan { cfg.port } else { 0 };
    let server = Server::new(host);
    let running = remote::serve(server.clone(), port, lan).await.map_err(|e| e.to_string())?;
    let cancel = CancellationToken::new();
    let relay_state = Arc::new(Mutex::new(String::new()));
    let mut urls = Vec::new();
    let mut tunnels = Vec::new();

    if lan {
        for u in remote::lan_urls(running.addr.port()) {
            urls.push(("Same Wi-Fi (plain http)".to_string(), u));
        }
        if urls.is_empty() {
            return Err("No Wi-Fi/LAN address found on this computer.".into());
        }
    }

    let want_relay = matches!(way, Way::Auto | Way::Relay) && s.key.is_some();
    if way == Way::Relay && s.key.is_none() {
        return Err("The BlazeAPI relay needs you signed in (/login).".into());
    }
    let mut relay_ok = false;
    if want_relay {
        let slot = cfg.relay_slot.clone().unwrap_or_else(relay::new_slot);
        let base = relay::relay_base();
        let rs = relay_state.clone();
        let cb: Arc<dyn Fn(relay::RelayState) + Send + Sync> = Arc::new(move |st| {
            *rs.lock().expect("relay") = match st {
                relay::RelayState::Connecting => "connecting".into(),
                relay::RelayState::Online => "online".into(),
                relay::RelayState::Error(e) => e,
            };
        });
        let relay_cancel = cancel.child_token();
        log("Connecting to the BlazeAPI relay...".into());
        tokio::spawn(relay::run(s.http.clone(), base.clone(), s.key.clone().unwrap_or_default(), slot.clone(), server.clone(), relay_cancel.clone(), cb));
        for _ in 0..16 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if relay_state.lock().expect("relay").as_str() == "online" {
                relay_ok = true;
                break;
            }
        }
        if relay_ok {
            urls.insert(0, ("BlazeAPI relay (works anywhere)".into(), relay::phone_url(&base, &slot)));
        } else {
            let why = relay_state.lock().expect("relay").clone();
            relay_cancel.cancel();
            if way == Way::Relay {
                return Err(format!("The relay is not reachable right now ({why})."));
            }
            log(format!("Relay not reachable ({}); using a quick tunnel instead.", if why.is_empty() { "no answer" } else { why.as_str() }));
        }
    }
    if way == Way::Tunnel || (way == Way::Auto && !relay_ok) {
        log(format!("Starting a Cloudflare quick tunnel (cloudflared {})...", tunnel::CLOUDFLARED_VERSION));
        let bin = tunnel::ensure_cloudflared(&s.http, &config::config_dir()).await.map_err(|e| e.to_string())?;
        let t = tunnel::quick(&bin, running.addr.port()).await.map_err(|e| e.to_string())?;
        if let Some(u) = &t.url {
            urls.insert(0, ("Quick tunnel (address changes each time)".into(), u.clone()));
        }
        tunnels.push(t);
    }
    if urls.is_empty() {
        return Err("No way in could be started.".into());
    }
    Ok(Remote { server, urls, _relay_state: relay_state, _running: running, _tunnels: tunnels, cancel })
}

/// Save a new phone password (min 10 chars). Signs existing phones out.
pub fn set_password(pw: &str) -> Result<(), String> {
    let store = blaze_core::store::Store::open(&config::config_dir());
    auth::set_password(&store, pw).map_err(|e| e.to_string())
}

pub fn has_password() -> bool {
    auth::load(&blaze_core::store::Store::open(&config::config_dir())).password_hash.is_some()
}
