//! The agent loop in Rust: model round -> tool calls -> results -> next
//! round, until the model answers without tools. The IDE's chat panel keeps
//! its own loop in the UI (it drives the diff view, the preview browser and
//! revert checkpoints); this one runs where there is no window: the `blaze`
//! CLI, turns sent from a phone, and BlBots.
//!
//! The host (CLI, remote server, bot runner) sees what happens through
//! [`AgentEvent`]s and answers approvals and questions through
//! [`HostRequest`]s. Nothing here prints or blocks on a terminal.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Map, Value};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::ai::{self, AiRequest, ChatMessage, StreamEvent, Target, ToolCall};
use crate::models::ModelEntry;
use crate::paths::resolve_in;
use crate::providers::{self, CustomProvider};

/// How much the agent may do without asking.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Approval {
    /// Ask before every file change, command and paid generation.
    #[default]
    Ask,
    /// File changes inside the project are applied; commands and paid
    /// generations still ask. (What the IDE's Agent mode does.)
    Edits,
    /// Everything runs except destructive actions (see `risk`), which ask.
    Auto,
    /// YOLO: nothing asks.
    Full,
}

impl Approval {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ask" => Some(Approval::Ask),
            "edits" | "auto-edit" | "autoedit" => Some(Approval::Edits),
            "auto" => Some(Approval::Auto),
            "yolo" | "full" | "full-access" => Some(Approval::Full),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Approval::Ask => "Ask",
            Approval::Edits => "Approve edits",
            Approval::Auto => "Auto",
            Approval::Full => "YOLO",
        }
    }
}

/// Tools that never change anything.
const READ_ONLY: &[&str] = &["read_file", "list_directory", "search_files"];
const WRITES: &[&str] = &["write_file", "apply_diff"];

/// Does `level` let `tool` run without asking? Unknown tools always ask.
pub fn needs_approval(level: Approval, tool: &str) -> bool {
    if READ_ONLY.contains(&tool) || matches!(tool, "ask_question" | "write_plan") {
        return false;
    }
    match level {
        Approval::Full => false,
        // Commands are judged by what they run: see `risk_of`.
        Approval::Auto => !(WRITES.contains(&tool) || matches!(tool, "run_command" | "generate_image" | "generate_video")),
        Approval::Edits => !WRITES.contains(&tool),
        Approval::Ask => true,
    }
}

/// Why Auto still asks about this call, if it does.
pub fn risk_of(call: &ToolCall) -> Option<&'static str> {
    if !matches!(call.name.as_str(), "run_command" | "vm_exec") {
        return None;
    }
    let args: Map<String, Value> = serde_json::from_str(&call.arguments).unwrap_or_default();
    match args.get("command").and_then(Value::as_str) {
        Some(c) => crate::risk::command_risk(c),
        None => Some("the command could not be read"),
    }
}

/// Does this built-in call ask the user at `level`?
pub fn call_needs_approval(level: Approval, call: &ToolCall) -> bool {
    needs_approval(level, &call.name) || (level == Approval::Auto && risk_of(call).is_some())
}

/// What the host is told while a turn runs.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// A model round started (1-based).
    Round { n: usize },
    Text { text: String },
    Reasoning { text: String },
    ToolStart { id: String, name: String, summary: String },
    ToolEnd { id: String, name: String, result: String, is_error: bool },
    /// A file the agent changed (relative path, +lines, -lines).
    FileChanged { rel: String, added: usize, removed: usize, created: bool },
    Usage { input_tokens: u64, output_tokens: u64 },
    Notice { text: String },
    Error { message: String, status: Option<u16> },
    Done { stopped: bool, rounds: usize },
}

/// Something only the host (a person) can answer.
pub enum HostRequest {
    Approve { call: ToolCall, summary: String, reply: oneshot::Sender<bool> },
    /// `questions`: [{question, options?}]. Reply: one answer per question.
    Ask { questions: Vec<(String, Vec<String>)>, reply: oneshot::Sender<Vec<String>> },
}

/// Extra tools a host adds (BlBots: remember, hand_off, computer...).
pub trait ExtraTools: Send + Sync {
    fn schemas(&self) -> Vec<Value>;
    fn handles(&self, name: &str) -> bool;
    /// Run a call; returns the text for the model. `None` = treat like a
    /// built-in that needs approval at the agent's level (default: no).
    fn run<'a>(&'a self, call: &'a ToolCall) -> Pin<Box<dyn Future<Output = ToolOutput> + Send + 'a>>;
    /// Whether this tool asks for approval at the given level.
    fn needs_approval(&self, _level: Approval, _call: &ToolCall) -> bool {
        false
    }
}

/// A tool's answer: text for the model, plus images to show it on the next
/// round (screenshots), since tool results cannot carry images.
#[derive(Default, Clone, Debug)]
pub struct ToolOutput {
    pub text: String,
    pub images: Vec<String>,
}

impl From<String> for ToolOutput {
    fn from(text: String) -> Self {
        ToolOutput { text, images: Vec::new() }
    }
}

/// Custom provider id -> its key.
pub type KeyLookup = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Where requests go and with which credentials.
#[derive(Clone)]
pub struct Backend {
    pub http: reqwest::Client,
    pub http_custom: reqwest::Client,
    pub v1_base: String,
    pub key: Option<String>,
    /// BlazeAPI routing choice ("", "normal", "direct", "cheapest").
    pub routing: String,
    pub providers: Vec<CustomProvider>,
    /// Custom provider id -> its key (from the OS keychain).
    pub provider_key: KeyLookup,
    /// The account's models (for image/video candidates).
    pub models: Vec<ModelEntry>,
}

#[derive(Clone)]
pub struct AgentConfig {
    pub root: PathBuf,
    pub model: String,
    /// "ask" | "plan" | "agent".
    pub mode: String,
    pub reasoning_effort: Option<String>,
    pub approval: Approval,
    pub image_gen: bool,
    pub video_gen: bool,
    pub image_model: String,
    pub video_model: String,
    pub max_rounds: usize,
    pub extra_system: Option<String>,
    pub extra_tools: Option<Arc<dyn ExtraTools>>,
}

impl AgentConfig {
    pub fn new(root: PathBuf, model: &str, mode: &str) -> Self {
        AgentConfig {
            root,
            model: model.to_string(),
            mode: mode.to_string(),
            reasoning_effort: None,
            approval: Approval::Edits,
            image_gen: false,
            video_gen: false,
            image_model: String::new(),
            video_model: String::new(),
            max_rounds: 60,
            extra_system: None,
            extra_tools: None,
        }
    }
}

/// One file the turn changed, with its content before (None = new file).
#[derive(Clone, Debug, Serialize, serde::Deserialize, PartialEq)]
pub struct FileBefore {
    pub rel: String,
    pub before: Option<String>,
}

#[derive(Debug, Default)]
pub struct TurnOutcome {
    pub rounds: usize,
    pub stopped: bool,
    pub error: Option<String>,
    /// First "before" of every file the turn changed, for revert checkpoints.
    pub changed: Vec<FileBefore>,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

const ROUND_RETRIES: usize = 3;

fn retryable(status: Option<u16>) -> bool {
    match status {
        None => true,
        Some(s) => matches!(s, 408 | 409 | 425 | 429) || s >= 500,
    }
}

/// Short human summary of a call for cards and approval prompts.
pub fn summarize(call: &ToolCall) -> String {
    let args: Map<String, Value> = serde_json::from_str(&call.arguments).unwrap_or_default();
    let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let clip = |t: String| if t.chars().count() > 160 { format!("{}…", t.chars().take(160).collect::<String>()) } else { t };
    match call.name.as_str() {
        "read_file" => format!("Read {}", s("path")),
        "list_directory" => format!("List {}", if s("path").is_empty() { ".".into() } else { s("path") }),
        "search_files" => format!("Search for \"{}\"", s("query")),
        "write_file" => format!("Write {}", s("path")),
        "apply_diff" => format!("Edit {}", s("path")),
        "run_command" => format!("Run: {}", clip(s("command"))),
        "generate_image" => format!("Generate image {} ({})", s("path"), clip(s("prompt"))),
        "generate_video" => format!("Generate video {} ({})", s("path"), clip(s("prompt"))),
        "write_plan" => format!("Save plan: {}", s("title")),
        "ask_question" => "Ask you a question".into(),
        "web_search" => format!("Search the web for \"{}\"", s("query")),
        "web_fetch" => format!("Read {}", clip(s("url"))),
        "web_browser" => format!("Browser: {} {}", s("action"), clip(s("url"))).trim_end().to_string(),
        "web_interact" => {
            let n = args.get("element").map(Value::to_string).unwrap_or_default();
            match s("action").as_str() {
                "type" => format!("Browser: type \"{}\" into [{n}]{}", clip(s("text")), if args.get("submit") == Some(&Value::Bool(true)) { " and press Enter" } else { "" }),
                a => format!("Browser: {a} [{n}]"),
            }
        }
        "use_skill" => format!("Use skill: {} {}", s("name"), s("file")).trim_end().to_string(),
        other => clip(format!("{other} {}", if call.arguments.len() > 2 { call.arguments.as_str() } else { "" })),
    }
}

fn line_diff_counts(before: &str, after: &str) -> (usize, usize) {
    // Cheap estimate: lines only in one side (multiset difference).
    use std::collections::HashMap;
    let mut counts: HashMap<&str, isize> = HashMap::new();
    for l in before.lines() {
        *counts.entry(l).or_default() -= 1;
    }
    for l in after.lines() {
        *counts.entry(l).or_default() += 1;
    }
    let added = counts.values().filter(|v| **v > 0).map(|v| *v as usize).sum();
    let removed = counts.values().filter(|v| **v < 0).map(|v| (-*v) as usize).sum();
    (added, removed)
}

struct Run<'a> {
    backend: &'a Backend,
    cfg: &'a AgentConfig,
    events: &'a mpsc::UnboundedSender<AgentEvent>,
    host: &'a mpsc::Sender<HostRequest>,
    cancel: &'a CancellationToken,
    changed: Vec<FileBefore>,
}

impl Run<'_> {
    fn emit(&self, ev: AgentEvent) {
        let _ = self.events.send(ev);
    }

    async fn approve(&self, call: &ToolCall) -> bool {
        let (tx, rx) = oneshot::channel();
        let mut summary = summarize(call);
        if let Some(why) = risk_of(call).filter(|_| self.cfg.approval == Approval::Auto) {
            summary = format!("{summary}  (Auto asks because {why})");
        }
        if self.host.send(HostRequest::Approve { call: call.clone(), summary, reply: tx }).await.is_err() {
            return false;
        }
        tokio::select! {
            r = rx => r.unwrap_or(false),
            _ = self.cancel.cancelled() => false,
        }
    }

    fn remember_before(&mut self, rel: &str, before: Option<String>) {
        if !self.changed.iter().any(|c| c.rel == rel) {
            self.changed.push(FileBefore { rel: rel.to_string(), before });
        }
    }

    async fn write_change(&mut self, call: &ToolCall, args: &Map<String, Value>) -> String {
        let rel = args.get("path").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if rel.is_empty() {
            return "Error: missing required argument: path".into();
        }
        let abs = match resolve_in(&self.cfg.root, &rel) {
            Ok(p) => p,
            Err(e) => return format!("Error: {e}"),
        };
        let original = std::fs::read_to_string(&abs).ok();
        let current = original.clone().unwrap_or_default();
        let modified = if call.name == "apply_diff" {
            let diff = args.get("diff").and_then(Value::as_str).unwrap_or("");
            match crate::edit::apply_diff(&current, diff) {
                Ok(c) => c,
                Err(e) => return format!("Error: {e}"),
            }
        } else {
            args.get("content").and_then(Value::as_str).unwrap_or("").to_string()
        };
        if original.as_deref() == Some(modified.as_str()) {
            return format!("No change needed for {rel} (content identical).");
        }
        if let Err(e) = crate::files::write_text(&abs, &modified) {
            return format!("Error: could not write {rel}: {e}");
        }
        let (added, removed) = line_diff_counts(&current, &modified);
        self.emit(AgentEvent::FileChanged { rel: rel.clone(), added, removed, created: original.is_none() });
        self.remember_before(&rel, original);
        format!("Applied edit to {rel}.")
    }

    async fn write_plan(&mut self, args: &Map<String, Value>) -> String {
        let md = args.get("markdown").and_then(Value::as_str).unwrap_or("");
        let title = args.get("title").and_then(Value::as_str).unwrap_or("").trim();
        let summary = args.get("summary").and_then(Value::as_str).unwrap_or("").trim();
        if md.trim().is_empty() {
            return "Error: the plan was empty.".into();
        }
        let slug: String = {
            let raw: String = title.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
            let mut s = raw.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-");
            s.truncate(48);
            if s.is_empty() { "plan".into() } else { s }
        };
        let rel = format!(".blazeide/plans/{slug}.md");
        let abs = match resolve_in(&self.cfg.root, &rel) {
            Ok(p) => p,
            Err(e) => return format!("Error: {e}"),
        };
        let full = if !title.is_empty() && !md.trim_start().starts_with('#') { format!("# {title}\n\n{md}") } else { md.to_string() };
        if let Err(e) = crate::files::write_text(&abs, &full) {
            return format!("Error: could not save plan ({e}).");
        }
        self.emit(AgentEvent::Notice { text: format!("Plan saved to {rel}. {summary}") });
        format!("Plan saved to {rel} and summarized for the user. Waiting for the user to implement it.")
    }

    async fn ask(&self, args: &Map<String, Value>) -> String {
        let qs: Vec<(String, Vec<String>)> = args
            .get("questions")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|q| {
                        let text = q.get("question")?.as_str()?.to_string();
                        let opts = q.get("options").and_then(Value::as_array).map(|o| o.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
                        Some((text, opts))
                    })
                    .collect()
            })
            .unwrap_or_default();
        if qs.is_empty() {
            return "Error: ask_question needs at least one question.".into();
        }
        let (tx, rx) = oneshot::channel();
        if self.host.send(HostRequest::Ask { questions: qs.clone(), reply: tx }).await.is_err() {
            return "The user is not available to answer. Decide yourself and say what you assumed.".into();
        }
        let answers = tokio::select! {
            r = rx => r.unwrap_or_default(),
            _ = self.cancel.cancelled() => return "Stopped.".into(),
        };
        qs.iter()
            .enumerate()
            .map(|(i, (q, _))| format!("Q: {q}\nA: {}", answers.get(i).map(String::as_str).filter(|a| !a.is_empty()).unwrap_or("(no answer)")))
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    async fn media(&mut self, call: &ToolCall, args: &Map<String, Value>) -> String {
        let Some(key) = self.backend.key.as_deref() else { return "Error: not signed in to BlazeAPI.".into() };
        let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let rel = s("path");
        let before = resolve_in(&self.cfg.root, &rel).ok().and_then(|p| std::fs::read(p).ok()).map(|_| String::new());
        let ctx = crate::mediagen::MediaCtx {
            http: &self.backend.http,
            v1_base: &self.backend.v1_base,
            key,
            routing: &self.backend.routing,
            models: &self.backend.models,
            root: &self.cfg.root,
        };
        let out = if call.name == "generate_video" {
            if !self.cfg.video_gen {
                return "Error: video generation is turned off.".into();
            }
            let img = s("image");
            crate::mediagen::video(&ctx, &self.cfg.video_model, Some(img.as_str()), &rel, &s("prompt")).await
        } else {
            if !self.cfg.image_gen {
                return "Error: image generation is turned off.".into();
            }
            let aspect = s("aspect_ratio");
            crate::mediagen::image(&ctx, &self.cfg.image_model, "", &rel, &s("prompt"), Some(aspect.as_str()).filter(|a| !a.is_empty())).await
        };
        if !out.starts_with("Error") {
            self.emit(AgentEvent::FileChanged { rel: rel.clone(), added: 0, removed: 0, created: before.is_none() });
        }
        out
    }

    /// Run one call. Returns the text for the model (errors as "Error: ...").
    async fn exec(&mut self, call: &ToolCall) -> ToolOutput {
        let extra = self.cfg.extra_tools.clone();
        let is_extra = extra.as_ref().is_some_and(|e| e.handles(&call.name));
        let gated = if is_extra {
            extra.as_ref().is_some_and(|e| e.needs_approval(self.cfg.approval, call))
        } else {
            call_needs_approval(self.cfg.approval, call)
        };
        if !is_extra && !crate::tools::mode_allows(&self.cfg.mode, &call.name) {
            return format!("Error: {} is not available in {} mode.", call.name, self.cfg.mode).into();
        }
        if gated && !self.approve(call).await {
            return "User rejected this action.".to_string().into();
        }
        if let Some(e) = extra.filter(|_| is_extra) {
            return e.run(call).await;
        }
        let args: Map<String, Value> = match serde_json::from_str(&call.arguments) {
            Ok(a) => a,
            Err(_) => return "Error: tool arguments were not valid JSON.".to_string().into(),
        };
        let text = match call.name.as_str() {
            "write_file" | "apply_diff" => self.write_change(call, &args).await,
            "write_plan" => self.write_plan(&args).await,
            "ask_question" => self.ask(&args).await,
            "generate_image" | "generate_video" => self.media(call, &args).await,
            "browser" => "Error: the browser is only available in the Blaze IDE window.".into(),
            name => {
                let root = self.cfg.root.clone();
                let name = name.to_string();
                let a = call.arguments.clone();
                let cancel = self.cancel.clone();
                tokio::select! {
                    r = crate::tools::run_tool(&name, &a, &root) => r,
                    _ = cancel.cancelled() => "Stopped.".into(),
                }
            }
        };
        text.into()
    }

    /// Where this model's requests go (resolved once per round, so a key
    /// change or a removed provider is noticed).
    fn resolve(&self) -> Result<Resolved, String> {
        match providers::split_model_id(&self.cfg.model) {
            None => {
                let key = self.backend.key.clone().ok_or("Not signed in to BlazeAPI. Run `blaze login`.")?;
                Ok(Resolved { custom: false, wire: String::new(), base: self.backend.v1_base.clone(), key: Some(key), model: self.cfg.model.clone() })
            }
            Some((pid, m)) => {
                let p = self.backend.providers.iter().find(|p| p.id == pid).ok_or("That custom provider was removed. Pick another model.")?;
                Ok(Resolved { custom: true, wire: p.wire.clone(), base: p.base_url.clone(), key: (self.backend.provider_key)(pid), model: m.to_string() })
            }
        }
    }
}

struct Resolved {
    custom: bool,
    /// A custom provider's endpoint setting.
    wire: String,
    base: String,
    key: Option<String>,
    model: String,
}

impl Resolved {
    fn target<'a>(&'a self, routing: &str) -> Target<'a> {
        if self.custom {
            Target::custom(&self.base, self.key.as_deref()).with_wire(&self.wire)
        } else {
            Target::blaze_routed(&self.base, self.key.as_deref().unwrap_or(""), routing)
        }
    }
}

/// Run one turn. `conversation` must end with the new user message; the
/// assistant and tool messages of the turn are appended to it.
pub async fn run_turn(
    backend: &Backend,
    cfg: &AgentConfig,
    conversation: &mut Vec<ChatMessage>,
    events: &mpsc::UnboundedSender<AgentEvent>,
    host: &mpsc::Sender<HostRequest>,
    cancel: &CancellationToken,
) -> TurnOutcome {
    let mut run = Run { backend, cfg, events, host, cancel, changed: Vec::new() };
    let mut outcome = TurnOutcome::default();
    let mut retries = 0;
    let mut round = 0;
    let extra_tools = cfg.extra_tools.as_ref().map(|e| e.schemas()).unwrap_or_default();
    while round < cfg.max_rounds.max(1) {
        if cancel.is_cancelled() {
            outcome.stopped = true;
            break;
        }
        round += 1;
        run.emit(AgentEvent::Round { n: round });
        let resolved = match run.resolve() {
            Ok(r) => r,
            Err(e) => {
                run.emit(AgentEvent::Error { message: e.clone(), status: None });
                outcome.error = Some(e);
                break;
            }
        };
        let req = AiRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            model: resolved.model.clone(),
            messages: conversation.clone(),
            mode: cfg.mode.clone(),
            filename: None,
            // Third-party servers may reject OpenAI's newer parameters.
            reasoning_effort: if resolved.custom { Some("off".into()) } else { cfg.reasoning_effort.clone() },
            media_gen: Some(cfg.image_gen),
            video_gen: Some(cfg.video_gen),
            no_reasoning_echo: false,
            extra_system: cfg.extra_system.clone(),
            extra_tools: extra_tools.clone(),
            exclude_tools: vec!["browser".into()],
        };
        let mut text = String::new();
        let mut reasoning = String::new();
        let mut calls: Vec<ToolCall> = Vec::new();
        let http = if resolved.custom { &backend.http_custom } else { &backend.http };
        let target = resolved.target(&backend.routing);
        let res = ai::stream_chat(http, &target, &req, cancel.clone(), |ev| match ev {
            StreamEvent::Text { text: t } => {
                text.push_str(&t);
                let _ = events.send(AgentEvent::Text { text: t });
            }
            StreamEvent::Reasoning { text: t } => {
                reasoning.push_str(&t);
                let _ = events.send(AgentEvent::Reasoning { text: t });
            }
            StreamEvent::ToolCall { tool_call } => calls.push(tool_call),
            StreamEvent::Usage { input_tokens, output_tokens } => {
                let _ = events.send(AgentEvent::Usage { input_tokens, output_tokens });
            }
            StreamEvent::ToolDelta { .. } => {}
        })
        .await;
        drop(target);
        if cancel.is_cancelled() {
            if !text.is_empty() {
                conversation.push(ChatMessage { role: "assistant".into(), content: text, ..Default::default() });
            }
            outcome.stopped = true;
            break;
        }
        if let Err(e) = res {
            if calls.is_empty() {
                let status = e.status();
                if retryable(status) && retries < ROUND_RETRIES {
                    retries += 1;
                    run.emit(AgentEvent::Notice { text: format!("{e} Retrying ({retries}/{ROUND_RETRIES})…") });
                    let wait = if status == Some(429) { 5000 * retries as u64 } else { 1500 * retries as u64 };
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(wait)) => {}
                        _ = cancel.cancelled() => { outcome.stopped = true; break; }
                    }
                    round -= 1;
                    continue;
                }
                run.emit(AgentEvent::Error { message: e.to_string(), status });
                outcome.error = Some(e.to_string());
                break;
            }
        }
        retries = 0;
        if calls.is_empty() {
            if !text.is_empty() {
                conversation.push(ChatMessage { role: "assistant".into(), content: text, ..Default::default() });
            }
            break;
        }
        conversation.push(ChatMessage {
            role: "assistant".into(),
            content: text,
            tool_calls: Some(calls.clone()),
            reasoning: Some(reasoning).filter(|r| !r.is_empty()),
            ..Default::default()
        });
        let mut images: Vec<String> = Vec::new();
        for call in &calls {
            if cancel.is_cancelled() {
                conversation.push(ChatMessage { role: "tool".into(), content: "Stopped.".into(), tool_call_id: Some(call.id.clone()), ..Default::default() });
                continue;
            }
            run.emit(AgentEvent::ToolStart { id: call.id.clone(), name: call.name.clone(), summary: summarize(call) });
            let out = run.exec(call).await;
            let is_error = out.text.starts_with("Error") || out.text.starts_with("User rejected");
            run.emit(AgentEvent::ToolEnd { id: call.id.clone(), name: call.name.clone(), result: out.text.clone(), is_error });
            images.extend(out.images);
            conversation.push(ChatMessage { role: "tool".into(), content: out.text, tool_call_id: Some(call.id.clone()), ..Default::default() });
        }
        if !images.is_empty() {
            conversation.push(ChatMessage {
                role: "user".into(),
                content: "[Screenshot(s) you just took. Look at them carefully before continuing.]".into(),
                images: Some(images),
                ..Default::default()
            });
        }
        if cancel.is_cancelled() {
            outcome.stopped = true;
            break;
        }
        if round == cfg.max_rounds {
            run.emit(AgentEvent::Notice { text: format!("Paused after {round} steps so it never runs away with your balance. Say \"continue\" to go on.") });
        }
    }
    outcome.rounds = round;
    outcome.changed = std::mem::take(&mut run.changed);
    run.emit(AgentEvent::Done { stopped: outcome.stopped, rounds: round });
    outcome
}

/// A short title for a new chat from its first message.
pub fn title_from(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("New chat").trim();
    let mut t: String = line.chars().take(60).collect();
    if line.chars().count() > 60 {
        t.push('…');
    }
    t
}

/// Build the stored-chat JSON the IDE reads (`StoredChat` in src/shared/types.ts).
pub fn stored_chat(id: &str, title: &str, created_at: f64, mode: &str, model: &str, messages: &[ChatMessage], checkpoints: &[Value]) -> Value {
    json!({
        "id": id,
        "title": title,
        "createdAt": created_at,
        "updatedAt": now_ms(),
        "mode": mode,
        "model": model,
        "messages": messages,
        "checkpoints": checkpoints,
    })
}

pub fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0)
}

/// A revert checkpoint in the IDE's format for a turn that began at `user_index`.
pub fn checkpoint(user_index: usize, changed: &[FileBefore]) -> Option<Value> {
    if changed.is_empty() {
        return None;
    }
    Some(json!({ "userIndex": user_index, "files": changed }))
}

/// Make the project root canonical and check it is a folder.
pub fn project_root(p: &Path) -> crate::Result<PathBuf> {
    let c = dunce::canonicalize(p).map_err(|_| crate::Error::msg(format!("Not a folder: {}", p.display())))?;
    if !c.is_dir() {
        return Err(crate::Error::msg(format!("Not a folder: {}", c.display())));
    }
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A fake OpenAI endpoint: each POST gets the next canned SSE body.
    async fn fake_server(bodies: Vec<String>) -> (String, Arc<std::sync::Mutex<Vec<Value>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        tokio::spawn(async move {
            let mut bodies = bodies.into_iter();
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { break };
                let mut buf = Vec::new();
                let mut tmp = [0u8; 65536];
                // Read headers + body (Content-Length).
                loop {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(h) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..h]).to_ascii_lowercase();
                        let len = head.lines().find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                        if buf.len() >= h + 4 + len {
                            let body: Value = serde_json::from_slice(&buf[h + 4..h + 4 + len]).unwrap_or(Value::Null);
                            seen2.lock().unwrap().push(body);
                            break;
                        }
                    }
                }
                let body = bodies.next().unwrap_or_else(|| sse_text("done"));
                let resp = format!("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}", body.len(), body);
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.shutdown().await;
            }
        });
        (format!("http://{addr}"), seen)
    }

    fn sse_text(t: &str) -> String {
        format!("data: {}\n\ndata: [DONE]\n\n", json!({"choices":[{"delta":{"content":t}}]}))
    }

    fn sse_tool(id: &str, name: &str, args: Value) -> String {
        format!(
            "data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}}]}}]}),
            json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":5}})
        )
    }

    fn backend(base: &str) -> Backend {
        Backend {
            http: reqwest::Client::new(),
            http_custom: reqwest::Client::new(),
            v1_base: base.to_string(),
            key: Some("sk-test".into()),
            routing: String::new(),
            providers: Vec::new(),
            provider_key: Arc::new(|_| None),
            models: Vec::new(),
        }
    }

    async fn turn(base: &str, root: &Path, approval: Approval, answer: bool, prompt: &str) -> (TurnOutcome, Vec<ChatMessage>, Vec<AgentEvent>, usize) {
        let mut cfg = AgentConfig::new(root.to_path_buf(), "glm-5.3", "agent");
        cfg.approval = approval;
        let (etx, mut erx) = mpsc::unbounded_channel();
        let (htx, mut hrx) = mpsc::channel(4);
        let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let asked2 = asked.clone();
        tokio::spawn(async move {
            while let Some(r) = hrx.recv().await {
                asked2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                match r {
                    HostRequest::Approve { reply, .. } => {
                        let _ = reply.send(answer);
                    }
                    HostRequest::Ask { reply, questions } => {
                        let _ = reply.send(questions.iter().map(|_| "blue".to_string()).collect());
                    }
                }
            }
        });
        let mut conv = vec![ChatMessage { role: "user".into(), content: prompt.into(), ..Default::default() }];
        let out = run_turn(&backend(base), &cfg, &mut conv, &etx, &htx, &CancellationToken::new()).await;
        drop(etx);
        let mut evs = Vec::new();
        while let Some(e) = erx.recv().await {
            evs.push(e);
        }
        (out, conv, evs, asked.load(std::sync::atomic::Ordering::SeqCst))
    }

    #[tokio::test]
    async fn edits_a_file_then_answers() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        std::fs::write(root.join("a.txt"), "hello\nworld\n").unwrap();
        let diff = "<<<<<<< SEARCH\nworld\n=======\nthere\n>>>>>>> REPLACE";
        let (base, seen) = fake_server(vec![
            sse_tool("c1", "read_file", json!({"path":"a.txt"})),
            sse_tool("c2", "apply_diff", json!({"path":"a.txt","diff":diff})),
            sse_text("Done: changed world to there."),
        ])
        .await;
        let (out, conv, evs, asked) = turn(&base, &root, Approval::Edits, true, "change world").await;
        assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "hello\nthere\n");
        assert_eq!(out.rounds, 3);
        assert!(out.error.is_none() && !out.stopped);
        assert_eq!(asked, 0, "edits level applies file changes without asking");
        assert_eq!(out.changed, vec![FileBefore { rel: "a.txt".into(), before: Some("hello\nworld\n".into()) }]);
        assert_eq!(conv.last().unwrap().content, "Done: changed world to there.");
        assert!(conv.iter().any(|m| m.role == "tool" && m.content.contains("hello")));
        assert!(evs.iter().any(|e| matches!(e, AgentEvent::FileChanged { rel, added: 1, removed: 1, created: false } if rel == "a.txt")));
        assert!(matches!(evs.last(), Some(AgentEvent::Done { stopped: false, rounds: 3 })));
        // The second request carried the tool result back, and no browser tool.
        let reqs = seen.lock().unwrap();
        assert_eq!(reqs.len(), 3);
        assert!(reqs[1]["messages"].as_array().unwrap().iter().any(|m| m["role"] == "tool"));
        assert!(!reqs[0]["tools"].as_array().unwrap().iter().any(|t| t["function"]["name"] == "browser"));
    }

    #[tokio::test]
    async fn ask_level_asks_and_respects_no() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let (base, _) = fake_server(vec![sse_tool("c1", "write_file", json!({"path":"new.txt","content":"x"})), sse_text("ok")]).await;
        let (out, conv, _, asked) = turn(&base, &root, Approval::Ask, false, "make a file").await;
        assert_eq!(asked, 1);
        assert!(!root.join("new.txt").exists());
        assert!(out.changed.is_empty());
        assert!(conv.iter().any(|m| m.role == "tool" && m.content == "User rejected this action."));
    }

    #[tokio::test]
    async fn commands_ask_unless_full() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let (base, _) = fake_server(vec![sse_tool("c1", "run_command", json!({"command":"echo hi"})), sse_text("ran")]).await;
        let (_, conv, _, asked) = turn(&base, &root, Approval::Full, false, "run").await;
        assert_eq!(asked, 0);
        assert!(conv.iter().any(|m| m.role == "tool" && m.content.contains("hi")));
        let (base, _) = fake_server(vec![sse_tool("c1", "run_command", json!({"command":"echo hi"})), sse_text("ran")]).await;
        let (_, _, _, asked) = turn(&base, &root, Approval::Edits, true, "run").await;
        assert_eq!(asked, 1);
    }

    #[tokio::test]
    async fn questions_go_to_the_host_and_paths_stay_inside() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let (base, _) = fake_server(vec![
            sse_tool("c1", "ask_question", json!({"questions":[{"question":"Colour?","options":["red","blue"]}]})),
            sse_tool("c2", "write_file", json!({"path":"../escape.txt","content":"x"})),
            sse_text("ok"),
        ])
        .await;
        let (_, conv, _, _) = turn(&base, &root, Approval::Full, true, "go").await;
        assert!(conv.iter().any(|m| m.role == "tool" && m.content.contains("A: blue")));
        assert!(conv.iter().any(|m| m.role == "tool" && m.content.starts_with("Error:")));
        assert!(!root.parent().unwrap().join("escape.txt").exists());
    }

    #[tokio::test]
    async fn cancel_stops_the_turn() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let (base, _) = fake_server(vec![sse_tool("c1", "run_command", json!({"command":"sleep 30"}))]).await;
        let cfg = { let mut c = AgentConfig::new(root.clone(), "m", "agent"); c.approval = Approval::Full; c };
        let (etx, _erx) = mpsc::unbounded_channel();
        let (htx, _hrx) = mpsc::channel(1);
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(600)).await;
            c2.cancel();
        });
        let mut conv = vec![ChatMessage { role: "user".into(), content: "x".into(), ..Default::default() }];
        let t = std::time::Instant::now();
        let out = run_turn(&backend(&base), &cfg, &mut conv, &etx, &htx, &cancel).await;
        assert!(out.stopped);
        assert!(t.elapsed() < Duration::from_secs(10));
    }

    #[tokio::test]
    async fn plan_mode_cannot_write() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let (base, _) = fake_server(vec![sse_tool("c1", "write_file", json!({"path":"x.txt","content":"x"})), sse_text("ok")]).await;
        let mut cfg = AgentConfig::new(root.clone(), "m", "plan");
        cfg.approval = Approval::Full;
        let (etx, _erx) = mpsc::unbounded_channel();
        let (htx, _hrx) = mpsc::channel(1);
        let mut conv = vec![ChatMessage { role: "user".into(), content: "x".into(), ..Default::default() }];
        run_turn(&backend(&base), &cfg, &mut conv, &etx, &htx, &CancellationToken::new()).await;
        assert!(!root.join("x.txt").exists());
        assert!(conv.iter().any(|m| m.role == "tool" && m.content.contains("not available in plan mode")));
    }

    #[test]
    fn approval_rules() {
        assert!(!needs_approval(Approval::Ask, "read_file"));
        assert!(needs_approval(Approval::Ask, "write_file"));
        assert!(!needs_approval(Approval::Edits, "apply_diff"));
        assert!(needs_approval(Approval::Edits, "run_command"));
        assert!(needs_approval(Approval::Edits, "generate_image"));
        assert!(!needs_approval(Approval::Full, "run_command"));
        assert!(needs_approval(Approval::Ask, "some_new_tool"));
        assert_eq!(Approval::parse("Full-Access"), Some(Approval::Full));
        assert_eq!(Approval::parse("YOLO"), Some(Approval::Full));
        assert_eq!(Approval::parse("auto"), Some(Approval::Auto));
        assert_eq!(Approval::parse("auto-edit"), Some(Approval::Edits));
        let cmd = |c: &str| ToolCall { id: "1".into(), name: "run_command".into(), arguments: serde_json::json!({ "command": c }).to_string(), ..Default::default() };
        assert!(!call_needs_approval(Approval::Auto, &cmd("npm test")));
        assert!(call_needs_approval(Approval::Auto, &cmd("rm -rf dist")));
        assert!(call_needs_approval(Approval::Auto, &ToolCall { arguments: "{}".into(), ..cmd("x") }));
        assert!(!call_needs_approval(Approval::Full, &cmd("rm -rf dist")));
        assert!(call_needs_approval(Approval::Edits, &cmd("npm test")));
        assert!(!needs_approval(Approval::Auto, "write_file") && !needs_approval(Approval::Auto, "generate_image"));
        assert!(needs_approval(Approval::Auto, "some_new_tool"));
        assert_eq!(title_from("\n  Fix the login bug please\nmore"), "Fix the login bug please");
    }
}
