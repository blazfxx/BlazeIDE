//! `blaze serve`: Continue on mobile from the terminal (or an always-on
//! machine), and `blaze mobile ...` to manage the password and phones.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use blaze_core::agent::{AgentConfig, Approval, Backend};
use blaze_core::models::ModelEntry;
use blaze_core::remote::{self, auth, relay, tunnel, Host, Server};
use blaze_core::store::Store;
use tokio_util::sync::CancellationToken;

use crate::config;
use crate::session::Session;

pub struct CliHost {
    bots: blaze_core::bots::runner::BotCtx,
    store: Store,
    backend: Backend,
    default_model: String,
    approval: Approval,
    image_gen: bool,
    video_gen: bool,
    image_model: String,
    video_model: String,
    effort: String,
}

impl CliHost {
    pub fn new(s: &Session) -> Self {
        CliHost {
            bots: crate::bots::ctx(s),
            store: Store::open(&config::config_dir()),
            backend: s.backend(),
            default_model: s.default_model(),
            approval: Approval::parse(&s.setting_str("mobileApproval")).unwrap_or(Approval::Edits),
            image_gen: s.setting_bool("aiMediaGen", false),
            video_gen: s.setting_bool("aiVideoGen", false),
            image_model: s.setting_str("imageModel"),
            video_model: s.setting_str("videoModel"),
            effort: s.setting_str("reasoningEffort"),
        }
    }
}

impl Host for CliHost {
    fn store(&self) -> &Store {
        &self.store
    }
    fn backend(&self) -> Backend {
        self.backend.clone()
    }
    fn agent_config(&self, root: &Path, model: &str, mode: &str) -> AgentConfig {
        let mut c = AgentConfig::new(root.to_path_buf(), model, mode);
        c.approval = self.approval;
        c.image_gen = self.image_gen;
        c.video_gen = self.video_gen;
        c.image_model = self.image_model.clone();
        c.video_model = self.video_model.clone();
        c.reasoning_effort = Some(if self.effort.is_empty() { "medium".into() } else { self.effort.clone() });
        c.extra_system = Some("This turn was sent from the user's phone (Continue on mobile). There is no editor or preview browser; keep replies short enough to read on a phone.".into());
        c
    }
    fn projects(&self) -> Vec<PathBuf> {
        self.store.reload();
        self.store.recent_projects().into_iter().map(PathBuf::from).collect()
    }
    fn models(&self) -> Vec<ModelEntry> {
        self.backend.models.clone()
    }
    fn default_model(&self) -> String {
        self.default_model.clone()
    }
    fn label(&self) -> String {
        format!("blaze on {}", hostname())
    }
    fn bots(&self) -> Option<blaze_core::bots::runner::BotCtx> {
        Some(self.bots.clone())
    }
}

pub fn hostname() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .or_else(|| std::env::var("COMPUTERNAME").ok())
        .or_else(|| std::fs::read_to_string("/etc/hostname").ok().map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "this computer".into())
}

fn read_secret(prompt: &str) -> Result<String, String> {
    if !std::io::stdin().is_terminal() {
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).map_err(|e| e.to_string())?;
        return Ok(line.trim_end_matches(['\r', '\n']).to_string());
    }
    eprint!("{prompt}");
    let _ = std::io::stderr().flush();
    // No echo while typing.
    #[cfg(unix)]
    let _restore = NoEcho::on();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).map_err(|e| e.to_string())?;
    eprintln!();
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

#[cfg(unix)]
struct NoEcho(Option<libc::termios>);

#[cfg(unix)]
impl NoEcho {
    fn on() -> Self {
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(0, &mut t) != 0 {
                return NoEcho(None);
            }
            let old = t;
            t.c_lflag &= !libc::ECHO;
            libc::tcsetattr(0, libc::TCSANOW, &t);
            NoEcho(Some(old))
        }
    }
}

#[cfg(unix)]
impl Drop for NoEcho {
    fn drop(&mut self) {
        if let Some(old) = self.0 {
            unsafe {
                libc::tcsetattr(0, libc::TCSANOW, &old);
            }
        }
    }
}

pub fn set_password_interactive(store: &Store) -> Result<(), String> {
    let a = read_secret(&format!("Choose a password for your phone (at least {} characters): ", auth::MIN_PASSWORD))?;
    if a.chars().count() < auth::MIN_PASSWORD {
        return Err(format!("Use at least {} characters.", auth::MIN_PASSWORD));
    }
    let b = read_secret("Type it again: ")?;
    if a != b {
        return Err("The two passwords were different.".into());
    }
    auth::set_password(store, &a).map_err(|e| e.to_string())?;
    eprintln!("Password saved (only a hash is stored). Every phone was signed out.");
    Ok(())
}

pub struct ServeOpts {
    pub lan: bool,
    pub tunnel: bool,
    pub relay: bool,
    pub domain: Option<String>,
    pub domain_token: bool,
    pub port: Option<u16>,
}

pub async fn serve(s: &mut Session, o: ServeOpts) -> Result<(), String> {
    let _ = s.load_models(false).await;
    let host = Arc::new(CliHost::new(s));
    let store = host.store();
    let mut cfg = auth::load(store);
    if cfg.password_hash.is_none() {
        eprintln!("Continue on mobile needs a password first.");
        set_password_interactive(store)?;
    }
    let none_chosen = !o.lan && !o.tunnel && !o.relay && o.domain.is_none();
    let (lan, quick, use_relay, domain) = if none_chosen { (cfg.lan, cfg.quick_tunnel, cfg.relay, cfg.domain.clone()) } else { (o.lan, o.tunnel, o.relay, o.domain.clone()) };
    if !lan && !quick && !use_relay && domain.is_none() {
        return Err("Pick at least one way in: --lan (same Wi-Fi), --tunnel (free Cloudflare address), --relay (BlazeAPI, works anywhere) or --domain https://your.domain".into());
    }
    let port = o.port.unwrap_or(cfg.port);
    cfg = auth::update(store, |c| {
        c.enabled = true;
        c.port = port;
        c.lan = lan;
        c.quick_tunnel = quick;
        c.relay = use_relay;
        c.domain = domain.clone();
        c.domain_tunnel = o.domain_token || c.domain_tunnel;
        if use_relay && c.relay_slot.is_none() {
            c.relay_slot = Some(relay::new_slot());
        }
        Ok(c.clone())
    })
    .map_err(|e| e.to_string())?;

    let server = Server::new(host.clone());
    let running = remote::serve(server.clone(), port, lan).await.map_err(|e| e.to_string())?;
    let cancel = CancellationToken::new();
    let mut urls: Vec<(String, String)> = Vec::new();
    if lan {
        for u in remote::lan_urls(running.addr.port()) {
            urls.push(("Same Wi-Fi (plain http: only on a network you trust)".into(), u));
        }
    } else {
        eprintln!("Listening on 127.0.0.1:{} (no direct Wi-Fi access).", running.addr.port());
    }
    let mut _tunnels = Vec::new();
    if quick || (domain.is_some() && cfg.domain_tunnel) {
        eprintln!("Preparing cloudflared {}…", tunnel::CLOUDFLARED_VERSION);
        let bin = tunnel::ensure_cloudflared(&s.http, &config::config_dir()).await.map_err(|e| e.to_string())?;
        if quick {
            let t = tunnel::quick(&bin, running.addr.port()).await.map_err(|e| e.to_string())?;
            if let Some(u) = &t.url {
                urls.push(("Quick tunnel (new address every start)".into(), u.clone()));
            }
            _tunnels.push(t);
        }
        if let Some(d) = &domain {
            if cfg.domain_tunnel {
                let token = config::load_secret("mobile-tunnel-token").ok_or("No Cloudflare tunnel token saved. Run: blaze mobile domain-token")?;
                _tunnels.push(tunnel::named(&bin, &token).await.map_err(|e| e.to_string())?);
                eprintln!("Started your Cloudflare tunnel for {d}.");
            }
        }
    }
    if let Some(d) = &domain {
        let d = d.trim_end_matches('/').to_string();
        // Give a named tunnel a moment to connect before checking.
        tokio::time::sleep(std::time::Duration::from_secs(if cfg.domain_tunnel { 6 } else { 0 })).await;
        match tunnel::verify(&s.http, &d, &server.server_id).await {
            Ok(()) => urls.push(("Your domain".into(), format!("{d}/"))),
            Err(e) => eprintln!("warning: {e}. Point {d} at http://localhost:{} and try again.", running.addr.port()),
        }
    }
    if use_relay {
        let key = s.key.clone().ok_or("The BlazeAPI relay needs you signed in: run `blaze login`.")?;
        let slot = cfg.relay_slot.clone().unwrap_or_else(relay::new_slot);
        let base = relay::relay_base();
        urls.push(("BlazeAPI relay (works anywhere; BlazeAPI can see this traffic)".into(), relay::phone_url(&base, &slot)));
        let state_cb: Arc<dyn Fn(relay::RelayState) + Send + Sync> = Arc::new(|st| {
            if let relay::RelayState::Error(e) = st {
                eprintln!("relay: {e}");
            }
        });
        tokio::spawn(relay::run(s.http.clone(), base, key, slot, server.clone(), cancel.clone(), state_cb));
    }
    if urls.is_empty() {
        return Err("No address is reachable yet. Check the messages above.".into());
    }
    eprintln!();
    for (what, u) in &urls {
        eprintln!("  {u}\n    {what}");
    }
    let best = urls.iter().rev().find(|(w, _)| !w.starts_with("Same Wi-Fi")).or(urls.first()).map(|(_, u)| u.clone()).unwrap_or_default();
    let code = server.pair_code();
    eprintln!("\nScan with your phone's camera to sign in without typing the password (valid 5 minutes):\n");
    eprintln!("{}", remote::qr::terminal(&format!("{best}#pair={code}")));
    eprintln!("Or open {best} and enter your password. Press Ctrl+C to stop.\n");

    // BlBots routines and check-backs run while `blaze serve` runs (unless
    // Blaze IDE is open and already scheduling them). Approvals they need go
    // to... nobody here: refused, so routines should use edits/full bots.
    let (rh_tx, mut rh_rx) = tokio::sync::mpsc::channel::<blaze_core::agent::HostRequest>(4);
    tokio::spawn(async move {
        while let Some(r) = rh_rx.recv().await {
            match r {
                blaze_core::agent::HostRequest::Approve { reply, summary, .. } => {
                    eprintln!("routine: refused (nobody to ask): {summary}");
                    let _ = reply.send(false);
                }
                blaze_core::agent::HostRequest::Ask { reply, .. } => {
                    let _ = reply.send(vec![]);
                }
            }
        }
    });
    let on_sched: Arc<dyn Fn(blaze_core::bots::runner::SchedEvent) + Send + Sync> = Arc::new(|e| match e {
        blaze_core::bots::runner::SchedEvent::Started { title, .. } => eprintln!("bots: running \"{title}\""),
        blaze_core::bots::runner::SchedEvent::Finished { title, error, .. } => eprintln!("bots: finished \"{title}\"{}", error.map(|e| format!(" with an error: {e}")).unwrap_or_default()),
    });
    tokio::spawn(blaze_core::bots::runner::scheduler(Arc::new(host.bots.clone()), rh_tx, cancel.clone(), on_sched));

    // Show what phones do, until Ctrl+C.
    let srv = server.clone();
    let watch = tokio::spawn(async move {
        let mut seq = srv.events_after(u64::MAX).0;
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            let (next, evs) = srv.events_after(seq);
            seq = next;
            for e in evs {
                match e["type"].as_str() {
                    Some("run_start") => eprintln!("phone: started \"{}\" ({})", e["title"].as_str().unwrap_or(""), e["model"].as_str().unwrap_or("")),
                    Some("pending") => eprintln!("phone: waiting for an answer on the phone: {}", e["item"]["summary"].as_str().unwrap_or("a question")),
                    Some("run_end") => eprintln!("phone: finished \"{}\"{}", e["title"].as_str().unwrap_or(""), e["error"].as_str().map(|x| format!(" with an error: {x}")).unwrap_or_default()),
                    _ => {}
                }
            }
        }
    });
    let _ = tokio::signal::ctrl_c().await;
    watch.abort();
    cancel.cancel();
    drop(running);
    eprintln!("Stopped. Phones cannot reach this computer now.");
    Ok(())
}

pub fn devices(store: &Store) {
    let cfg = auth::load(store);
    if cfg.devices.is_empty() {
        println!("No phones are signed in.");
    }
    for d in cfg.devices {
        let ago = auth::now_secs().saturating_sub(d.last_seen) / 60;
        println!("{:<14} {:<22} last seen {} min ago", d.id, d.name, ago);
    }
}
