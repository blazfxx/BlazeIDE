//! Running bots: a turn in one of a bot's own threads, a turn in a room
//! (with hand-offs), and the scheduler for routines and check-backs.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{mpsc, Semaphore};
use tokio_util::sync::CancellationToken;

use super::room::{self, Room, RoomMsg, Rooms};
use super::tools::{BotTools, HandOff};
use super::{now_secs, persona, schedule, Bot, Bots, Engine};
use crate::agent::{self, AgentConfig, AgentEvent, Backend, ExtraTools, HostRequest, TurnOutcome};
use crate::ai::ChatMessage;
use crate::chatfile::ChatFile;

pub type ConfigFn = Arc<dyn Fn(&Path, &str, &str) -> AgentConfig + Send + Sync>;
pub type ComputerFn = Arc<dyn Fn(&Bot) -> Option<Arc<dyn ExtraTools>> + Send + Sync>;

/// What a host (IDE, CLI, phone server) gives the bot runner.
#[derive(Clone)]
pub struct BotCtx {
    pub config_dir: PathBuf,
    pub bots: Arc<Bots>,
    pub rooms: Arc<Rooms>,
    pub backend: Arc<dyn Fn() -> Backend + Send + Sync>,
    /// Base agent settings (reasoning effort, media generation) for a folder/model/mode.
    pub agent_config: ConfigFn,
    pub default_model: Arc<dyn Fn() -> String + Send + Sync>,
    /// The bot's private desktop tools, when it has one.
    pub computer: Option<ComputerFn>,
}

impl BotCtx {
    fn model_for(&self, bot: &Bot) -> String {
        if bot.model.trim().is_empty() { (self.default_model)() } else { bot.model.clone() }
    }

    fn config(&self, bot: &Bot, work: &Path, room: Option<&Room>, thread: Option<String>, hand_off: Option<HandOff>) -> AgentConfig {
        let model = self.model_for(bot);
        let mut cfg = (self.agent_config)(work, &model, &bot.mode);
        cfg.approval = bot.approval;
        let roster = self.bots.list();
        let mut sys = persona(bot, &self.bots.memory(&bot.id), &roster, room);
        if let Some(base) = cfg.extra_system.take() {
            sys.push('\n');
            sys.push_str(&base);
        }
        cfg.extra_system = Some(sys);
        let computer = self.computer.as_ref().and_then(|f| f(bot));
        cfg.extra_tools = Some(Arc::new(BotTools { bots: self.bots.clone(), bot_id: bot.id.clone(), thread_id: thread, hand_off, computer }));
        if bot.engine == Engine::Blaze {
            crate::skills::tool::for_bot(&mut cfg, &self.config_dir, &bot.skills);
        } else if let Some(list) = engine_skills(&self.config_dir, &bot.skills) {
            // A local CLI has its own file tools, not use_skill.
            let sys = cfg.extra_system.take().unwrap_or_default();
            cfg.extra_system = Some(format!("{sys}\n{list}"));
        }
        cfg
    }
}

fn engine_skills(config_dir: &Path, names: &[String]) -> Option<String> {
    let lib = crate::skills::Library::open(config_dir);
    let lines: Vec<String> = lib
        .list()
        .into_iter()
        .filter(|s| names.contains(&s.name))
        .filter_map(|s| {
            let md = lib.folder(&s.name)?.join("SKILL.md");
            Some(format!("- {}: {} (read {} first)", s.name, s.description.chars().take(160).collect::<String>(), md.display()))
        })
        .collect();
    // Kept short: it travels as a command-line argument.
    let mut out = String::from("<skills>\nWhen a task matches one of these skills, read its file and follow it.\n");
    for l in &lines {
        if out.len() + l.len() > 4_000 {
            break;
        }
        out.push_str(l);
        out.push('\n');
    }
    out.push_str("</skills>");
    (!lines.is_empty()).then_some(out)
}

/// A new thread for a bot (kept in the bot's folder, not the project's).
pub fn new_thread(ctx: &BotCtx, bot: &Bot) -> ChatFile {
    let work = ctx.bots.workdir(bot);
    let mut c = ChatFile::new(&work, &bot.mode, &ctx.model_for(bot));
    c.dir = ctx.bots.threads_dir(&bot.id);
    c.extra.insert("bot".into(), serde_json::Value::String(bot.id.clone()));
    c
}

pub fn load_thread(ctx: &BotCtx, bot: &Bot, id: &str) -> Result<ChatFile, String> {
    ChatFile::load_from(&ctx.bots.workdir(bot), &ctx.bots.threads_dir(&bot.id), id)
}

fn ensure_workdir(ctx: &BotCtx, bot: &Bot) -> Result<PathBuf, String> {
    let w = ctx.bots.workdir(bot);
    if bot.folder.trim().is_empty() {
        std::fs::create_dir_all(&w).map_err(|e| e.to_string())?;
    }
    agent::project_root(&w).map_err(|_| format!("{}'s folder {} does not exist", bot.name, w.display()))
}

#[allow(clippy::too_many_arguments)]
async fn run_engine(
    ctx: &BotCtx,
    bot: &Bot,
    cfg: &AgentConfig,
    conversation: &mut Vec<ChatMessage>,
    session_key: &str,
    events: &mpsc::UnboundedSender<AgentEvent>,
    host: &mpsc::Sender<HostRequest>,
    cancel: &CancellationToken,
) -> TurnOutcome {
    match &bot.engine {
        Engine::Blaze => agent::run_turn(&(ctx.backend)(), cfg, conversation, events, host, cancel).await,
        other => super::engines::run(other, &ctx.bots, bot, cfg, conversation, session_key, events, cancel).await,
    }
}

/// One turn in a bot's own thread (None = start a new thread).
pub async fn thread_turn(
    ctx: &BotCtx,
    bot_id: &str,
    thread: Option<&str>,
    text: &str,
    events: &mpsc::UnboundedSender<AgentEvent>,
    host: &mpsc::Sender<HostRequest>,
    cancel: &CancellationToken,
) -> Result<(String, TurnOutcome), String> {
    let bot = ctx.bots.get(bot_id).ok_or("No such bot")?;
    let work = ensure_workdir(ctx, &bot)?;
    // An id that does not exist yet starts a new thread under that id (the
    // phone picks the id first so it can follow the run).
    let mut chat = match thread {
        Some(t) => match load_thread(ctx, &bot, t) {
            Ok(c) => c,
            Err(_) if t.len() <= 64 && t.chars().all(|c| c.is_ascii_alphanumeric()) => {
                let mut c = new_thread(ctx, &bot);
                c.id = t.to_string();
                c
            }
            Err(e) => return Err(e),
        },
        None => new_thread(ctx, &bot),
    };
    chat.root = work.clone();
    chat.model = ctx.model_for(&bot);
    let user_index = chat.messages.len();
    chat.messages.push(ChatMessage { role: "user".into(), content: text.to_string(), ..Default::default() });
    chat.save()?;
    let cfg = ctx.config(&bot, &work, None, Some(chat.id.clone()), None);
    let key = format!("thread:{}", chat.id);
    let out = run_engine(ctx, &bot, &cfg, &mut chat.messages, &key, events, host, cancel).await;
    chat.add_checkpoint(user_index, &out.changed);
    chat.save()?;
    Ok((chat.id.clone(), out))
}

/// The room's history as one bot sees it.
fn room_view(room: &Room, bot: &Bot, roster: &[Bot]) -> Vec<ChatMessage> {
    let name = |id: &str| roster.iter().find(|b| b.id == id).map(|b| b.name.clone()).unwrap_or_else(|| "a bot".into());
    room.messages
        .iter()
        .filter(|m| !m.text.trim().is_empty())
        .map(|m| {
            if m.author == "user" {
                ChatMessage { role: "user".into(), content: m.text.clone(), ..Default::default() }
            } else if m.author == bot.id {
                ChatMessage { role: "assistant".into(), content: m.text.clone(), ..Default::default() }
            } else {
                ChatMessage { role: "user".into(), content: format!("[{}] {}", name(&m.author), m.text), ..Default::default() }
            }
        })
        .collect()
}

type RoomEvents = mpsc::UnboundedSender<(String, AgentEvent)>;

struct RoomRun {
    ctx: BotCtx,
    room: Arc<Mutex<Room>>,
    events: RoomEvents,
    host: mpsc::Sender<HostRequest>,
    cancel: CancellationToken,
}

/// One member's turn in a room; `task` = a hand-off from the lead.
fn member_turn(run: Arc<RoomRun>, bot: Bot, task: Option<(String, String)>, may_hand_off: bool) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send>> {
    Box::pin(async move {
        let roster = run.ctx.bots.list();
        let snapshot = run.room.lock().expect("room").clone();
        let work = if snapshot.folder.trim().is_empty() {
            match ensure_workdir(&run.ctx, &bot) {
                Ok(w) => w,
                Err(e) => return format!("Error: {e}"),
            }
        } else {
            PathBuf::from(&snapshot.folder)
        };
        let mut conv = room_view(&snapshot, &bot, &roster);
        if let Some((from, t)) = &task {
            conv.push(ChatMessage { role: "user".into(), content: format!("[{from}] {t}"), ..Default::default() });
        }
        if conv.last().is_none_or(|m| m.role != "user") {
            conv.push(ChatMessage { role: "user".into(), content: "(Your turn.)".into(), ..Default::default() });
        }
        let lead = snapshot.lead.as_deref() == Some(bot.id.as_str());
        let hand_off: Option<HandOff> = (lead && may_hand_off).then(|| {
            let run2 = run.clone();
            let me = bot.name.clone();
            let h: HandOff = Arc::new(move |name: String, t: String| {
                let run3 = run2.clone();
                let me = me.clone();
                Box::pin(async move {
                    let members = run3.room.lock().expect("room").members.clone();
                    let target = run3.ctx.bots.list().into_iter().find(|b| members.contains(&b.id) && b.name.eq_ignore_ascii_case(name.trim()));
                    match target {
                        Some(b) if b.name != me => {
                            let reply = member_turn(run3.clone(), b.clone(), Some((me.clone(), t)), false).await;
                            format!("{} answered:\n{reply}", b.name)
                        }
                        Some(_) => "Error: hand the task to another bot, not yourself.".into(),
                        None => format!("Error: no bot called {name} in this room."),
                    }
                }) as std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send>>
            });
            h
        });
        let cfg = run.ctx.config(&bot, &work, Some(&snapshot), None, hand_off);
        let (etx, mut erx) = mpsc::unbounded_channel::<AgentEvent>();
        let fwd_events = run.events.clone();
        let bid = bot.id.clone();
        let fwd = tokio::spawn(async move {
            while let Some(ev) = erx.recv().await {
                let _ = fwd_events.send((bid.clone(), ev));
            }
        });
        let start = conv.len();
        let key = format!("room:{}:{}", snapshot.id, bot.id);
        let out = run_engine(&run.ctx, &bot, &cfg, &mut conv, &key, &etx, &run.host, &run.cancel).await;
        drop(etx);
        let _ = fwd.await;
        let new = &conv[start..];
        let text = new.iter().filter(|m| m.role == "assistant").map(|m| m.content.trim()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n\n");
        let tools: Vec<String> = new.iter().filter(|m| m.role == "assistant").flat_map(|m| m.tool_calls.iter().flatten().map(agent::summarize)).collect();
        let text = match (&out.error, text.is_empty()) {
            (Some(e), true) => format!("(Could not answer: {e})"),
            _ => text,
        };
        let mut r = run.room.lock().expect("room");
        r.messages.push(RoomMsg { author: bot.id.clone(), text: text.clone(), tools, at: now_secs() });
        r.updated = now_secs();
        let _ = run.ctx.rooms.save(&r);
        text
    })
}

/// The user writes in a room; the addressed bots (or the lead) answer.
pub async fn room_turn(ctx: &BotCtx, room_id: &str, text: &str, events: &RoomEvents, host: &mpsc::Sender<HostRequest>, cancel: &CancellationToken) -> Result<Room, String> {
    let mut room = ctx.rooms.get(room_id).ok_or("No such room")?;
    room.messages.push(RoomMsg { author: "user".into(), text: text.to_string(), tools: vec![], at: now_secs() });
    room.updated = now_secs();
    ctx.rooms.save(&room).map_err(|e| e.to_string())?;
    let roster = ctx.bots.list();
    let names: Vec<(String, String)> = roster.iter().map(|b| (b.id.clone(), b.name.clone())).collect();
    let who = room::targets(text, &room, &names);
    let run = Arc::new(RoomRun { ctx: ctx.clone(), room: Arc::new(Mutex::new(room)), events: events.clone(), host: host.clone(), cancel: cancel.clone() });
    for id in who {
        if cancel.is_cancelled() {
            break;
        }
        if let Some(bot) = roster.iter().find(|b| b.id == id).cloned() {
            member_turn(run.clone(), bot, None, true).await;
        }
    }
    let r = run.room.lock().expect("room").clone();
    Ok(r)
}

/// What the scheduler reports.
#[derive(Clone, Debug)]
pub enum SchedEvent {
    Started { bot: String, thread: Option<String>, title: String },
    Finished { bot: String, thread: String, title: String, error: Option<String> },
}

/// Runs routines and check-backs while this process lives. Only one process
/// (IDE or `blaze serve`) schedules at a time: the other one sees the lock
/// taken and stays quiet.
pub async fn scheduler(ctx: Arc<BotCtx>, host: mpsc::Sender<HostRequest>, cancel: CancellationToken, on_event: Arc<dyn Fn(SchedEvent) + Send + Sync>) {
    let lock_path = ctx.bots.dir().join("scheduler.lock");
    let mut held: Option<std::fs::File> = None;
    let slots = Arc::new(Semaphore::new(2));
    let mut first = true;
    loop {
        if held.is_none() {
            let _ = std::fs::create_dir_all(ctx.bots.dir());
            if let Ok(f) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&lock_path) {
                if f.try_lock().is_ok() {
                    held = Some(f);
                }
            }
        }
        if held.is_some() {
            let now = now_secs();
            for bot in ctx.bots.list().into_iter().filter(|b| !b.hidden) {
                for r in bot.routines.iter().filter(|r| r.enabled) {
                    // Catch-up only applies on the first pass after start.
                    if !schedule::is_due(&r.schedule, r.last_run, bot.created, now, r.catch_up && first) {
                        continue;
                    }
                    let rid = r.id.clone();
                    let _ = ctx.bots.update(&bot.id, |b| {
                        if let Some(x) = b.routines.iter_mut().find(|x| x.id == rid) {
                            x.last_run = now;
                        }
                    });
                    let text = format!("[Routine \"{}\", {}] {}", r.title, r.schedule.describe(), r.prompt);
                    spawn_run(ctx.clone(), host.clone(), cancel.clone(), slots.clone(), on_event.clone(), bot.id.clone(), r.thread.clone(), r.title.clone(), text, Some(r.id.clone()));
                }
                for cb in bot.check_backs.iter().filter(|c| c.at <= now) {
                    let cid = cb.id.clone();
                    let _ = ctx.bots.update(&bot.id, |b| b.check_backs.retain(|c| c.id != cid));
                    let late = if now - cb.at > 300 { " (late: this computer was off or Blaze was closed)" } else { "" };
                    let text = format!("[Check back{late}] {}", cb.note);
                    spawn_run(ctx.clone(), host.clone(), cancel.clone(), slots.clone(), on_event.clone(), bot.id.clone(), Some(cb.thread.clone()), "Check back".into(), text, None);
                }
            }
            first = false;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(30)) => {}
            _ = cancel.cancelled() => break,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_run(ctx: Arc<BotCtx>, host: mpsc::Sender<HostRequest>, cancel: CancellationToken, slots: Arc<Semaphore>, on_event: Arc<dyn Fn(SchedEvent) + Send + Sync>, bot: String, thread: Option<String>, title: String, text: String, routine: Option<String>) {
    tokio::spawn(async move {
        let Ok(_permit) = slots.acquire_owned().await else { return };
        on_event(SchedEvent::Started { bot: bot.clone(), thread: thread.clone(), title: title.clone() });
        let (etx, mut erx) = mpsc::unbounded_channel();
        tokio::spawn(async move { while erx.recv().await.is_some() {} });
        let res = thread_turn(&ctx, &bot, thread.as_deref(), &text, &etx, &host, &cancel).await;
        match res {
            Ok((tid, out)) => {
                if let Some(rid) = routine {
                    let t = tid.clone();
                    let _ = ctx.bots.update(&bot, |b| {
                        if let Some(r) = b.routines.iter_mut().find(|r| r.id == rid) {
                            r.thread.get_or_insert(t);
                        }
                    });
                }
                on_event(SchedEvent::Finished { bot, thread: tid, title, error: out.error });
            }
            Err(e) => on_event(SchedEvent::Finished { bot, thread: thread.unwrap_or_default(), title, error: Some(e) }),
        }
    });
}

#[cfg(test)]
mod tests;
