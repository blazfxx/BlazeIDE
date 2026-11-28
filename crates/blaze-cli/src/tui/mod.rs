//! The full-screen chat. Clean opencode-style layout (see render.rs), a `/`
//! command menu, pickers, first-run setup, and /rc to continue on a phone.

pub mod commands;
mod render;
pub mod theme;

use std::collections::HashSet;
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use blaze_core::agent::{self, AgentEvent, Approval, HostRequest, TurnOutcome};
use blaze_core::ai::ChatMessage;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::rc::{self, Remote, Way};
use crate::session::{Chat, Session};

#[derive(Clone, Debug)]
pub enum Entry {
    User(String),
    Assistant(String),
    Tool { summary: String, state: ToolState },
    File { rel: String, added: usize, removed: usize, created: bool },
    Notice(String),
    Error(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ToolState {
    Running,
    Ok,
    Failed(String),
}

pub enum Pending {
    Approve { summary: String, tool: String, reply: oneshot::Sender<bool> },
    Ask { questions: Vec<(String, Vec<String>)>, answers: Vec<String>, reply: oneshot::Sender<Vec<String>> },
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum PickKind {
    Model,
    Chats,
    Bots,
}

pub struct PickItem {
    pub id: String,
    pub label: String,
    pub detail: String,
}

pub struct Picker {
    pub kind: PickKind,
    pub filter: String,
    pub sel: usize,
    pub items: Vec<PickItem>,
}

impl Picker {
    pub fn visible(&self) -> Vec<&PickItem> {
        let f = self.filter.to_lowercase();
        self.items.iter().filter(|i| f.is_empty() || i.label.to_lowercase().contains(&f) || i.id.to_lowercase().contains(&f) || i.detail.to_lowercase().contains(&f)).collect()
    }
}

pub struct PwPrompt {
    pub a: String,
    pub b: String,
    pub second: bool,
    pub err: String,
    pub then: Way,
}

/// API key entry for /provider add|key (one hidden field; empty = no key).
pub struct KeyPrompt {
    pub name: String,
    pub url: String,
    /// Some(id) = set the key of an existing provider.
    pub existing: Option<String>,
    pub value: String,
    pub err: String,
    pub busy: bool,
}

pub enum Overlay {
    Help,
    Key(KeyPrompt),
    Picker(Picker),
    Password(PwPrompt),
    Rc,
    Info(String, Vec<String>),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Onboard {
    Welcome,
    SignIn,
    Model,
    Approval,
}

#[derive(Clone)]
pub enum Target {
    Project,
    Bot { id: String, name: String, thread: Option<String> },
}

enum UiMsg {
    Login(Result<String, String>),
    RcLog(String),
    Rc(Box<Result<Remote, String>>),
    Info(String, Vec<String>),
    /// /provider add: the provider's /models answer.
    ProviderModels(blaze_core::providers::CustomProvider, Option<String>, Result<Vec<String>, String>),
    /// Background update check (and the time to remember as "checked").
    Update(crate::update::Found, Option<u64>),
}

enum RunResult {
    Chat(Vec<ChatMessage>, TurnOutcome),
    Bot(Result<(String, TurnOutcome), String>),
}

struct Running {
    cancel: CancellationToken,
    handle: JoinHandle<RunResult>,
    user_index: usize,
    started: Instant,
}

pub struct App {
    pub s: Session,
    pub root: PathBuf,
    pub model: String,
    pub mode: String,
    pub approval: Approval,
    pub chat: Chat,
    pub target: Target,
    pub entries: Vec<Entry>,
    pub input: String,
    pub cursor: usize,
    pub scroll: usize,
    pub pending: Option<Pending>,
    pub overlay: Option<Overlay>,
    pub onboard: Option<Onboard>,
    pub onboard_sel: usize,
    pub signing_in: bool,
    pub status: String,
    pub tokens: (u64, u64),
    pub last_input: u64,
    pub cost: f64,
    pub spinner: usize,
    pub palette_sel: usize,
    pub rc: Option<Remote>,
    pub rc_busy: bool,
    pub rc_log: Vec<String>,
    pub rc_qr: Option<String>,
    rc_seq: u64,
    history: Vec<String>,
    hist_pos: Option<usize>,
    always: HashSet<String>,
    running: Option<Running>,
    ui_tx: mpsc::UnboundedSender<UiMsg>,
    /// Reload the model list on the next tick (a provider changed).
    pub models_dirty: bool,
    /// What the terminal said about its background at start (None = no answer).
    pub detected_light: Option<bool>,
    /// After that reload, open the model picker with this filter.
    pub pick_after_reload: Option<String>,
    quit: bool,
    presence: Option<blaze_core::presence::Presence>,
    shown_what: String,
}

impl App {
    pub fn project_name(&self) -> String {
        self.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "a project".into())
    }

    /// "Working on <what> - BlazeCLI - blazeapi.org": terminal title + Discord status.
    fn show_working(&mut self, what: &str) {
        if what == self.shown_what {
            return;
        }
        self.shown_what = what.to_string();
        let (details, state) = blaze_core::presence::line(what);
        let _ = crossterm::execute!(io::stdout(), crossterm::terminal::SetTitle(format!("{details} - {state}")));
        if let Some(p) = &self.presence {
            p.set(what);
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    pub fn elapsed(&self) -> u64 {
        self.running.as_ref().map(|r| r.started.elapsed().as_secs()).unwrap_or(0)
    }

    pub fn context_pct(&self) -> Option<u64> {
        let cw = self.s.find_model(&self.model).map(|m| m.context_window).unwrap_or(0);
        (cw > 0 && self.last_input > 0).then(|| (self.last_input * 100 / cw).min(100))
    }

    pub fn model_label(&self) -> String {
        self.s.find_model(&self.model).map(|m| m.display_name.clone()).unwrap_or_else(|| self.model.clone())
    }

    pub fn title(&self) -> String {
        match &self.target {
            Target::Bot { name, .. } => format!("{name} · BlBot"),
            Target::Project => {
                if self.chat.messages.is_empty() { "New chat".into() } else { self.chat.title.clone() }
            }
        }
    }

    fn push(&mut self, e: Entry) {
        self.entries.push(e);
        self.scroll = 0;
    }

    fn note(&mut self, t: impl Into<String>) {
        self.push(Entry::Notice(t.into()));
    }

    fn err(&mut self, t: impl Into<String>) {
        self.push(Entry::Error(t.into()));
    }

    fn load_transcript(&mut self, msgs: &[ChatMessage]) {
        self.entries.clear();
        for m in msgs {
            match m.role.as_str() {
                "user" if !m.content.starts_with("[Screenshot") => self.entries.push(Entry::User(m.content.clone())),
                "assistant" => {
                    if !m.content.trim().is_empty() {
                        self.entries.push(Entry::Assistant(m.content.clone()));
                    }
                    for c in m.tool_calls.iter().flatten() {
                        self.entries.push(Entry::Tool { summary: agent::summarize(c), state: ToolState::Ok });
                    }
                }
                _ => {}
            }
        }
        self.scroll = 0;
    }

    // ---------- sending ----------

    fn send(&mut self, text: String, events: &mpsc::UnboundedSender<AgentEvent>, host: &mpsc::Sender<HostRequest>) {
        if self.s.key.is_none() && !self.model.starts_with("custom:") && matches!(self.target, Target::Project) {
            self.err("Not signed in. Type /login.");
            return;
        }
        self.history.push(text.clone());
        self.hist_pos = None;
        self.push(Entry::User(text.clone()));
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        let (events, host) = (events.clone(), host.clone());
        let user_index = self.chat.messages.len();
        let handle = match self.target.clone() {
            Target::Project => {
                self.chat.messages.push(ChatMessage { role: "user".into(), content: text, ..Default::default() });
                self.chat.model = self.model.clone();
                self.chat.mode = self.mode.clone();
                let backend = self.s.backend();
                let cfg = self.s.agent_config(&self.root, &self.model, &self.mode, self.approval);
                let mut conv = self.chat.messages.clone();
                tokio::spawn(async move {
                    let out = agent::run_turn(&backend, &cfg, &mut conv, &events, &host, &c2).await;
                    RunResult::Chat(conv, out)
                })
            }
            Target::Bot { id, thread, .. } => {
                let ctx = crate::bots::ctx(&self.s);
                tokio::spawn(async move {
                    let r = blaze_core::bots::runner::thread_turn(&ctx, &id, thread.as_deref(), &text, &events, &host, &c2).await;
                    RunResult::Bot(r)
                })
            }
        };
        self.running = Some(Running { cancel, handle, user_index, started: Instant::now() });
    }

    async fn finish_if_done(&mut self) {
        if !self.running.as_ref().is_some_and(|r| r.handle.is_finished()) {
            return;
        }
        let r = self.running.take().expect("running");
        let p = self.project_name();
        self.show_working(&p);
        match r.handle.await {
            Ok(RunResult::Chat(conv, out)) => {
                self.chat.messages = conv;
                self.chat.add_checkpoint(r.user_index, &out.changed);
                if out.stopped {
                    self.note("Stopped.");
                }
                if let Err(e) = self.chat.save() {
                    self.err(format!("Could not save the chat: {e}"));
                }
            }
            Ok(RunResult::Bot(Ok((tid, out)))) => {
                if let Target::Bot { thread, .. } = &mut self.target {
                    *thread = Some(tid);
                }
                if out.stopped {
                    self.note("Stopped.");
                }
            }
            Ok(RunResult::Bot(Err(e))) => self.err(e),
            Err(e) => self.err(format!("The turn crashed: {e}")),
        }
        self.pending = None;
    }

    fn on_agent(&mut self, ev: AgentEvent) {
        match ev {
            AgentEvent::Text { text } => match self.entries.last_mut() {
                Some(Entry::Assistant(t)) => t.push_str(&text),
                _ => self.push(Entry::Assistant(text)),
            },
            AgentEvent::ToolStart { summary, .. } => {
                let what = blaze_core::presence::what_from_summary(&summary, &self.project_name());
                self.show_working(&what);
                self.push(Entry::Tool { summary, state: ToolState::Running });
            }
            AgentEvent::ToolEnd { result, is_error, .. } => {
                if let Some(Entry::Tool { state, .. }) = self.entries.iter_mut().rev().find(|e| matches!(e, Entry::Tool { state: ToolState::Running, .. })) {
                    *state = if is_error { ToolState::Failed(result.lines().next().unwrap_or("").to_string()) } else { ToolState::Ok };
                }
            }
            AgentEvent::FileChanged { rel, added, removed, created } => self.push(Entry::File { rel, added, removed, created }),
            AgentEvent::Usage { input_tokens, output_tokens } => {
                self.tokens.0 += input_tokens;
                self.tokens.1 += output_tokens;
                self.last_input = input_tokens;
                if let Some(m) = self.s.find_model(&self.model) {
                    self.cost += input_tokens as f64 * m.price_in / 1e6 + output_tokens as f64 * m.price_out / 1e6;
                }
            }
            AgentEvent::Notice { text } => self.note(text),
            AgentEvent::Error { message, .. } => self.err(message),
            AgentEvent::Round { .. } | AgentEvent::Reasoning { .. } | AgentEvent::Done { .. } => {}
        }
    }

    fn on_host(&mut self, req: HostRequest) {
        match req {
            HostRequest::Approve { call, summary, reply } => {
                // "Always" never covers what Auto asks about as destructive.
                let risky = self.approval == Approval::Auto && blaze_core::agent::risk_of(&call).is_some();
                if self.always.contains(&call.name) && !risky {
                    let _ = reply.send(true);
                } else {
                    self.pending = Some(Pending::Approve { summary, tool: call.name, reply });
                }
            }
            HostRequest::Ask { questions, reply } => {
                self.pending = Some(Pending::Ask { questions, answers: Vec::new(), reply });
                self.input.clear();
                self.cursor = 0;
            }
        }
    }

    fn on_ui(&mut self, m: UiMsg) {
        match m {
            UiMsg::Login(Ok(who)) => {
                self.signing_in = false;
                self.s.key = crate::config::load_key();
                self.note(format!("Signed in as {who}."));
                if self.onboard == Some(Onboard::SignIn) {
                    self.onboard = Some(Onboard::Model);
                    self.onboard_sel = 0;
                }
                self.status = "Loading models...".into();
            }
            UiMsg::Login(Err(e)) => {
                self.signing_in = false;
                self.err(e);
            }
            UiMsg::RcLog(l) => self.rc_log.push(l),
            UiMsg::Rc(r) => {
                self.rc_busy = false;
                match *r {
                    Ok(remote) => {
                        self.rc_seq = remote.server.events_after(u64::MAX).0;
                        self.rc_qr = remote.pair_link().map(|l| blaze_core::remote::qr::terminal(&l));
                        let best = remote.best().unwrap_or("").to_string();
                        self.rc = Some(remote);
                        self.note(format!("Remote connect is on: {best}"));
                    }
                    Err(e) => {
                        self.rc_log.push(format!("Error: {e}"));
                        self.err(format!("Remote connect: {e}"));
                    }
                }
            }
            UiMsg::Info(t, lines) => self.overlay = Some(Overlay::Info(t, lines)),
            UiMsg::Update(found, stamp) => {
                if let Some(t) = stamp {
                    self.s.set_setting("cliUpdateChecked", serde_json::json!(t));
                }
                match found {
                    crate::update::Found::None => {}
                    crate::update::Found::Available(v) => self.note(format!("blaze {v} is available (you have {}). Type /update to install it.", crate::VERSION)),
                    crate::update::Found::Installed(v) => self.note(format!("Updated blaze to {v}. It runs from the next time you start blaze.")),
                    crate::update::Found::Failed(e) => self.note(e),
                }
            }
            UiMsg::ProviderModels(mut p, key, res) => {
                self.overlay = None;
                let fetched = match res {
                    Ok(m) if !m.is_empty() => {
                        p.models = m;
                        true
                    }
                    Ok(_) => false,
                    Err(e) => {
                        self.note(format!("Could not list its models ({e})."));
                        false
                    }
                };
                let p = match blaze_core::providers::sanitize(p) {
                    Ok(p) => p,
                    Err(e) => return self.err(e.to_string()),
                };
                if let Some(k) = &key {
                    if let Err(e) = crate::config::save_secret(&format!("provider-{}", p.id), k) {
                        return self.err(e);
                    }
                }
                if let Err(e) = self.s.store.upsert_provider(p.clone()) {
                    return self.err(e.to_string());
                }
                self.models_dirty = true;
                if fetched || !p.models.is_empty() {
                    self.note(format!("{} is ready with {} models. Its requests go straight to {} with your key, not through BlazeAPI.", p.name, p.models.len(), p.base_url));
                    self.pick_after_reload = Some(format!("custom:{}/", p.id));
                } else {
                    self.note(format!("Added {}, but it listed no models. Add them with: blaze providers rm \"{}\" then blaze providers add \"{}\" {} --models a,b", p.name, p.name, p.name, p.base_url));
                }
            }
        }
    }

    /// Phone activity while remote connect is on.
    fn poll_rc(&mut self) {
        let Some(r) = &self.rc else { return };
        let (seq, evs) = r.server.events_after(self.rc_seq);
        self.rc_seq = seq;
        for e in evs {
            match e["type"].as_str() {
                Some("run_start") => self.note(format!("Phone: started \"{}\"", e["title"].as_str().unwrap_or("a chat"))),
                Some("pending") => self.note(format!("Phone: waiting for an answer on the phone ({})", e["item"]["summary"].as_str().unwrap_or("a question"))),
                Some("run_end") => self.note(format!("Phone: finished{}", e["error"].as_str().map(|x| format!(" with an error: {x}")).unwrap_or_default())),
                _ => {}
            }
        }
    }

    // ---------- commands ----------

    fn open_picker(&mut self, kind: PickKind) {
        let items: Vec<PickItem> = match kind {
            PickKind::Model => self.s.models.iter().filter(|m| m.kind == "chat").map(|m| PickItem { id: m.id.clone(), label: m.display_name.clone(), detail: if m.tags.iter().any(|t| t == "custom") { format!("{} · your provider", m.provider) } else { crate::session::price_text(m) } }).collect(),
            PickKind::Chats => blaze_core::chats::list(&self.root).into_iter().map(|c| PickItem { id: c.id, label: c.title, detail: format!("{} messages", c.message_count) }).collect(),
            PickKind::Bots => blaze_core::bots::Bots::open(&crate::config::config_dir()).list().into_iter().filter(|b| !b.hidden).map(|b| PickItem { label: format!("{} {}", b.avatar, b.name), detail: if b.title.is_empty() { b.engine.label().to_string() } else { b.title.clone() }, id: b.id }).collect(),
        };
        if items.is_empty() {
            self.note(match kind {
                PickKind::Model => "No models yet: /login for BlazeAPI, or /provider add <name> <url> for your own.",
                PickKind::Chats => "No saved chats in this project yet.",
                PickKind::Bots => "No BlBots yet. Create one: blaze bots new Nova --soul \"...\" (or in Blaze IDE).",
            });
            return;
        }
        let sel = if kind == PickKind::Model { items.iter().position(|i| i.id == self.model).unwrap_or(0) } else { 0 };
        self.overlay = Some(Overlay::Picker(Picker { kind, filter: String::new(), sel, items }));
    }

    fn picked(&mut self, kind: PickKind, id: String) {
        match kind {
            PickKind::Model => self.set_model(id),
            PickKind::Chats => match Chat::load(&self.root, &id) {
                Ok(c) => {
                    self.model = c.model.clone();
                    self.mode = c.mode.clone();
                    self.load_transcript(&c.messages.clone());
                    self.chat = c;
                    self.target = Target::Project;
                }
                Err(e) => self.err(e),
            },
            PickKind::Bots => {
                let bots = blaze_core::bots::Bots::open(&crate::config::config_dir());
                if let Some(b) = bots.get(&id) {
                    let latest = blaze_core::chats::list_in(&bots.threads_dir(&b.id)).into_iter().next();
                    let mut msgs = Vec::new();
                    if let Some(t) = &latest {
                        if let Ok(c) = blaze_core::chatfile::ChatFile::load_from(&bots.workdir(&b), &bots.threads_dir(&b.id), &t.id) {
                            msgs = c.messages;
                        }
                    }
                    self.load_transcript(&msgs);
                    self.target = Target::Bot { id: b.id.clone(), name: b.name.clone(), thread: latest.map(|t| t.id) };
                    self.note(format!("Talking to {} ({}). /new to start a fresh chat with it, /chats to go back to your project.", b.name, b.engine.label()));
                }
            }
        }
    }

    /// `/provider` (list), `/provider add <name> <url>`, `/provider key <name>`,
    /// `/provider models <name>`, `/provider rm <name>`.
    fn provider_cmd(&mut self, arg: &str) {
        let (sub, rest) = arg.split_once(' ').map(|(a, b)| (a, b.trim())).unwrap_or((arg, ""));
        let find = |s: &crate::session::Session, k: &str| s.store.providers().into_iter().find(|p| p.id == k || p.name.eq_ignore_ascii_case(k.trim_matches('"')));
        match sub {
            "" | "list" => {
                let mut lines = Vec::new();
                for p in self.s.store.providers() {
                    let key = if crate::config::load_provider_key(&p.id).is_some() { "key saved" } else { "no key" };
                    lines.push(format!("{}  ·  {}  ·  {} models  ·  {key}", p.name, p.base_url, p.models.len()));
                }
                if lines.is_empty() {
                    lines.push("No providers yet.".into());
                }
                lines.push(String::new());
                lines.push("/provider add <name> <url>   add one (asks for its API key)".into());
                lines.push("/provider key <name>         set or replace its key".into());
                lines.push("/provider models <name>      refresh its model list".into());
                lines.push("/provider rm <name>          remove it".into());
                lines.push("/model                       pick one of its models".into());
                lines.push(String::new());
                lines.push("e.g. /provider add Ollama http://localhost:11434/v1".into());
                lines.push("     /provider add OpenRouter https://openrouter.ai/api/v1".into());
                lines.push(String::new());
                lines.push("Your own providers are called straight from this computer with your key.".into());
                lines.push("They never go through BlazeAPI and are not billed to your BlazeAPI balance.".into());
                self.overlay = Some(Overlay::Info("Your providers".into(), lines));
            }
            "add" => {
                let Some((name, url)) = rest.rsplit_once(' ') else {
                    return self.err("Use /provider add <name> <url>, e.g. /provider add Ollama http://localhost:11434/v1");
                };
                let (name, url) = (name.trim().trim_matches('"').to_string(), url.trim().to_string());
                if let Err(e) = blaze_core::providers::sanitize(blaze_core::providers::CustomProvider { id: String::new(), name: name.clone(), base_url: url.clone(), models: vec![] }) {
                    return self.err(e.to_string());
                }
                if self.s.store.providers().iter().any(|p| p.name.eq_ignore_ascii_case(&name)) {
                    return self.err(format!("You already have a provider called {name}."));
                }
                self.overlay = Some(Overlay::Key(KeyPrompt { name, url, existing: None, value: String::new(), err: String::new(), busy: false }));
            }
            "key" => match find(&self.s, rest) {
                Some(p) => self.overlay = Some(Overlay::Key(KeyPrompt { name: p.name, url: p.base_url, existing: Some(p.id), value: String::new(), err: String::new(), busy: false })),
                None => self.err("No such provider. Type /provider to see yours."),
            },
            "models" => match find(&self.s, rest) {
                Some(p) => {
                    let key = crate::config::load_provider_key(&p.id);
                    self.fetch_provider_models(p, key);
                }
                None => self.err("No such provider. Type /provider to see yours."),
            },
            "rm" | "remove" | "delete" => match find(&self.s, rest) {
                Some(p) => {
                    if let Err(e) = self.s.store.remove_provider(&p.id) {
                        return self.err(e.to_string());
                    }
                    crate::config::delete_secret(&format!("provider-{}", p.id));
                    if self.model.starts_with(&format!("custom:{}/", p.id)) {
                        self.note("That was your current model: pick another with /model.");
                    }
                    self.models_dirty = true;
                    self.note(format!("Removed {}.", p.name));
                }
                None => self.err("No such provider. Type /provider to see yours."),
            },
            _ => self.err("Use /provider, /provider add <name> <url>, /provider key <name>, /provider models <name> or /provider rm <name>."),
        }
    }

    fn fetch_provider_models(&mut self, p: blaze_core::providers::CustomProvider, key: Option<String>) {
        let (http, tx) = (self.s.http_custom.clone(), self.ui_tx.clone());
        self.status = format!("Asking {} for its models...", p.name);
        tokio::spawn(async move {
            let res = blaze_core::providers::fetch_models(&http, &p.base_url, key.as_deref()).await.map_err(|e| e.to_string());
            let _ = tx.send(UiMsg::ProviderModels(p, key, res));
        });
    }

    /// Pick the palette from the `cliTheme` setting. `probe` = ask the
    /// terminal (only at start: later the event reader owns the input).
    pub fn apply_theme(&mut self, probe: bool) {
        match self.s.setting_str("cliTheme").as_str() {
            "light" => theme::set_light(true),
            "dark" => theme::set_light(false),
            _ => {
                if probe {
                    self.detected_light = theme::detect();
                }
                theme::set_light(self.detected_light.unwrap_or(false));
            }
        }
    }

    fn set_model(&mut self, id: String) {
        let id = if self.s.models.is_empty() || self.s.find_model(&id).is_some() {
            id
        } else {
            // `/model sonnet`: a unique match by name or id switches directly,
            // several open the picker filtered to them.
            let q = id.to_lowercase();
            let hits: Vec<String> = self.s.models.iter().filter(|m| m.kind == "chat" && (m.id.to_lowercase().contains(&q) || m.display_name.to_lowercase().contains(&q))).map(|m| m.id.clone()).collect();
            match hits.len() {
                0 => return self.err(format!("No model matches {id}. Type /model to browse.")),
                1 => hits[0].clone(),
                _ => {
                    self.open_picker(PickKind::Model);
                    if let Some(Overlay::Picker(p)) = self.overlay.as_mut() {
                        p.filter = id;
                        p.sel = 0;
                    }
                    return;
                }
            }
        };
        self.model = id;
        self.s.set_setting("cliModel", serde_json::json!(self.model));
        self.status = format!("Model: {}", self.model_label());
    }

    fn command(&mut self, line: &str, events: &mpsc::UnboundedSender<AgentEvent>, host: &mpsc::Sender<HostRequest>) {
        let mut parts = line.trim().splitn(2, ' ');
        let name = parts.next().unwrap_or("");
        let arg = parts.next().unwrap_or("").trim().to_string();
        let Some(cmd) = commands::find(name) else {
            self.err(format!("Unknown command {name}. Type / to see them all."));
            return;
        };
        match cmd.name {
            "help" => self.overlay = Some(Overlay::Help),
            "exit" => self.quit = true,
            "new" => {
                self.chat = Chat::new(&self.root, &self.mode, &self.model);
                if let Target::Bot { thread, .. } = &mut self.target {
                    *thread = None;
                }
                self.entries.clear();
                self.tokens = (0, 0);
                self.cost = 0.0;
                self.last_input = 0;
            }
            "chats" => {
                if !arg.is_empty() {
                    self.picked(PickKind::Chats, arg);
                } else {
                    self.open_picker(PickKind::Chats);
                }
            }
            "model" => {
                if arg.is_empty() {
                    self.open_picker(PickKind::Model);
                } else {
                    self.set_model(arg);
                }
            }
            "mode" => match arg.as_str() {
                "ask" | "plan" | "agent" => self.mode = arg,
                _ => self.err("Use /mode ask, /mode plan or /mode agent (or press Tab)."),
            },
            "approval" => match Approval::parse(&arg) {
                Some(a) => {
                    self.approval = a;
                    self.s.set_setting("cliApproval", serde_json::json!(approval_key(a)));
                    self.status = format!("Approval: {}", a.label());
                }
                None => self.err("Use /approval ask, edits, auto or yolo."),
            },
            "bots" => self.open_picker(PickKind::Bots),
            "undo" => self.undo(),
            "init" => {
                if self.is_running() {
                    return;
                }
                self.send("Look through this project and write (or update) AGENTS.md at its root: what the project is, how it is laid out, how to build, run and test it, and conventions an AI agent must follow. Keep it short and concrete. Use write_file.".into(), events, host);
            }
            "copy" => {
                let last = self.entries.iter().rev().find_map(|e| if let Entry::Assistant(t) = e { Some(t.clone()) } else { None });
                match last {
                    Some(t) => {
                        copy_to_clipboard(&t);
                        self.status = "Copied the last answer.".into();
                    }
                    None => self.status = "Nothing to copy yet.".into(),
                }
            }
            "export" => self.export(),
            "status" => self.status_info(),
            "provider" => self.provider_cmd(&arg),
            "login" => self.login(),
            "logout" => {
                let (http, base, key) = (self.s.http.clone(), self.s.api_base.clone(), self.s.key.clone());
                tokio::spawn(async move {
                    if let Some(k) = key {
                        blaze_core::auth::revoke_key(&http, &base, &k).await;
                    }
                });
                crate::config::delete_key();
                let _ = self.s.store.set_account(None);
                self.s.key = None;
                self.note("Signed out on this computer.");
            }
            "setup" => {
                self.onboard = Some(Onboard::Welcome);
                self.onboard_sel = 0;
            }
            "rc" => self.rc_command(&arg),
            "update" => match arg.as_str() {
                "auto" | "notify" | "manual" | "off" => {
                    let m = crate::update::Mode::parse(&arg).unwrap_or(crate::update::Mode::Auto);
                    self.s.set_setting("cliUpdates", serde_json::json!(m.key()));
                    self.status = match m {
                        crate::update::Mode::Auto => "Updates: automatic (installed in the background, used next start)".into(),
                        crate::update::Mode::Notify => "Updates: tell me, I'll type /update".into(),
                        crate::update::Mode::Off => "Updates: off (type /update to check by hand)".into(),
                    };
                }
                "" => {
                    let (http, tx) = (self.s.http.clone(), self.ui_tx.clone());
                    self.status = "Checking for a new blaze...".into();
                    tokio::spawn(async move {
                        let found = match crate::update::latest(&http).await {
                            Err(e) => crate::update::Found::Failed(e),
                            Ok(v) if !crate::update::newer(&v, crate::VERSION) => crate::update::Found::Failed(format!("blaze {} is the newest version.", crate::VERSION)),
                            Ok(v) => match crate::update::install(&http, &v).await {
                                Ok(_) => crate::update::Found::Installed(v),
                                Err(e) => crate::update::Found::Failed(e),
                            },
                        };
                        let _ = tx.send(UiMsg::Update(found, None));
                    });
                }
                _ => self.err("Use /update (install now) or /update auto|notify|off."),
            },
            "theme" => match arg.as_str() {
                "auto" | "dark" | "light" => {
                    self.s.set_setting("cliTheme", serde_json::json!(arg));
                    self.apply_theme(false);
                    self.status = format!("Theme: {arg}{}", if arg == "auto" { if theme::is_light() { " (light terminal)" } else { " (dark terminal)" } } else { "" });
                }
                "" => self.status = format!("Theme: {} · /theme auto|dark|light", self.s.setting_str("cliTheme").chars().next().map(|_| self.s.setting_str("cliTheme")).unwrap_or_else(|| "auto".into())),
                _ => self.err("Use /theme auto, /theme dark or /theme light."),
            },
            "discord" => {
                let on = arg != "off";
                self.s.set_setting("discordStatus", serde_json::json!(on));
                if on && self.presence.is_none() {
                    self.presence = blaze_core::presence::Presence::start();
                    self.shown_what.clear();
                    let p = self.project_name();
                    self.show_working(&p);
                } else if !on {
                    if let Some(p) = self.presence.take() {
                        p.clear();
                    }
                }
                self.status = if !on {
                    "Discord status off.".into()
                } else if self.presence.is_some() {
                    "Discord status on (shows the project and file you work on to your Discord friends).".into()
                } else {
                    "Discord status needs this build's Discord app id; it is not set yet.".into()
                };
            }
            _ => {}
        }
    }

    fn rc_command(&mut self, arg: &str) {
        if arg == "off" || arg == "stop" {
            if self.rc.take().is_some() {
                self.note("Remote connect is off. Phones cannot reach this computer now.");
            } else {
                self.status = "Remote connect was not on.".into();
            }
            return;
        }
        if self.rc.is_some() || self.rc_busy {
            self.overlay = Some(Overlay::Rc);
            return;
        }
        let Some(way) = Way::parse(arg) else {
            self.err("Use /rc, /rc relay, /rc tunnel, /rc lan or /rc off.");
            return;
        };
        if !rc::has_password() {
            self.overlay = Some(Overlay::Password(PwPrompt { a: String::new(), b: String::new(), second: false, err: String::new(), then: way }));
            return;
        }
        self.start_rc(way);
    }

    fn start_rc(&mut self, way: Way) {
        self.rc_busy = true;
        self.rc_log = vec!["Starting...".into()];
        self.overlay = Some(Overlay::Rc);
        let tx = self.ui_tx.clone();
        let tx2 = self.ui_tx.clone();
        // Session is not Send-safe to share; make a fresh one for the task.
        let mut s = Session::open();
        s.models = self.s.models.clone();
        tokio::spawn(async move {
            let r = rc::start(&s, way, move |l| {
                let _ = tx2.send(UiMsg::RcLog(l));
            })
            .await;
            let _ = tx.send(UiMsg::Rc(Box::new(r)));
        });
    }

    fn login(&mut self) {
        if self.signing_in {
            return;
        }
        self.signing_in = true;
        self.note("Opening your browser to sign in with BlazeAPI... (if nothing opens, quit and run `blaze login --key`)");
        let tx = self.ui_tx.clone();
        let (http, base) = (self.s.http.clone(), self.s.api_base.clone());
        let store = blaze_core::store::Store::open(&crate::config::config_dir());
        tokio::spawn(async move {
            let r = blaze_core::auth::sign_in(
                &http,
                &base,
                |url| {
                    let _ = crate::open_browser(url);
                    Ok(())
                },
                CancellationToken::new(),
            )
            .await;
            let msg = match r {
                Ok(signed) => match crate::config::save_key(&signed.api_key) {
                    Ok(()) => {
                        let _ = store.set_account(Some(serde_json::json!({ "email": signed.email, "username": signed.username })));
                        Ok(if signed.email.is_empty() { signed.username } else { signed.email })
                    }
                    Err(e) => Err(e),
                },
                Err(e) => Err(format!("Sign-in failed: {e}")),
            };
            let _ = tx.send(UiMsg::Login(msg));
        });
    }

    fn status_info(&mut self) {
        let mut lines = vec![
            format!("Account     {}", self.s.account_name().unwrap_or_else(|| if self.s.key.is_some() { "signed in with a key".into() } else { "not signed in (/login)".into() })),
            format!("Project     {}", self.root.display()),
            format!("Model       {} ({})", self.model_label(), self.model),
            format!("Mode        {} · {}", self.mode, self.approval.label()),
            format!("This chat   {} in · {} out · ${:.4}", self.tokens.0, self.tokens.1, self.cost),
            format!("Remote      {}", self.rc.as_ref().and_then(|r| r.best().map(str::to_string)).unwrap_or_else(|| "off (/rc)".into())),
            format!("Version     blaze {}", crate::VERSION),
        ];
        let Some(key) = self.s.key.clone() else {
            self.overlay = Some(Overlay::Info("Status".into(), lines));
            return;
        };
        lines.push("Balance     checking...".into());
        self.overlay = Some(Overlay::Info("Status".into(), lines.clone()));
        let (http, v1, tx) = (self.s.http.clone(), self.s.v1_base.clone(), self.ui_tx.clone());
        tokio::spawn(async move {
            let b = blaze_core::models::fetch_balance(&http, &v1, &key).await;
            lines.pop();
            lines.push(match b {
                Ok(v) => format!("Balance     ${v:.2}"),
                Err(e) => format!("Balance     unavailable ({e})"),
            });
            let _ = tx.send(UiMsg::Info("Status".into(), lines));
        });
    }

    fn undo(&mut self) {
        if self.is_running() || !matches!(self.target, Target::Project) {
            self.status = "Nothing to undo here.".into();
            return;
        }
        let Some(last_user) = self.chat.messages.iter().rposition(|m| m.role == "user") else {
            self.status = "Nothing to undo.".into();
            return;
        };
        // Put files back as they were before that turn.
        let mut restored = 0;
        if let Some(pos) = self.chat.checkpoints.iter().rposition(|c| c["userIndex"].as_u64() == Some(last_user as u64)) {
            let cp = self.chat.checkpoints.remove(pos);
            for f in cp["files"].as_array().into_iter().flatten() {
                let Some(rel) = f["rel"].as_str() else { continue };
                let Ok(p) = blaze_core::paths::resolve_in(&self.root, rel) else { continue };
                let ok = match f["before"].as_str() {
                    Some(b) => blaze_core::files::write_text(&p, b).is_ok(),
                    None => std::fs::remove_file(&p).is_ok(),
                };
                if ok {
                    restored += 1;
                }
            }
        }
        let text = self.chat.messages[last_user].content.clone();
        self.chat.messages.truncate(last_user);
        let _ = self.chat.save();
        let msgs = self.chat.messages.clone();
        self.load_transcript(&msgs);
        self.input = text;
        self.cursor = self.input.len();
        self.note(format!("Undid the last turn{}. Its message is back in the box to edit.", if restored > 0 { format!(" and restored {restored} file(s)") } else { String::new() }));
    }

    fn export(&mut self) {
        let mut md = format!("# {}\n\n", self.title());
        for e in &self.entries {
            match e {
                Entry::User(t) => md.push_str(&format!("## You\n\n{t}\n\n")),
                Entry::Assistant(t) => md.push_str(&format!("## Blaze\n\n{t}\n\n")),
                Entry::Tool { summary, .. } => md.push_str(&format!("- {summary}\n")),
                Entry::File { rel, added, removed, .. } => md.push_str(&format!("- {rel} (+{added} -{removed})\n")),
                _ => {}
            }
        }
        let name = format!("blaze-chat-{}.md", self.chat.id);
        match std::fs::write(self.root.join(&name), md) {
            Ok(()) => self.status = format!("Saved {name}"),
            Err(e) => self.err(format!("Could not save: {e}")),
        }
    }

    // ---------- keys ----------

    pub fn palette(&self) -> Vec<&'static commands::Cmd> {
        if self.overlay.is_some() || self.pending.is_some() || !self.input.starts_with('/') || self.input.contains(' ') || self.input.contains('\n') {
            return Vec::new();
        }
        commands::matching(&self.input)
    }

    fn on_key(&mut self, k: KeyEvent, events: &mpsc::UnboundedSender<AgentEvent>, host: &mpsc::Sender<HostRequest>) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        if ctrl && k.code == KeyCode::Char('c') && self.overlay.is_none() && self.onboard.is_none() {
            if let Some(r) = &self.running {
                r.cancel.cancel();
            } else if !self.input.is_empty() {
                self.input.clear();
                self.cursor = 0;
            } else {
                self.quit = true;
            }
            return;
        }
        if self.onboard.is_some() {
            self.onboard_key(k);
            return;
        }
        if self.overlay.is_some() {
            self.overlay_key(k);
            return;
        }
        if matches!(self.pending, Some(Pending::Approve { .. })) {
            let answer = match k.code {
                KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => Some((true, false)),
                KeyCode::Char('a') | KeyCode::Char('A') => Some((true, true)),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => Some((false, false)),
                _ => None,
            };
            if let Some((ok, always)) = answer {
                if let Some(Pending::Approve { reply, tool, summary }) = self.pending.take() {
                    if always {
                        self.always.insert(tool);
                    }
                    let _ = reply.send(ok);
                    if !ok {
                        self.note(format!("Refused: {summary}"));
                    }
                }
            }
            return;
        }
        // Command menu navigation.
        let pal = self.palette();
        if !pal.is_empty() {
            match k.code {
                KeyCode::Up => {
                    self.palette_sel = self.palette_sel.saturating_sub(1);
                    return;
                }
                KeyCode::Down => {
                    self.palette_sel = (self.palette_sel + 1).min(pal.len() - 1);
                    return;
                }
                KeyCode::Tab => {
                    let c = pal[self.palette_sel.min(pal.len() - 1)];
                    self.input = format!("/{}{}", c.name, if c.args.is_empty() { "" } else { " " });
                    self.cursor = self.input.len();
                    self.palette_sel = 0;
                    return;
                }
                KeyCode::Enter if !alt && !ctrl => {
                    let c = pal[self.palette_sel.min(pal.len() - 1)];
                    self.input.clear();
                    self.cursor = 0;
                    self.palette_sel = 0;
                    self.command(&format!("/{}", c.name), events, host);
                    return;
                }
                KeyCode::Esc => {
                    self.input.clear();
                    self.cursor = 0;
                    return;
                }
                _ => {}
            }
        }
        match k.code {
            KeyCode::Char('d') if ctrl && self.input.is_empty() => self.quit = true,
            KeyCode::Char('l') if ctrl => self.open_picker(PickKind::Model),
            KeyCode::Char('p') if ctrl => {
                self.input = "/".into();
                self.cursor = 1;
                self.palette_sel = 0;
            }
            KeyCode::Char('r') if ctrl => self.rc_command(""),
            KeyCode::Esc => {
                if let Some(r) = &self.running {
                    r.cancel.cancel();
                }
            }
            KeyCode::Tab => {
                self.mode = match self.mode.as_str() {
                    "agent" => "plan".into(),
                    "plan" => "ask".into(),
                    _ => "agent".into(),
                };
            }
            KeyCode::PageUp => self.scroll += 10,
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(10),
            KeyCode::Up if k.modifiers.contains(KeyModifiers::SHIFT) => self.scroll += 1,
            KeyCode::Down if k.modifiers.contains(KeyModifiers::SHIFT) => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Up if !self.input.contains('\n') && !self.history.is_empty() => {
                let i = self.hist_pos.map(|p| p.saturating_sub(1)).unwrap_or(self.history.len() - 1);
                self.hist_pos = Some(i);
                self.input = self.history[i].clone();
                self.cursor = self.input.len();
            }
            KeyCode::Down if self.hist_pos.is_some() => {
                let i = self.hist_pos.unwrap_or(0) + 1;
                if i >= self.history.len() {
                    self.hist_pos = None;
                    self.input.clear();
                } else {
                    self.hist_pos = Some(i);
                    self.input = self.history[i].clone();
                }
                self.cursor = self.input.len();
            }
            KeyCode::Enter if alt || ctrl => self.insert('\n'),
            KeyCode::Char('j') if ctrl => self.insert('\n'),
            KeyCode::Enter => self.submit(events, host),
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    let prev = self.input[..self.cursor].chars().next_back().map(char::len_utf8).unwrap_or(1);
                    self.cursor -= prev;
                    self.input.remove(self.cursor);
                }
                self.palette_sel = 0;
            }
            KeyCode::Delete => {
                if self.cursor < self.input.len() {
                    self.input.remove(self.cursor);
                }
            }
            KeyCode::Left => {
                if let Some(c) = self.input[..self.cursor].chars().next_back() {
                    self.cursor -= c.len_utf8();
                }
            }
            KeyCode::Right => {
                if let Some(c) = self.input[self.cursor..].chars().next() {
                    self.cursor += c.len_utf8();
                }
            }
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => self.cursor = self.input.len(),
            KeyCode::Char('u') if ctrl => {
                self.input.clear();
                self.cursor = 0;
            }
            KeyCode::Char(c) if !ctrl => {
                self.insert(c);
                self.palette_sel = 0;
            }
            _ => {}
        }
    }

    fn overlay_key(&mut self, k: KeyEvent) {
        let Some(ov) = self.overlay.as_mut() else { return };
        match ov {
            Overlay::Help | Overlay::Info(..) => {
                if matches!(k.code, KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q')) {
                    self.overlay = None;
                }
            }
            Overlay::Rc => match k.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => self.overlay = None,
                KeyCode::Char('n') => {
                    if let Some(r) = &self.rc {
                        self.rc_qr = r.pair_link().map(|l| blaze_core::remote::qr::terminal(&l));
                    }
                }
                KeyCode::Char('x') => {
                    self.rc = None;
                    self.rc_qr = None;
                    self.overlay = None;
                    self.note("Remote connect is off. Phones cannot reach this computer now.");
                }
                _ => {}
            },
            Overlay::Picker(p) => match k.code {
                KeyCode::Esc => self.overlay = None,
                KeyCode::Up => p.sel = p.sel.saturating_sub(1),
                KeyCode::Down => p.sel += 1,
                KeyCode::Backspace => {
                    p.filter.pop();
                    p.sel = 0;
                }
                KeyCode::Char(c) => {
                    p.filter.push(c);
                    p.sel = 0;
                }
                KeyCode::Enter => {
                    let kind = p.kind;
                    let vis = p.visible();
                    let id = vis.get(p.sel.min(vis.len().saturating_sub(1))).map(|i| i.id.clone());
                    self.overlay = None;
                    if let Some(id) = id {
                        self.picked(kind, id);
                    }
                }
                _ => {}
            },
            Overlay::Key(kp) => match k.code {
                KeyCode::Esc => self.overlay = None,
                _ if kp.busy => {}
                KeyCode::Backspace => {
                    kp.value.pop();
                }
                KeyCode::Char(c) => kp.value.push(c),
                KeyCode::Enter => {
                    let key = Some(kp.value.trim().to_string()).filter(|v| !v.is_empty());
                    match kp.existing.clone() {
                        Some(id) => {
                            let r = match &key {
                                Some(v) => crate::config::save_secret(&format!("provider-{id}"), v),
                                None => {
                                    crate::config::delete_secret(&format!("provider-{id}"));
                                    Ok(())
                                }
                            };
                            match r {
                                Ok(()) => {
                                    let name = kp.name.clone();
                                    self.overlay = None;
                                    self.note(if key.is_some() { format!("Saved the key for {name}.") } else { format!("Removed the key for {name}.") });
                                }
                                Err(e) => kp.err = e,
                            }
                        }
                        None => {
                            kp.busy = true;
                            kp.err.clear();
                            let p = blaze_core::providers::CustomProvider { id: String::new(), name: kp.name.clone(), base_url: kp.url.clone(), models: vec![] };
                            match blaze_core::providers::sanitize(p) {
                                Ok(p) => self.fetch_provider_models(p, key),
                                Err(e) => {
                                    kp.busy = false;
                                    kp.err = e.to_string();
                                }
                            }
                        }
                    }
                }
                _ => {}
            },
            Overlay::Password(pw) => match k.code {
                KeyCode::Esc => self.overlay = None,
                KeyCode::Backspace => {
                    if pw.second { pw.b.pop() } else { pw.a.pop() };
                }
                KeyCode::Char(c) => {
                    if pw.second { pw.b.push(c) } else { pw.a.push(c) }
                }
                KeyCode::Tab | KeyCode::Down | KeyCode::Up => pw.second = !pw.second,
                KeyCode::Enter => {
                    if !pw.second {
                        if pw.a.chars().count() < 10 {
                            pw.err = "Use at least 10 characters.".into();
                        } else {
                            pw.second = true;
                            pw.err.clear();
                        }
                    } else if pw.a != pw.b {
                        pw.err = "The two passwords are different.".into();
                        pw.b.clear();
                    } else {
                        let way = pw.then;
                        match rc::set_password(&pw.a) {
                            Ok(()) => {
                                self.overlay = None;
                                self.start_rc(way);
                            }
                            Err(e) => pw.err = e,
                        }
                    }
                }
                _ => {}
            },
        }
    }

    fn onboard_key(&mut self, k: KeyEvent) {
        let Some(step) = self.onboard else { return };
        if k.code == KeyCode::Esc {
            self.finish_onboard();
            return;
        }
        match step {
            Onboard::Welcome => {
                if k.code == KeyCode::Enter {
                    self.onboard = Some(if self.s.key.is_some() { Onboard::Model } else { Onboard::SignIn });
                    self.onboard_sel = 0;
                }
            }
            Onboard::SignIn => match k.code {
                KeyCode::Enter => self.login(),
                KeyCode::Char('s') => {
                    self.onboard = Some(Onboard::Model);
                    self.onboard_sel = 0;
                }
                _ => {}
            },
            Onboard::Model => {
                let list = onboard_models(self);
                match k.code {
                    KeyCode::Up => self.onboard_sel = self.onboard_sel.saturating_sub(1),
                    KeyCode::Down => self.onboard_sel = (self.onboard_sel + 1).min(list.len().saturating_sub(1)),
                    KeyCode::Enter => {
                        if let Some((id, _, _)) = list.get(self.onboard_sel) {
                            let id = id.clone();
                            self.set_model(id);
                        }
                        self.onboard = Some(Onboard::Approval);
                        self.onboard_sel = 1;
                    }
                    _ => {}
                }
            }
            Onboard::Approval => match k.code {
                KeyCode::Up => self.onboard_sel = self.onboard_sel.saturating_sub(1),
                KeyCode::Down => self.onboard_sel = (self.onboard_sel + 1).min(3),
                KeyCode::Enter => {
                    let a = [Approval::Ask, Approval::Edits, Approval::Auto, Approval::Full][self.onboard_sel.min(3)];
                    self.approval = a;
                    self.s.set_setting("cliApproval", serde_json::json!(approval_key(a)));
                    self.finish_onboard();
                }
                _ => {}
            },
        }
    }

    fn finish_onboard(&mut self) {
        self.onboard = None;
        self.s.set_setting("cliOnboarded", serde_json::json!(true));
        self.status = "All set. Type a message, or / for commands.".into();
    }

    fn insert(&mut self, c: char) {
        self.input.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    fn submit(&mut self, events: &mpsc::UnboundedSender<AgentEvent>, host: &mpsc::Sender<HostRequest>) {
        let text = std::mem::take(&mut self.input);
        self.cursor = 0;
        if let Some(Pending::Ask { questions, answers, .. }) = self.pending.as_mut() {
            let i = answers.len();
            let picked = text.trim().parse::<usize>().ok().and_then(|n| questions.get(i).and_then(|(_, o)| o.get(n.wrapping_sub(1)).cloned()));
            answers.push(picked.unwrap_or_else(|| text.trim().to_string()));
            if answers.len() >= questions.len() {
                if let Some(Pending::Ask { answers, reply, .. }) = self.pending.take() {
                    self.note(format!("You answered: {}", answers.join(" · ")));
                    let _ = reply.send(answers);
                }
            }
            return;
        }
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return;
        }
        if trimmed.starts_with('/') {
            self.command(trimmed, events, host);
            return;
        }
        if self.is_running() {
            self.input = text;
            self.cursor = self.input.len();
            self.status = "Still working: Esc stops it.".into();
            return;
        }
        self.send(trimmed.to_string(), events, host);
    }
}

pub fn approval_key(a: Approval) -> &'static str {
    match a {
        Approval::Ask => "ask",
        Approval::Edits => "edits",
        Approval::Auto => "auto",
        Approval::Full => "full",
    }
}

/// A short list for the first-run model step: a few good picks first.
pub fn onboard_models(app: &App) -> Vec<(String, String, String)> {
    const PREFERRED: &[&str] = &["claude-sonnet-5", "claude-sonnet-5-5", "gpt-6-1-sol", "deepseek-v4.1-flash", "glm-5.3", "gemini-4-carbon"];
    let mut out: Vec<(String, String, String)> = PREFERRED
        .iter()
        .filter_map(|id| app.s.find_model(id).map(|m| (m.id.clone(), m.display_name.clone(), crate::session::price_text(m))))
        .collect();
    for m in app.s.models.iter().filter(|m| m.kind == "chat") {
        if out.len() >= 8 {
            break;
        }
        if !out.iter().any(|(id, _, _)| id == &m.id) {
            out.push((m.id.clone(), m.display_name.clone(), crate::session::price_text(m)));
        }
    }
    if out.is_empty() {
        out.push((app.model.clone(), app.model.clone(), String::new()));
    }
    out
}

/// Copy text: pbcopy on macOS, clip on Windows, else the OSC 52 escape.
fn copy_to_clipboard(text: &str) {
    use std::process::{Command, Stdio};
    let tool: Option<(&str, &[&str])> = if cfg!(target_os = "macos") {
        Some(("pbcopy", &[]))
    } else if cfg!(windows) {
        Some(("clip", &[]))
    } else {
        None
    };
    if let Some((cmd, args)) = tool {
        if let Ok(mut c) = Command::new(cmd).args(args).stdin(Stdio::piped()).spawn() {
            if let Some(mut i) = c.stdin.take() {
                let _ = i.write_all(text.as_bytes());
            }
            let _ = c.wait();
            return;
        }
    }
    use base64::Engine;
    let b = base64::engine::general_purpose::STANDARD.encode(text.as_bytes());
    let _ = write!(io::stdout(), "\x1b]52;c;{b}\x07");
    let _ = io::stdout().flush();
}

pub async fn run(mut s: Session, root: PathBuf, model: String, mode: String, approval: Approval, resume: Option<String>) -> Result<(), String> {
    let models_err = s.load_models(false).await.err();
    let chat = match &resume {
        Some(id) => Chat::load(&root, id)?,
        None => Chat::new(&root, &mode, &model),
    };
    let (ui_tx, mut ui_rx) = mpsc::unbounded_channel::<UiMsg>();
    let onboarded = s.setting_bool("cliOnboarded", false);
    let mut app = App {
        root,
        model: if resume.is_some() { chat.model.clone() } else { model },
        mode: if resume.is_some() { chat.mode.clone() } else { mode },
        approval,
        target: Target::Project,
        entries: Vec::new(),
        input: String::new(),
        cursor: 0,
        scroll: 0,
        pending: None,
        overlay: None,
        onboard: if onboarded { None } else { Some(Onboard::Welcome) },
        onboard_sel: 0,
        signing_in: false,
        status: String::new(),
        tokens: (0, 0),
        last_input: 0,
        cost: 0.0,
        spinner: 0,
        palette_sel: 0,
        rc: None,
        rc_busy: false,
        rc_log: Vec::new(),
        rc_qr: None,
        rc_seq: 0,
        history: Vec::new(),
        hist_pos: None,
        always: HashSet::new(),
        running: None,
        ui_tx,
        models_dirty: false,
        detected_light: None,
        pick_after_reload: None,
        quit: false,
        presence: None,
        shown_what: String::new(),
        chat,
        s,
    };
    if app.s.setting_bool("discordStatus", true) {
        app.presence = blaze_core::presence::Presence::start();
    }
    let msgs = app.chat.messages.clone();
    app.load_transcript(&msgs);
    if let Some(e) = models_err {
        if app.s.key.is_some() {
            app.err(format!("Model list unavailable: {e}"));
        }
    }

    enable_raw_mode().map_err(|e| e.to_string())?;
    app.apply_theme(true);
    {
        let mode = crate::update::Mode::parse(&app.s.setting_str("cliUpdates")).unwrap_or(crate::update::Mode::Auto);
        let last = app.s.store.settings().get("cliUpdateChecked").and_then(|v| v.as_u64()).unwrap_or(0);
        let (http, tx) = (app.s.http.clone(), app.ui_tx.clone());
        tokio::spawn(async move {
            let (found, stamp) = crate::update::background(&http, mode, last).await;
            if stamp.is_some() {
                let _ = tx.send(UiMsg::Update(found, stamp));
            }
        });
    }
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, crossterm::event::EnableBracketedPaste).map_err(|e| e.to_string())?;
    let mut term = Terminal::new(CrosstermBackend::new(stdout)).map_err(|e| e.to_string())?;
    let p = app.project_name();
    app.show_working(&p);
    let result = event_loop(&mut term, &mut app, &mut ui_rx).await;
    let _ = execute!(term.backend_mut(), crossterm::terminal::SetTitle(""));
    if let Some(p) = app.presence.take() {
        p.clear();
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = disable_raw_mode();
    let _ = execute!(term.backend_mut(), crossterm::event::DisableBracketedPaste, LeaveAlternateScreen);
    let _ = term.show_cursor();
    if let Some(r) = app.running.take() {
        r.cancel.cancel();
        if let Ok(RunResult::Chat(conv, out)) = r.handle.await {
            app.chat.messages = conv;
            app.chat.add_checkpoint(r.user_index, &out.changed);
        }
    }
    let _ = app.chat.save();
    if !app.chat.messages.is_empty() {
        println!("Chat saved. Resume with: blaze -r {}", app.chat.id);
    }
    result
}

async fn event_loop(term: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App, ui_rx: &mut mpsc::UnboundedReceiver<UiMsg>) -> Result<(), String> {
    let (etx, mut erx) = mpsc::unbounded_channel::<AgentEvent>();
    let (htx, mut hrx) = mpsc::channel::<HostRequest>(4);
    let (ktx, mut krx) = mpsc::unbounded_channel::<Event>();
    std::thread::spawn(move || loop {
        if event::poll(Duration::from_millis(200)).unwrap_or(false) {
            match event::read() {
                Ok(ev) => {
                    if ktx.send(ev).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        } else if ktx.is_closed() {
            break;
        }
    });
    let mut tick = tokio::time::interval(Duration::from_millis(120));
    let mut n = 0u64;
    loop {
        term.draw(|f| render::draw(f, app)).map_err(|e| e.to_string())?;
        tokio::select! {
            Some(ev) = krx.recv() => {
                match ev {
                    Event::Key(k) if k.kind == KeyEventKind::Press => app.on_key(k, &etx, &htx),
                    Event::Paste(p) => {
                        if let Some(Overlay::Key(kp)) = app.overlay.as_mut() {
                            kp.value.push_str(p.trim());
                        } else if let Some(Overlay::Password(pw)) = app.overlay.as_mut() {
                            if pw.second { pw.b.push_str(p.trim()) } else { pw.a.push_str(p.trim()) }
                        } else if app.overlay.is_none() {
                            for c in p.chars() { app.insert(if c == '\r' { '\n' } else { c }); }
                        }
                    }
                    _ => {}
                }
            }
            Some(ev) = erx.recv() => app.on_agent(ev),
            Some(req) = hrx.recv() => app.on_host(req),
            Some(m) = ui_rx.recv() => {
                let reload = matches!(m, UiMsg::Login(Ok(_)));
                app.on_ui(m);
                if reload {
                    let _ = app.s.load_models(true).await;
                    app.status.clear();
                }
            }
            _ = tick.tick() => {
                app.spinner = app.spinner.wrapping_add(1);
                app.finish_if_done().await;
                n += 1;
                if n.is_multiple_of(8) { app.poll_rc(); }
                if app.models_dirty {
                    app.models_dirty = false;
                    let _ = app.s.load_models(true).await;
                    if let Some(f) = app.pick_after_reload.take() {
                        app.open_picker(PickKind::Model);
                        if let Some(Overlay::Picker(p)) = app.overlay.as_mut() {
                            p.filter = f;
                            p.sel = 0;
                        }
                    }
                }
            }
        }
        if app.quit {
            return Ok(());
        }
    }
}
