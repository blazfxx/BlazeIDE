//! Continue on mobile: a small web server inside Blaze IDE or `blaze serve`
//! that a phone uses to continue chats and answer the agent's approval
//! prompts.
//!
//! What the phone can do: list the projects this machine opened, list and
//! read their chats, send a message (the turn runs here with the Rust agent),
//! stop it, and approve or refuse actions. Nothing else: no file browser, no
//! terminal, no settings.
//!
//! Ways in (see `tunnel.rs`, `relay.rs`): the local network, the user's own
//! domain, a free Cloudflare quick tunnel, or the BlazeAPI relay. All of them
//! end at the same router; every request needs the session cookie except the
//! sign-in calls and the static app.

pub mod auth;
pub mod qr;
pub mod relay;
pub mod tunnel;

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{ConnectInfo, Query, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, Notify};
use tokio_util::sync::CancellationToken;

use crate::agent::{self, AgentConfig, AgentEvent, Backend, HostRequest};
use crate::ai::ChatMessage;
use crate::chatfile::ChatFile;
use crate::models::ModelEntry;
use crate::store::Store;

pub use auth::MobileConfig;

/// What the IDE or the CLI provides to the phone server.
pub trait Host: Send + Sync + 'static {
    fn store(&self) -> &Store;
    fn backend(&self) -> Backend;
    /// The agent settings for a phone turn (approval level, media, ...).
    fn agent_config(&self, root: &Path, model: &str, mode: &str) -> AgentConfig;
    /// Project folders the phone may use (the ones opened on this machine).
    fn projects(&self) -> Vec<PathBuf>;
    fn models(&self) -> Vec<ModelEntry>;
    fn default_model(&self) -> String;
    /// "Blaze IDE on Alexs-MacBook-Air".
    fn label(&self) -> String;
    /// BlBots, when this host runs them.
    fn bots(&self) -> Option<crate::bots::runner::BotCtx> {
        None
    }
}

const COOKIE: &str = "bz_s";
const MAX_RUNS: usize = 3;
const LOG_KEEP: usize = 3000;
const POLL_WAIT: Duration = Duration::from_secs(25);

const INDEX_HTML: &str = include_str!("web/index.html");
const APP_JS: &str = include_str!("web/app.js");
const APP_CSS: &str = include_str!("web/app.css");
const MANIFEST: &str = include_str!("web/manifest.json");
const ICON_SVG: &str = include_str!("web/icon.svg");

/// Client address and transport, filled in per request (relay requests carry
/// their own).
#[derive(Clone, Debug)]
pub struct ClientInfo {
    pub ip: String,
    pub https: bool,
    /// Path prefix the phone sees (`/r/<slot>` behind the relay).
    pub prefix: String,
}

struct Pending {
    run_id: String,
    view: Value,
    reply: PendingReply,
}

enum PendingReply {
    Approve(oneshot::Sender<bool>),
    Ask(oneshot::Sender<Vec<String>>),
}

enum BotJob {
    Thread { bot: String, thread: String, text: String },
    Room { room: String, text: String },
}

struct RunState {
    chat_key: String,
    cancel: CancellationToken,
}

pub struct Server {
    host: Arc<dyn Host>,
    guard: auth::Guard,
    runs: Mutex<HashMap<String, RunState>>,
    pending: Mutex<HashMap<String, Pending>>,
    log: Mutex<(u64, VecDeque<(u64, Value)>)>,
    wake: Notify,
    /// Public id answered by /healthz (lets the app check a custom domain
    /// really reaches this machine). Not a secret.
    pub server_id: String,
}

fn canon(p: &Path) -> Option<PathBuf> {
    dunce::canonicalize(p).ok()
}

impl Server {
    pub fn new(host: Arc<dyn Host>) -> Arc<Self> {
        let server_id = auth::sha256_hex(&format!("blaze-mobile:{}", host.store().install_id()))[..16].to_string();
        Arc::new(Server {
            host,
            guard: auth::Guard::default(),
            runs: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
            log: Mutex::new((0, VecDeque::new())),
            wake: Notify::new(),
            server_id,
        })
    }

    pub fn host(&self) -> &Arc<dyn Host> {
        &self.host
    }

    /// One-time code for a pairing QR (5 minutes).
    pub fn pair_code(&self) -> String {
        self.guard.new_pair_code()
    }

    fn push(&self, ev: Value) {
        let mut log = self.log.lock().expect("log");
        log.0 += 1;
        let seq = log.0;
        log.1.push_back((seq, ev));
        while log.1.len() > LOG_KEEP {
            log.1.pop_front();
        }
        drop(log);
        self.wake.notify_waiters();
    }

    /// Events after `after` and the newest sequence number.
    pub fn events_after(&self, after: u64) -> (u64, Vec<Value>) {
        let log = self.log.lock().expect("log");
        let evs = log.1.iter().filter(|(s, _)| *s > after).map(|(s, v)| {
            let mut v = v.clone();
            v["seq"] = json!(s);
            v
        });
        (log.0, evs.collect())
    }

    fn allowed_root(&self, raw: &str) -> Option<PathBuf> {
        let want = canon(Path::new(raw))?;
        self.host.projects().iter().filter_map(|p| canon(p)).find(|p| *p == want)
    }

    pub fn active_runs(&self) -> usize {
        self.runs.lock().expect("runs").len()
    }

    /// Start a turn on a chat. Returns (chat id, run id).
    fn start_run(self: &Arc<Self>, root: PathBuf, chat_id: Option<String>, text: String, model: Option<String>, mode: Option<String>) -> Result<(String, String), String> {
        let mut chat = match chat_id.as_deref().filter(|s| !s.is_empty()) {
            Some(id) => ChatFile::load(&root, id)?,
            None => ChatFile::new(&root, "agent", &self.host.default_model()),
        };
        if crate::chats::is_live(chat.extra.get("live")) {
            return Err("Blaze IDE is working on this chat on your computer. Wait for it to finish, or stop it there.".into());
        }
        chat.extra.remove("live");
        if let Some(m) = model.filter(|m| !m.is_empty()) {
            chat.model = m;
        }
        if let Some(m) = mode.filter(|m| matches!(m.as_str(), "ask" | "plan" | "agent")) {
            chat.mode = m;
        }
        let chat_key = format!("{}|{}", root.display(), chat.id);
        {
            let runs = self.runs.lock().expect("runs");
            if runs.values().any(|r| r.chat_key == chat_key) {
                return Err("This chat is already working. Wait, or stop it first.".into());
            }
            if runs.len() >= MAX_RUNS {
                return Err(format!("{MAX_RUNS} chats are already working on this computer. Wait for one to finish."));
            }
        }
        let user_index = chat.messages.len();
        chat.messages.push(ChatMessage { role: "user".into(), content: text, ..Default::default() });
        chat.save()?;
        let run_id = format!("r{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
        let cancel = CancellationToken::new();
        self.runs.lock().expect("runs").insert(run_id.clone(), RunState { chat_key, cancel: cancel.clone() });
        let chat_id = chat.id.clone();
        let root_s = root.display().to_string();
        self.push(json!({ "type": "run_start", "runId": run_id, "chatId": chat_id, "root": root_s, "title": chat.title, "model": chat.model }));

        let me = self.clone();
        let rid = run_id.clone();
        tokio::spawn(async move {
            let backend = me.host.backend();
            let cfg = me.host.agent_config(&root, &chat.model, &chat.mode);
            let (etx, mut erx) = mpsc::unbounded_channel::<AgentEvent>();
            let (htx, mut hrx) = mpsc::channel::<HostRequest>(4);
            // Agent events -> log.
            let me2 = me.clone();
            let (rid2, cid2) = (rid.clone(), chat.id.clone());
            let fwd = tokio::spawn(async move {
                while let Some(ev) = erx.recv().await {
                    me2.push(json!({ "type": "agent", "runId": rid2, "chatId": cid2, "ev": ev }));
                }
            });
            // Approvals and questions -> pending cards the phone answers.
            let me3 = me.clone();
            let (rid3, cid3) = (rid.clone(), chat.id.clone());
            let asks = tokio::spawn(async move {
                while let Some(req) = hrx.recv().await {
                    let id = format!("p{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
                    let (view, reply) = match req {
                        HostRequest::Approve { call, summary, reply } => {
                            (json!({ "id": id, "kind": "approve", "runId": rid3, "chatId": cid3, "tool": call.name, "summary": summary }), PendingReply::Approve(reply))
                        }
                        HostRequest::Ask { questions, reply } => {
                            let qs: Vec<Value> = questions.iter().map(|(q, o)| json!({ "question": q, "options": o })).collect();
                            (json!({ "id": id, "kind": "ask", "runId": rid3, "chatId": cid3, "questions": qs }), PendingReply::Ask(reply))
                        }
                    };
                    me3.pending.lock().expect("pending").insert(id.clone(), Pending { run_id: rid3.clone(), view: view.clone(), reply });
                    me3.push(json!({ "type": "pending", "item": view }));
                }
            });
            let out = agent::run_turn(&backend, &cfg, &mut chat.messages, &etx, &htx, &cancel).await;
            drop(etx);
            drop(htx);
            let _ = fwd.await;
            asks.abort();
            // Unanswered cards of this run are dropped (the agent already moved on).
            me.pending.lock().expect("pending").retain(|_, p| p.run_id != rid);
            chat.add_checkpoint(user_index, &out.changed);
            let saved = chat.save();
            me.runs.lock().expect("runs").remove(&rid);
            me.push(json!({
                "type": "run_end", "runId": rid, "chatId": chat.id, "stopped": out.stopped,
                "error": out.error.or_else(|| saved.err()), "title": chat.title,
            }));
        });
        Ok((chat_id, run_id))
    }

    /// Run a bot turn or a room turn in the background, with the same events
    /// and approval cards as project chats. `chat_id` is what the phone
    /// follows (a thread id, or "room:<id>").
    fn start_bot_run(self: &Arc<Self>, chat_key: String, chat_id: String, title: String, job: BotJob) -> Result<String, String> {
        let ctx = self.host.bots().ok_or("BlBots are not available here")?;
        {
            let runs = self.runs.lock().expect("runs");
            if runs.values().any(|r| r.chat_key == chat_key) {
                return Err("Already working. Wait, or stop it first.".into());
            }
            if runs.len() >= MAX_RUNS {
                return Err(format!("{MAX_RUNS} chats are already working on this computer. Wait for one to finish."));
            }
        }
        let run_id = format!("r{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
        let cancel = CancellationToken::new();
        self.runs.lock().expect("runs").insert(run_id.clone(), RunState { chat_key, cancel: cancel.clone() });
        self.push(json!({ "type": "run_start", "runId": run_id, "chatId": chat_id, "title": title }));
        let me = self.clone();
        let rid = run_id.clone();
        tokio::spawn(async move {
            let (htx, mut hrx) = mpsc::channel::<HostRequest>(4);
            let me3 = me.clone();
            let (rid3, cid3) = (rid.clone(), chat_id.clone());
            let asks = tokio::spawn(async move {
                while let Some(req) = hrx.recv().await {
                    let id = format!("p{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
                    let (view, reply) = match req {
                        HostRequest::Approve { call, summary, reply } => (json!({ "id": id, "kind": "approve", "runId": rid3, "chatId": cid3, "tool": call.name, "summary": summary }), PendingReply::Approve(reply)),
                        HostRequest::Ask { questions, reply } => {
                            let qs: Vec<Value> = questions.iter().map(|(q, o)| json!({ "question": q, "options": o })).collect();
                            (json!({ "id": id, "kind": "ask", "runId": rid3, "chatId": cid3, "questions": qs }), PendingReply::Ask(reply))
                        }
                    };
                    me3.pending.lock().expect("pending").insert(id.clone(), Pending { run_id: rid3.clone(), view: view.clone(), reply });
                    me3.push(json!({ "type": "pending", "item": view }));
                }
            });
            let error = match job {
                BotJob::Thread { bot, thread, text } => {
                    let (etx, mut erx) = mpsc::unbounded_channel::<AgentEvent>();
                    let me2 = me.clone();
                    let (rid2, cid2, b2) = (rid.clone(), chat_id.clone(), bot.clone());
                    let fwd = tokio::spawn(async move {
                        while let Some(ev) = erx.recv().await {
                            me2.push(json!({ "type": "agent", "runId": rid2, "chatId": cid2, "bot": b2, "ev": ev }));
                        }
                    });
                    let r = crate::bots::runner::thread_turn(&ctx, &bot, Some(&thread), &text, &etx, &htx, &cancel).await;
                    drop(etx);
                    let _ = fwd.await;
                    match r {
                        Ok((_, out)) => out.error,
                        Err(e) => Some(e),
                    }
                }
                BotJob::Room { room, text } => {
                    let (etx, mut erx) = mpsc::unbounded_channel::<(String, AgentEvent)>();
                    let me2 = me.clone();
                    let (rid2, cid2) = (rid.clone(), chat_id.clone());
                    let fwd = tokio::spawn(async move {
                        while let Some((bot, ev)) = erx.recv().await {
                            me2.push(json!({ "type": "agent", "runId": rid2, "chatId": cid2, "bot": bot, "ev": ev }));
                        }
                    });
                    let r = crate::bots::runner::room_turn(&ctx, &room, &text, &etx, &htx, &cancel).await;
                    drop(etx);
                    let _ = fwd.await;
                    r.err()
                }
            };
            drop(htx);
            asks.abort();
            me.pending.lock().expect("pending").retain(|_, p| p.run_id != rid);
            me.runs.lock().expect("runs").remove(&rid);
            me.push(json!({ "type": "run_end", "runId": rid, "chatId": chat_id, "stopped": cancel.is_cancelled(), "error": error }));
        });
        Ok(run_id)
    }

    pub fn router(self: &Arc<Self>) -> Router {
        let api = Router::new()
            .route("/api/me", get(me))
            .route("/api/projects", get(projects))
            .route("/api/chats", get(chats))
            .route("/api/chat", get(chat))
            .route("/api/models", get(models))
            .route("/api/send", post(send))
            .route("/api/stop", post(stop))
            .route("/api/answer", post(answer))
            .route("/api/events", get(events))
            .route("/api/logout", post(logout))
            .route("/api/bots", get(bots_list))
            .route("/api/bot/threads", get(bot_threads))
            .route("/api/bot/thread", get(bot_thread))
            .route("/api/bot/send", post(bot_send))
            .route("/api/room", get(room_get))
            .route("/api/room/send", post(room_send))
            .route_layer(middleware::from_fn_with_state(self.clone(), require_session));
        Router::new()
            .route("/", get(|| async { asset(INDEX_HTML, "text/html; charset=utf-8") }))
            .route("/index.html", get(|| async { asset(INDEX_HTML, "text/html; charset=utf-8") }))
            .route("/app.js", get(|| async { asset(APP_JS, "text/javascript; charset=utf-8") }))
            .route("/app.css", get(|| async { asset(APP_CSS, "text/css; charset=utf-8") }))
            .route("/manifest.json", get(|| async { asset(MANIFEST, "application/manifest+json") }))
            .route("/icon.svg", get(|| async { asset(ICON_SVG, "image/svg+xml") }))
            .route("/healthz", get(healthz))
            .route("/api/login", post(login))
            .route("/api/pair", post(pair))
            .merge(api)
            .layer(middleware::from_fn(guard_requests))
            .layer(middleware::from_fn(client_info))
            .with_state(self.clone())
    }
}

fn asset(body: &'static str, ctype: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, ctype),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        body,
    )
        .into_response()
}

fn ok(v: Value) -> Response {
    Json(v).into_response()
}

fn fail(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "error": msg }))).into_response()
}

/// Works out the client's address. Requests from this machine that carry a
/// tunnel's client header (Cloudflare) use that address; relay requests
/// arrive with ClientInfo already set.
async fn client_info(mut req: Request, next: Next) -> Response {
    if req.extensions().get::<ClientInfo>().is_none() {
        let peer = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0);
        let h = req.headers();
        let via_tunnel = peer.is_some_and(|p| p.ip().is_loopback());
        let forwarded = h.get("cf-connecting-ip").and_then(|v| v.to_str().ok()).map(str::to_string);
        let ip = match (via_tunnel, forwarded) {
            (true, Some(f)) => f,
            _ => peer.map(|p| p.ip().to_string()).unwrap_or_else(|| "?".into()),
        };
        let https = via_tunnel
            && (h.get("x-forwarded-proto").and_then(|v| v.to_str().ok()) == Some("https")
                || h.get("cf-visitor").and_then(|v| v.to_str().ok()).is_some_and(|v| v.contains("https")));
        req.extensions_mut().insert(ClientInfo { ip, https, prefix: String::new() });
    }
    next.run(req).await
}

/// Security headers on everything; requests that change something must come
/// from the app itself (custom header + same origin), so another site cannot
/// drive a signed-in phone's browser.
async fn guard_requests(req: Request, next: Next) -> Response {
    if req.method() != Method::GET && req.method() != Method::HEAD {
        let h = req.headers();
        if h.get("x-blaze").and_then(|v| v.to_str().ok()) != Some("1") {
            return fail(StatusCode::FORBIDDEN, "Missing request header");
        }
        if let Some(origin) = h.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
            let host = h.get("x-forwarded-host").or_else(|| h.get(header::HOST)).and_then(|v| v.to_str().ok()).unwrap_or("");
            let o_host = origin.split("://").nth(1).unwrap_or("");
            if o_host != host {
                return fail(StatusCode::FORBIDDEN, "Cross-site request refused");
            }
        }
    }
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert("content-security-policy", HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; manifest-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'"));
    h.insert("x-content-type-options", HeaderValue::from_static("nosniff"));
    h.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    if !h.contains_key(header::CACHE_CONTROL) {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    res
}

fn cookie_token(h: &HeaderMap) -> Option<String> {
    let raw = h.get_all(header::COOKIE).iter().filter_map(|v| v.to_str().ok()).collect::<Vec<_>>().join("; ");
    raw.split(';').map(str::trim).find_map(|c| c.strip_prefix(&format!("{COOKIE}="))).map(str::to_string)
}

fn session_cookie(info: &ClientInfo, token: &str, max_age: u64) -> String {
    let path = if info.prefix.is_empty() { "/".to_string() } else { format!("{}/", info.prefix.trim_end_matches('/')) };
    format!("{COOKIE}={token}; Path={path}; Max-Age={max_age}; HttpOnly; SameSite=Strict{}", if info.https { "; Secure" } else { "" })
}

async fn require_session(State(s): State<Arc<Server>>, req: Request, next: Next) -> Response {
    let token = cookie_token(req.headers()).unwrap_or_default();
    match auth::check_session(s.host.store(), &token) {
        Some(_) => next.run(req).await,
        None => fail(StatusCode::UNAUTHORIZED, "Sign in again"),
    }
}

async fn healthz(State(s): State<Arc<Server>>) -> Response {
    ok(json!({ "ok": true, "app": "blaze-mobile", "id": s.server_id }))
}

#[derive(Deserialize)]
struct LoginBody {
    password: String,
}

async fn login(State(s): State<Arc<Server>>, req: Request) -> Response {
    let info = req.extensions().get::<ClientInfo>().cloned().unwrap_or(ClientInfo { ip: "?".into(), https: false, prefix: String::new() });
    let ua = req.headers().get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let Ok(body) = axum::body::to_bytes(req.into_body(), 4096).await else { return fail(StatusCode::BAD_REQUEST, "Bad request") };
    let Ok(LoginBody { password }) = serde_json::from_slice(&body) else { return fail(StatusCode::BAD_REQUEST, "Bad request") };
    if let Err(m) = s.guard.allowed(&info.ip) {
        return fail(StatusCode::TOO_MANY_REQUESTS, &m);
    }
    let cfg = auth::load(s.host.store());
    let Some(hash) = cfg.password_hash.filter(|_| cfg.enabled) else { return fail(StatusCode::FORBIDDEN, "Continue on mobile is turned off on this computer") };
    // argon2 is deliberately slow: keep it off the async threads.
    let good = tokio::task::spawn_blocking(move || auth::verify_password(&hash, &password)).await.unwrap_or(false);
    if !good {
        s.guard.failed(&info.ip);
        tokio::time::sleep(Duration::from_millis(400)).await;
        return fail(StatusCode::UNAUTHORIZED, "Wrong password");
    }
    signed_in(&s, &info, &ua)
}

fn signed_in(s: &Server, info: &ClientInfo, ua: &str) -> Response {
    match auth::new_session(s.host.store(), ua) {
        Ok(token) => {
            let mut res = ok(json!({ "ok": true, "name": s.host.label() }));
            if let Ok(v) = HeaderValue::from_str(&session_cookie(info, &token, auth::SESSION_DAYS * 86400)) {
                res.headers_mut().insert(header::SET_COOKIE, v);
            }
            res
        }
        Err(e) => fail(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

#[derive(Deserialize)]
struct PairBody {
    code: String,
}

async fn pair(State(s): State<Arc<Server>>, req: Request) -> Response {
    let info = req.extensions().get::<ClientInfo>().cloned().unwrap_or(ClientInfo { ip: "?".into(), https: false, prefix: String::new() });
    let ua = req.headers().get(header::USER_AGENT).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let Ok(body) = axum::body::to_bytes(req.into_body(), 4096).await else { return fail(StatusCode::BAD_REQUEST, "Bad request") };
    let Ok(PairBody { code }) = serde_json::from_slice(&body) else { return fail(StatusCode::BAD_REQUEST, "Bad request") };
    if let Err(m) = s.guard.allowed(&info.ip) {
        return fail(StatusCode::TOO_MANY_REQUESTS, &m);
    }
    if !auth::load(s.host.store()).enabled || !s.guard.take_pair_code(&code) {
        s.guard.failed(&info.ip);
        return fail(StatusCode::UNAUTHORIZED, "This pairing code expired. Show a new QR code on your computer, or sign in with the password.");
    }
    signed_in(&s, &info, &ua)
}

async fn logout(State(s): State<Arc<Server>>, req: Request) -> Response {
    let info = req.extensions().get::<ClientInfo>().cloned().unwrap_or(ClientInfo { ip: "?".into(), https: false, prefix: String::new() });
    if let Some(tok) = cookie_token(req.headers()) {
        if let Some(d) = auth::check_session(s.host.store(), &tok) {
            let _ = auth::revoke_device(s.host.store(), &d.id);
        }
    }
    let mut res = ok(json!({ "ok": true }));
    if let Ok(v) = HeaderValue::from_str(&session_cookie(&info, "", 0)) {
        res.headers_mut().insert(header::SET_COOKIE, v);
    }
    res
}

async fn me(State(s): State<Arc<Server>>, headers: HeaderMap) -> Response {
    let dev = cookie_token(&headers).and_then(|t| auth::check_session(s.host.store(), &t));
    ok(json!({ "name": s.host.label(), "device": dev.map(|d| d.name), "defaultModel": s.host.default_model() }))
}

async fn projects(State(s): State<Arc<Server>>) -> Response {
    let list: Vec<Value> = s
        .host
        .projects()
        .iter()
        .filter(|p| p.is_dir())
        .map(|p| json!({ "root": p.display().to_string(), "name": p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string()) }))
        .collect();
    ok(json!({ "projects": list }))
}

#[derive(Deserialize)]
struct RootQ {
    root: String,
    #[serde(default)]
    id: Option<String>,
}

async fn chats(State(s): State<Arc<Server>>, Query(q): Query<RootQ>) -> Response {
    let Some(root) = s.allowed_root(&q.root) else { return fail(StatusCode::NOT_FOUND, "Unknown project") };
    let running: Vec<String> = s.runs.lock().expect("runs").values().map(|r| r.chat_key.clone()).collect();
    let list: Vec<Value> = crate::chats::list(&root)
        .into_iter()
        .take(100)
        .map(|c| {
            let key = format!("{}|{}", root.display(), c.id);
            json!({ "id": c.id, "title": c.title, "updatedAt": c.updated_at, "messages": c.message_count, "working": c.live || running.contains(&key), "live": c.live })
        })
        .collect();
    ok(json!({ "chats": list }))
}

/// The IDE puts the project's file list in front of the first message.
fn without_context(text: &str) -> &str {
    match (text.trim_start().starts_with("<environment_details>"), text.find("</environment_details>")) {
        (true, Some(end)) => text[end + "</environment_details>".len()..].trim_start(),
        _ => text,
    }
}

/// A chat for the phone: user and assistant text plus one line per tool call.
pub fn phone_view(messages: &[ChatMessage]) -> Vec<Value> {
    let mut out = Vec::new();
    for m in messages {
        match m.role.as_str() {
            "user" if !m.content.starts_with("[Screenshot") && !m.content.starts_with("[Browser screenshot") => out.push(json!({ "role": "user", "text": without_context(&m.content) })),
            "assistant" => {
                let tools: Vec<String> = m.tool_calls.iter().flatten().map(agent::summarize).collect();
                if !m.content.trim().is_empty() || !tools.is_empty() {
                    out.push(json!({ "role": "assistant", "text": m.content, "tools": tools }));
                }
            }
            _ => {}
        }
    }
    out
}

async fn chat(State(s): State<Arc<Server>>, Query(q): Query<RootQ>) -> Response {
    let Some(root) = s.allowed_root(&q.root) else { return fail(StatusCode::NOT_FOUND, "Unknown project") };
    let Some(id) = q.id else { return fail(StatusCode::BAD_REQUEST, "Missing chat id") };
    match ChatFile::load(&root, &id) {
        Ok(c) => {
            let key = format!("{}|{}", root.display(), c.id);
            let run = s.runs.lock().expect("runs").iter().find(|(_, r)| r.chat_key == key).map(|(id, _)| id.clone());
            let pending: Vec<Value> = s.pending.lock().expect("pending").values().filter(|p| p.view["chatId"] == c.id.as_str()).map(|p| p.view.clone()).collect();
            let live = crate::chats::is_live(c.extra.get("live"));
            ok(json!({ "live": live, "id": c.id, "title": c.title, "model": c.model, "mode": c.mode, "messages": phone_view(&c.messages), "runId": run, "pending": pending, "seq": s.log.lock().expect("log").0 }))
        }
        Err(e) => fail(StatusCode::NOT_FOUND, &e),
    }
}

async fn models(State(s): State<Arc<Server>>) -> Response {
    let list: Vec<Value> = s
        .host
        .models()
        .iter()
        .filter(|m| m.kind == "chat")
        .map(|m| json!({ "id": m.id, "name": m.display_name, "free": m.free, "in": m.price_in, "out": m.price_out }))
        .collect();
    ok(json!({ "models": list, "default": s.host.default_model() }))
}

#[derive(Deserialize)]
struct SendBody {
    root: String,
    #[serde(default, rename = "chatId")]
    chat_id: Option<String>,
    text: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    mode: Option<String>,
}

async fn send(State(s): State<Arc<Server>>, Json(b): Json<SendBody>) -> Response {
    let text = b.text.trim().to_string();
    if text.is_empty() || text.len() > 100_000 {
        return fail(StatusCode::BAD_REQUEST, "Write a message first");
    }
    let Some(root) = s.allowed_root(&b.root) else { return fail(StatusCode::NOT_FOUND, "Unknown project") };
    match s.start_run(root, b.chat_id, text, b.model, b.mode) {
        Ok((chat_id, run_id)) => ok(json!({ "chatId": chat_id, "runId": run_id })),
        Err(e) => fail(StatusCode::CONFLICT, &e),
    }
}

#[derive(Deserialize)]
struct StopBody {
    #[serde(rename = "runId")]
    run_id: String,
}

async fn stop(State(s): State<Arc<Server>>, Json(b): Json<StopBody>) -> Response {
    if let Some(r) = s.runs.lock().expect("runs").get(&b.run_id) {
        r.cancel.cancel();
    }
    ok(json!({ "ok": true }))
}

#[derive(Deserialize)]
struct AnswerBody {
    id: String,
    #[serde(default)]
    allow: Option<bool>,
    #[serde(default)]
    answers: Option<Vec<String>>,
}

async fn answer(State(s): State<Arc<Server>>, Json(b): Json<AnswerBody>) -> Response {
    let Some(p) = s.pending.lock().expect("pending").remove(&b.id) else { return fail(StatusCode::GONE, "Already answered") };
    match p.reply {
        PendingReply::Approve(tx) => {
            let _ = tx.send(b.allow.unwrap_or(false));
        }
        PendingReply::Ask(tx) => {
            let _ = tx.send(b.answers.unwrap_or_default().into_iter().map(|a| a.chars().take(4000).collect()).collect());
        }
    }
    s.push(json!({ "type": "answered", "id": b.id }));
    ok(json!({ "ok": true }))
}

#[derive(Deserialize)]
struct EventsQ {
    #[serde(default)]
    after: u64,
}

/// Long poll: returns new events at once, or after up to 25 s.
async fn events(State(s): State<Arc<Server>>, Query(q): Query<EventsQ>) -> Response {
    let wait = s.wake.notified();
    let (seq, evs) = s.events_after(q.after);
    if !evs.is_empty() || q.after > seq {
        return ok(json!({ "seq": seq, "events": evs }));
    }
    let _ = tokio::time::timeout(POLL_WAIT, wait).await;
    // Give a burst of streamed text a moment to collect into one answer.
    tokio::time::sleep(Duration::from_millis(60)).await;
    let (seq, evs) = s.events_after(q.after);
    ok(json!({ "seq": seq, "events": evs }))
}

fn bot_view(b: &crate::bots::Bot) -> Value {
    json!({ "id": b.id, "name": b.name, "avatar": b.avatar, "color": b.color, "title": b.title, "engine": b.engine.label(), "model": b.model })
}

async fn bots_list(State(s): State<Arc<Server>>) -> Response {
    let Some(ctx) = s.host.bots() else { return ok(json!({ "bots": [], "rooms": [] })) };
    let bots: Vec<Value> = ctx.bots.list().iter().filter(|b| !b.hidden).map(bot_view).collect();
    let rooms: Vec<Value> = ctx.rooms.list().iter().map(|r| json!({ "id": r.id, "name": r.name, "members": r.members, "updated": r.updated })).collect();
    ok(json!({ "bots": bots, "rooms": rooms }))
}

#[derive(Deserialize)]
struct BotQ {
    bot: String,
    #[serde(default)]
    id: Option<String>,
}

async fn bot_threads(State(s): State<Arc<Server>>, Query(q): Query<BotQ>) -> Response {
    let Some(ctx) = s.host.bots() else { return fail(StatusCode::NOT_FOUND, "No bots") };
    let Some(bot) = ctx.bots.get(&q.bot) else { return fail(StatusCode::NOT_FOUND, "No such bot") };
    let running: Vec<String> = s.runs.lock().expect("runs").values().map(|r| r.chat_key.clone()).collect();
    let list: Vec<Value> = crate::chats::list_in(&ctx.bots.threads_dir(&bot.id))
        .into_iter()
        .take(100)
        .map(|c| json!({ "id": c.id, "title": c.title, "updatedAt": c.updated_at, "messages": c.message_count, "working": running.contains(&format!("bot|{}|{}", bot.id, c.id)) }))
        .collect();
    ok(json!({ "bot": bot_view(&bot), "threads": list }))
}

async fn bot_thread(State(s): State<Arc<Server>>, Query(q): Query<BotQ>) -> Response {
    let Some(ctx) = s.host.bots() else { return fail(StatusCode::NOT_FOUND, "No bots") };
    let Some(bot) = ctx.bots.get(&q.bot) else { return fail(StatusCode::NOT_FOUND, "No such bot") };
    let Some(id) = q.id else { return fail(StatusCode::BAD_REQUEST, "Missing thread id") };
    match crate::bots::runner::load_thread(&ctx, &bot, &id) {
        Ok(c) => {
            let key = format!("bot|{}|{}", bot.id, c.id);
            let run = s.runs.lock().expect("runs").iter().find(|(_, r)| r.chat_key == key).map(|(id, _)| id.clone());
            let pending: Vec<Value> = s.pending.lock().expect("pending").values().filter(|p| p.view["chatId"] == c.id.as_str()).map(|p| p.view.clone()).collect();
            ok(json!({ "id": c.id, "title": c.title, "messages": phone_view(&c.messages), "runId": run, "pending": pending, "seq": s.log.lock().expect("log").0, "bot": bot_view(&bot) }))
        }
        Err(e) => fail(StatusCode::NOT_FOUND, &e),
    }
}

#[derive(Deserialize)]
struct BotSend {
    bot: String,
    #[serde(default)]
    thread: Option<String>,
    text: String,
}

async fn bot_send(State(s): State<Arc<Server>>, Json(b): Json<BotSend>) -> Response {
    let text = b.text.trim().to_string();
    if text.is_empty() || text.len() > 100_000 {
        return fail(StatusCode::BAD_REQUEST, "Write a message first");
    }
    let Some(ctx) = s.host.bots() else { return fail(StatusCode::NOT_FOUND, "No bots") };
    let Some(bot) = ctx.bots.get(&b.bot) else { return fail(StatusCode::NOT_FOUND, "No such bot") };
    let thread = b.thread.filter(|t| !t.is_empty()).unwrap_or_else(|| format!("c{}", &uuid::Uuid::new_v4().simple().to_string()[..12]));
    match s.start_bot_run(format!("bot|{}|{}", bot.id, thread), thread.clone(), bot.name.clone(), BotJob::Thread { bot: bot.id.clone(), thread: thread.clone(), text }) {
        Ok(run) => ok(json!({ "chatId": thread, "runId": run })),
        Err(e) => fail(StatusCode::CONFLICT, &e),
    }
}

#[derive(Deserialize)]
struct RoomQ {
    id: String,
}

async fn room_get(State(s): State<Arc<Server>>, Query(q): Query<RoomQ>) -> Response {
    let Some(ctx) = s.host.bots() else { return fail(StatusCode::NOT_FOUND, "No bots") };
    let Some(r) = ctx.rooms.get(&q.id) else { return fail(StatusCode::NOT_FOUND, "No such room") };
    let roster = ctx.bots.list();
    let names: HashMap<String, String> = roster.iter().map(|b| (b.id.clone(), b.name.clone())).collect();
    let chat_id = format!("room:{}", r.id);
    let run = s.runs.lock().expect("runs").iter().find(|(_, x)| x.chat_key == chat_id).map(|(id, _)| id.clone());
    let pending: Vec<Value> = s.pending.lock().expect("pending").values().filter(|p| p.view["chatId"] == chat_id.as_str()).map(|p| p.view.clone()).collect();
    let msgs: Vec<Value> = r
        .messages
        .iter()
        .map(|m| if m.author == "user" { json!({ "role": "user", "text": m.text }) } else { json!({ "role": "assistant", "text": m.text, "tools": m.tools, "author": names.get(&m.author).cloned().unwrap_or_else(|| "bot".into()) }) })
        .collect();
    let members: Vec<Value> = roster.iter().filter(|b| r.members.contains(&b.id)).map(bot_view).collect();
    ok(json!({ "id": chat_id, "title": r.name, "messages": msgs, "runId": run, "pending": pending, "seq": s.log.lock().expect("log").0, "members": members }))
}

#[derive(Deserialize)]
struct RoomSend {
    room: String,
    text: String,
}

async fn room_send(State(s): State<Arc<Server>>, Json(b): Json<RoomSend>) -> Response {
    let text = b.text.trim().to_string();
    if text.is_empty() || text.len() > 100_000 {
        return fail(StatusCode::BAD_REQUEST, "Write a message first");
    }
    let Some(ctx) = s.host.bots() else { return fail(StatusCode::NOT_FOUND, "No bots") };
    let Some(r) = ctx.rooms.get(&b.room) else { return fail(StatusCode::NOT_FOUND, "No such room") };
    let chat_id = format!("room:{}", r.id);
    match s.start_bot_run(chat_id.clone(), chat_id.clone(), r.name.clone(), BotJob::Room { room: r.id.clone(), text }) {
        Ok(run) => ok(json!({ "chatId": chat_id, "runId": run })),
        Err(e) => fail(StatusCode::CONFLICT, &e),
    }
}

/// A running phone server.
pub struct Running {
    pub server: Arc<Server>,
    pub addr: SocketAddr,
    pub cancel: CancellationToken,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Bind and serve. `lan` = listen on every interface (the phone connects
/// directly over Wi-Fi); otherwise only 127.0.0.1 (tunnels and the relay
/// still work).
pub async fn serve(server: Arc<Server>, port: u16, lan: bool) -> crate::Result<Running> {
    let ip = if lan { [0, 0, 0, 0] } else { [127, 0, 0, 1] };
    let listener = tokio::net::TcpListener::bind(SocketAddr::from((ip, port)))
        .await
        .map_err(|e| crate::Error::msg(format!("Could not use port {port}: {e}. Pick another port.")))?;
    let addr = listener.local_addr()?;
    let cancel = CancellationToken::new();
    let app = server.router().into_make_service_with_connect_info::<SocketAddr>();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).with_graceful_shutdown(async move { c2.cancelled().await }).await;
    });
    Ok(Running { server, addr, cancel })
}

/// Hand one request to the router (used by the relay client).
pub async fn handle(server: &Arc<Server>, req: Request<Body>) -> Response {
    use tower::ServiceExt;
    match server.router().oneshot(req).await {
        Ok(r) => r,
        Err(e) => match e {},
    }
}

/// Addresses on the local network a phone can use (private IPv4 only).
pub fn lan_urls(port: u16) -> Vec<String> {
    let mut out: Vec<String> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !i.is_loopback())
        .filter_map(|i| match i.ip() {
            std::net::IpAddr::V4(v4) if v4.is_private() => Some(format!("http://{v4}:{port}/")),
            _ => None,
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests;
