//! The full-screen chat: transcript, input, approval prompts, model picker.
//! Keys: Enter send · Alt+Enter / Ctrl+J newline · Esc stop · Tab mode ·
//! Ctrl+L model · PgUp/PgDn scroll · Ctrl+C clear/stop/quit · /help.

mod render;

use std::collections::HashSet;
use std::io;
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

pub struct Picker {
    pub filter: String,
    pub sel: usize,
}

struct Running {
    cancel: CancellationToken,
    handle: JoinHandle<(Vec<ChatMessage>, TurnOutcome)>,
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
    pub entries: Vec<Entry>,
    pub input: String,
    pub cursor: usize,
    pub scroll: usize,
    pub pending: Option<Pending>,
    pub picker: Option<Picker>,
    pub status: String,
    pub tokens: (u64, u64),
    pub spinner: usize,
    history: Vec<String>,
    hist_pos: Option<usize>,
    always: HashSet<String>,
    running: Option<Running>,
    quit: bool,
}

impl App {
    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    pub fn elapsed(&self) -> u64 {
        self.running.as_ref().map(|r| r.started.elapsed().as_secs()).unwrap_or(0)
    }

    fn push(&mut self, e: Entry) {
        self.entries.push(e);
        self.scroll = 0;
    }

    fn load_transcript(&mut self) {
        self.entries.clear();
        for m in &self.chat.messages {
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
    }

    fn send(&mut self, text: String, events: &mpsc::UnboundedSender<AgentEvent>, host: &mpsc::Sender<HostRequest>) {
        if self.s.key.is_none() && !self.model.starts_with("custom:") {
            self.push(Entry::Error("Not signed in. Quit and run `blaze login` (or set BLAZE_API_KEY).".into()));
            return;
        }
        self.history.push(text.clone());
        self.hist_pos = None;
        self.push(Entry::User(text.clone()));
        let user_index = self.chat.messages.len();
        self.chat.messages.push(ChatMessage { role: "user".into(), content: text, ..Default::default() });
        self.chat.model = self.model.clone();
        self.chat.mode = self.mode.clone();
        let backend = self.s.backend();
        let cfg = self.s.agent_config(&self.root, &self.model, &self.mode, self.approval);
        let mut conv = self.chat.messages.clone();
        let (events, host) = (events.clone(), host.clone());
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        let handle = tokio::spawn(async move {
            let out = agent::run_turn(&backend, &cfg, &mut conv, &events, &host, &c2).await;
            (conv, out)
        });
        self.running = Some(Running { cancel, handle, user_index, started: Instant::now() });
    }

    async fn finish_if_done(&mut self) {
        let done = self.running.as_ref().is_some_and(|r| r.handle.is_finished());
        if !done {
            return;
        }
        let r = self.running.take().expect("running");
        if let Ok((conv, out)) = r.handle.await {
            self.chat.messages = conv;
            self.chat.add_checkpoint(r.user_index, &out.changed);
            if out.stopped {
                self.push(Entry::Notice("Stopped.".into()));
            }
            if let Err(e) = self.chat.save() {
                self.push(Entry::Error(format!("Could not save the chat: {e}")));
            }
        }
        if let Some(Pending::Approve { .. } | Pending::Ask { .. }) = &self.pending {
            self.pending = None;
        }
    }

    fn on_agent(&mut self, ev: AgentEvent) {
        match ev {
            AgentEvent::Text { text } => match self.entries.last_mut() {
                Some(Entry::Assistant(t)) => t.push_str(&text),
                _ => self.push(Entry::Assistant(text)),
            },
            AgentEvent::ToolStart { summary, .. } => self.push(Entry::Tool { summary, state: ToolState::Running }),
            AgentEvent::ToolEnd { result, is_error, .. } => {
                if let Some(Entry::Tool { state, .. }) = self.entries.iter_mut().rev().find(|e| matches!(e, Entry::Tool { state: ToolState::Running, .. })) {
                    *state = if is_error { ToolState::Failed(result.lines().next().unwrap_or("").to_string()) } else { ToolState::Ok };
                }
            }
            AgentEvent::FileChanged { rel, added, removed, created } => self.push(Entry::File { rel, added, removed, created }),
            AgentEvent::Usage { input_tokens, output_tokens } => {
                self.tokens.0 += input_tokens;
                self.tokens.1 += output_tokens;
            }
            AgentEvent::Notice { text } => self.push(Entry::Notice(text)),
            AgentEvent::Error { message, .. } => self.push(Entry::Error(message)),
            AgentEvent::Round { .. } | AgentEvent::Reasoning { .. } | AgentEvent::Done { .. } => {}
        }
    }

    fn on_host(&mut self, req: HostRequest) {
        match req {
            HostRequest::Approve { call, summary, reply } => {
                if self.always.contains(&call.name) {
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

    fn command(&mut self, line: &str) {
        let mut parts = line.trim().splitn(2, ' ');
        let cmd = parts.next().unwrap_or("");
        let arg = parts.next().unwrap_or("").trim().to_string();
        match cmd {
            "/help" | "/?" => self.push(Entry::Notice(HELP.into())),
            "/quit" | "/exit" | "/q" => self.quit = true,
            "/new" | "/clear" => {
                self.chat = Chat::new(&self.root, &self.mode, &self.model);
                self.entries.clear();
                self.tokens = (0, 0);
                self.push(Entry::Notice("New chat.".into()));
            }
            "/model" => {
                if arg.is_empty() {
                    self.picker = Some(Picker { filter: String::new(), sel: 0 });
                } else {
                    self.set_model(arg);
                }
            }
            "/mode" => match arg.as_str() {
                "ask" | "plan" | "agent" => {
                    self.mode = arg;
                    self.push(Entry::Notice(format!("Mode: {}", self.mode)));
                }
                _ => self.push(Entry::Error("Use /mode ask, /mode plan or /mode agent.".into())),
            },
            "/approval" => match Approval::parse(&arg) {
                Some(a) => {
                    self.approval = a;
                    self.s.set_setting("cliApproval", serde_json::json!(match a { Approval::Ask => "ask", Approval::Edits => "edits", Approval::Full => "full" }));
                    self.push(Entry::Notice(format!("Approval: {}", a.label())));
                }
                None => self.push(Entry::Error("Use /approval ask, /approval edits or /approval full.".into())),
            },
            "/chats" => {
                let list = blaze_core::chats::list(&self.root);
                let text = if list.is_empty() {
                    "No saved chats in this project.".to_string()
                } else {
                    list.iter().take(15).map(|c| format!("{}  {} ({} msgs)", c.id, c.title, c.message_count)).collect::<Vec<_>>().join("\n") + "\n\nResume one with /resume <id>."
                };
                self.push(Entry::Notice(text));
            }
            "/resume" => match Chat::load(&self.root, &arg) {
                Ok(c) => {
                    self.model = c.model.clone();
                    self.mode = c.mode.clone();
                    self.chat = c;
                    self.load_transcript();
                    self.push(Entry::Notice(format!("Resumed \"{}\".", self.chat.title)));
                }
                Err(e) => self.push(Entry::Error(e)),
            },
            "/bots" => {
                let bots = blaze_core::bots::Bots::open(&crate::config::config_dir()).list();
                let text = if bots.is_empty() {
                    "No BlBots yet. Create one in Blaze IDE or with: blaze bots new Nova --soul \"...\"".to_string()
                } else {
                    bots.iter().map(|b| format!("{} {}  {} · {}", b.avatar, b.name, b.engine.label(), b.title)).collect::<Vec<_>>().join("\n") + "\n\nTalk to one: blaze bots chat <name> (or from your phone / Blaze IDE)."
                };
                self.push(Entry::Notice(text));
            }
            "/mobile" => self.push(Entry::Notice("Continue on mobile: run `blaze serve --relay` (anywhere), `--tunnel`, `--lan` or `--domain https://…` in another terminal, or turn it on in Blaze IDE > Settings.".into())),
            "/cost" | "/tokens" => self.push(Entry::Notice(format!("This session: {} input + {} output tokens.", self.tokens.0, self.tokens.1))),
            "/login" | "/logout" => self.push(Entry::Notice(format!("Quit and run `blaze {}` in your terminal.", &cmd[1..]))),
            _ => self.push(Entry::Error(format!("Unknown command {cmd}. Type /help."))),
        }
    }

    fn set_model(&mut self, id: String) {
        if !self.s.models.is_empty() && self.s.find_model(&id).is_none() {
            self.push(Entry::Error(format!("{id} is not in your model list (see /model).")));
            return;
        }
        self.model = id;
        self.s.set_setting("cliModel", serde_json::json!(self.model));
        self.push(Entry::Notice(format!("Model: {}", self.model)));
    }

    pub fn picker_items(&self) -> Vec<&blaze_core::models::ModelEntry> {
        let f = self.picker.as_ref().map(|p| p.filter.to_lowercase()).unwrap_or_default();
        self.s.models.iter().filter(|m| m.kind == "chat" && (f.is_empty() || m.id.to_lowercase().contains(&f) || m.display_name.to_lowercase().contains(&f))).collect()
    }

    fn on_key(&mut self, k: KeyEvent, events: &mpsc::UnboundedSender<AgentEvent>, host: &mpsc::Sender<HostRequest>) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);

        // Model picker.
        if let Some(p) = self.picker.as_mut() {
            match k.code {
                KeyCode::Esc => self.picker = None,
                KeyCode::Up => p.sel = p.sel.saturating_sub(1),
                KeyCode::Down => p.sel += 1,
                KeyCode::Backspace => {
                    p.filter.pop();
                    p.sel = 0;
                }
                KeyCode::Char(c) if !ctrl => {
                    p.filter.push(c);
                    p.sel = 0;
                }
                KeyCode::Enter => {
                    let items = self.picker_items();
                    let sel = self.picker.as_ref().map(|p| p.sel).unwrap_or(0).min(items.len().saturating_sub(1));
                    let id = items.get(sel).map(|m| m.id.clone());
                    self.picker = None;
                    if let Some(id) = id {
                        self.set_model(id);
                    }
                }
                _ => {}
            }
            return;
        }

        // Approval prompt.
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
                        self.push(Entry::Notice(format!("Refused: {summary}")));
                    }
                }
            }
            return;
        }

        match k.code {
            KeyCode::Char('c') if ctrl => {
                if let Some(r) = &self.running {
                    r.cancel.cancel();
                } else if !self.input.is_empty() {
                    self.input.clear();
                    self.cursor = 0;
                } else {
                    self.quit = true;
                }
            }
            KeyCode::Char('d') if ctrl && self.input.is_empty() => self.quit = true,
            KeyCode::Char('l') if ctrl => self.picker = Some(Picker { filter: String::new(), sel: 0 }),
            KeyCode::Esc => {
                if let Some(r) = &self.running {
                    r.cancel.cancel();
                }
            }
            KeyCode::Tab => {
                self.mode = match self.mode.as_str() {
                    "ask" => "plan".into(),
                    "plan" => "agent".into(),
                    _ => "ask".into(),
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
            KeyCode::Char(c) if !ctrl => self.insert(c),
            _ => {}
        }
    }

    fn insert(&mut self, c: char) {
        self.input.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    fn submit(&mut self, events: &mpsc::UnboundedSender<AgentEvent>, host: &mpsc::Sender<HostRequest>) {
        let text = std::mem::take(&mut self.input);
        self.cursor = 0;
        // Answering ask_question.
        if let Some(Pending::Ask { questions, answers, .. }) = self.pending.as_mut() {
            let i = answers.len();
            let picked = text.trim().parse::<usize>().ok().and_then(|n| questions.get(i).and_then(|(_, o)| o.get(n.wrapping_sub(1)).cloned()));
            answers.push(picked.unwrap_or_else(|| text.trim().to_string()));
            if answers.len() >= questions.len() {
                if let Some(Pending::Ask { answers, reply, .. }) = self.pending.take() {
                    self.push(Entry::Notice(format!("You answered: {}", answers.join(" · "))));
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
            self.command(trimmed);
            return;
        }
        if self.is_running() {
            // Keep it for after this turn.
            self.input = text;
            self.cursor = self.input.len();
            self.status = "Still working: press Esc to stop, or wait.".into();
            return;
        }
        self.send(trimmed.to_string(), events, host);
    }
}

pub const HELP: &str = "Keys: Enter send · Alt+Enter or Ctrl+J new line · Esc stop · Tab switch mode (ask/plan/agent) · Ctrl+L pick model · PgUp/PgDn scroll · Ctrl+C clear, stop or quit.
Commands: /model [id] · /mode ask|plan|agent · /approval ask|edits|full · /new · /chats · /resume <id> · /bots · /mobile · /cost · /quit
Approval levels: ask = every change and command asks · edits = file edits apply, commands ask · full = nothing asks.
Chats are saved in .blazeide/chats and open in Blaze IDE too.";

pub async fn run(mut s: Session, root: PathBuf, model: String, mode: String, approval: Approval, resume: Option<String>) -> Result<(), String> {
    let models_err = s.load_models(false).await.err();
    let chat = match &resume {
        Some(id) => Chat::load(&root, id)?,
        None => Chat::new(&root, &mode, &model),
    };
    let mut app = App {
        s,
        root,
        model: if resume.is_some() { chat.model.clone() } else { model },
        mode: if resume.is_some() { chat.mode.clone() } else { mode },
        approval,
        chat,
        entries: Vec::new(),
        input: String::new(),
        cursor: 0,
        scroll: 0,
        pending: None,
        picker: None,
        status: String::new(),
        tokens: (0, 0),
        spinner: 0,
        history: Vec::new(),
        hist_pos: None,
        always: HashSet::new(),
        running: None,
        quit: false,
    };
    app.load_transcript();
    if app.entries.is_empty() {
        let who = app.s.account_name().map(|a| format!(" Signed in as {a}.")).unwrap_or_else(|| if app.s.key.is_some() { " Signed in with a key.".into() } else { " Not signed in: run `blaze login`.".into() });
        app.entries.push(Entry::Notice(format!("Blaze · {}.{who} Type /help for keys and commands.", app.root.display())));
    }
    if let Some(e) = models_err {
        app.entries.push(Entry::Error(format!("Model list unavailable: {e}")));
    }

    enable_raw_mode().map_err(|e| e.to_string())?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).map_err(|e| e.to_string())?;
    let mut term = Terminal::new(CrosstermBackend::new(stdout)).map_err(|e| e.to_string())?;
    let result = event_loop(&mut term, &mut app).await;
    let _ = disable_raw_mode();
    let _ = execute!(term.backend_mut(), LeaveAlternateScreen);
    let _ = term.show_cursor();
    if let Some(r) = app.running.take() {
        r.cancel.cancel();
        if let Ok((conv, out)) = r.handle.await {
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

async fn event_loop(term: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &mut App) -> Result<(), String> {
    let (etx, mut erx) = mpsc::unbounded_channel::<AgentEvent>();
    let (htx, mut hrx) = mpsc::channel::<HostRequest>(4);
    let (ktx, mut krx) = mpsc::unbounded_channel::<Event>();
    // Terminal input on its own thread (crossterm's read blocks).
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
    loop {
        term.draw(|f| render::draw(f, app)).map_err(|e| e.to_string())?;
        tokio::select! {
            Some(ev) = krx.recv() => {
                if let Event::Key(k) = ev {
                    if k.kind == KeyEventKind::Press {
                        app.on_key(k, &etx, &htx);
                    }
                } else if let Event::Paste(p) = ev {
                    for c in p.chars() { app.insert(c); }
                }
            }
            Some(ev) = erx.recv() => app.on_agent(ev),
            Some(req) = hrx.recv() => app.on_host(req),
            _ = tick.tick() => {
                app.spinner = app.spinner.wrapping_add(1);
                app.finish_if_done().await;
            }
        }
        if app.quit {
            return Ok(());
        }
    }
}
