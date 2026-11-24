//! `blaze bots ...`: manage BlBots and talk to them from the terminal.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use blaze_core::agent::{AgentConfig, AgentEvent, Approval, HostRequest};
use blaze_core::bots::room::{Room, Rooms};
use blaze_core::bots::runner::{self, BotCtx};
use blaze_core::bots::{self, Bot, Bots, Engine};
use clap::Subcommand;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::config;
use crate::session::Session;

pub fn ctx(s: &Session) -> BotCtx {
    let dir = config::config_dir();
    let backend = s.backend();
    let effort = s.setting_str("reasoningEffort");
    let (img, vid, im, vm) = (s.setting_bool("aiMediaGen", false), s.setting_bool("aiVideoGen", false), s.setting_str("imageModel"), s.setting_str("videoModel"));
    let model = s.default_model();
    let bots = Arc::new(Bots::open(&dir));
    let b2 = bots.clone();
    BotCtx {
        config_dir: dir.clone(),
        bots,
        rooms: Arc::new(Rooms::open(&dir)),
        backend: Arc::new(move || backend.clone()),
        agent_config: Arc::new(move |root, m, mode| {
            let mut c = AgentConfig::new(root.to_path_buf(), m, mode);
            c.reasoning_effort = Some(if effort.is_empty() { "medium".into() } else { effort.clone() });
            c.image_gen = img;
            c.video_gen = vid;
            c.image_model = im.clone();
            c.video_model = vm.clone();
            c
        }),
        default_model: Arc::new(move || model.clone()),
        computer: Some(Arc::new(move |bot: &Bot| bots::computer::for_bot(bot, b2.workdir(bot)))),
    }
}

#[derive(Subcommand)]
pub enum BotsCmd {
    /// List your bots and rooms.
    List,
    /// Create a bot.
    New {
        name: String,
        /// One line under the name.
        #[arg(long)]
        title: Option<String>,
        /// Personality / standing instructions (text, or @file.md).
        #[arg(long)]
        soul: Option<String>,
        /// blaze (default), claude, codex or grok.
        #[arg(long)]
        engine: Option<String>,
        #[arg(long)]
        model: Option<String>,
        /// Working folder (default: the bot's own folder).
        #[arg(long)]
        folder: Option<PathBuf>,
        /// ask (default), edits or full.
        #[arg(long)]
        approval: Option<String>,
        /// Give it a private Linux desktop (needs Docker).
        #[arg(long)]
        computer: bool,
    },
    /// Show one bot.
    Show { bot: String },
    /// Delete a bot (its threads and memory too).
    Rm { bot: String },
    /// Talk to a bot: one message, or an interactive chat without a message.
    Chat {
        bot: String,
        message: Option<String>,
        /// Continue a thread (see `blaze bots show`).
        #[arg(long)]
        thread: Option<String>,
    },
    /// Add a routine: `--daily 09:00`, `--weekdays 08:30`, `--every 6` (hours) or `--cron "0 9 * * 1"`.
    Routine {
        bot: String,
        title: String,
        prompt: String,
        #[arg(long)]
        daily: Option<String>,
        #[arg(long)]
        weekdays: Option<String>,
        #[arg(long)]
        every: Option<u32>,
        #[arg(long)]
        cron: Option<String>,
    },
    /// Create a room: `blaze bots room "Launch" Nova Orbit` (the first bot leads).
    Room { name: String, bots: Vec<String> },
    /// Say something in a room.
    Say { room: String, message: String },
}

fn engine_of(s: Option<&str>) -> Result<Engine, String> {
    Ok(match s.unwrap_or("blaze") {
        "blaze" | "blazeapi" => Engine::Blaze,
        "claude" => Engine::Claude { bin: None },
        "codex" => Engine::Codex { bin: None },
        "grok" => Engine::Grok { bin: None },
        other => return Err(format!("Unknown engine {other}: use blaze, claude, codex or grok")),
    })
}

fn find_room(rooms: &Rooms, key: &str) -> Option<Room> {
    rooms.list().into_iter().find(|r| r.id == key || r.name.eq_ignore_ascii_case(key))
}

/// Terminal host for bot runs: prints the stream, asks approvals on stdin.
async fn print_run<F, Fut, T>(f: F) -> T
where
    F: FnOnce(mpsc::UnboundedSender<(String, AgentEvent)>, mpsc::Sender<HostRequest>, CancellationToken) -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let (etx, mut erx) = mpsc::unbounded_channel::<(String, AgentEvent)>();
    let (htx, mut hrx) = mpsc::channel::<HostRequest>(4);
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            c2.cancel();
        }
    });
    let printer = tokio::spawn(async move {
        let names: std::collections::HashMap<String, String> = Bots::open(&config::config_dir()).list().into_iter().map(|b| (b.id, b.name)).collect();
        let mut last = String::new();
        let mut at_line_start = true;
        while let Some((bot, ev)) = erx.recv().await {
            if bot != last && !bot.is_empty() {
                println!("{}\n{}:", if at_line_start { "" } else { "\n" }, names.get(&bot).cloned().unwrap_or(bot.clone()));
                last = bot;
                at_line_start = true;
            }
            match ev {
                AgentEvent::Text { text } => {
                    print!("{text}");
                    let _ = std::io::stdout().flush();
                    at_line_start = text.ends_with('\n');
                }
                AgentEvent::ToolStart { summary, .. } => {
                    if !at_line_start {
                        println!();
                        at_line_start = true;
                    }
                    eprintln!("  ● {summary}")
                }
                AgentEvent::Error { message, .. } => eprintln!("  error: {message}"),
                AgentEvent::Notice { text } => eprintln!("  {text}"),
                _ => {}
            }
        }
        println!();
    });
    tokio::spawn(async move {
        while let Some(r) = hrx.recv().await {
            match r {
                HostRequest::Approve { summary, reply, .. } => {
                    eprint!("Allow? {summary} [y/N] ");
                    let mut l = String::new();
                    let _ = std::io::stdin().read_line(&mut l);
                    let _ = reply.send(matches!(l.trim(), "y" | "Y" | "yes"));
                }
                HostRequest::Ask { questions, reply } => {
                    let mut a = Vec::new();
                    for (q, o) in questions {
                        eprint!("{q}{}\n> ", if o.is_empty() { String::new() } else { format!(" ({})", o.join(" / ")) });
                        let mut l = String::new();
                        let _ = std::io::stdin().read_line(&mut l);
                        a.push(l.trim().to_string());
                    }
                    let _ = reply.send(a);
                }
            }
        }
    });
    let out = f(etx, htx, cancel).await;
    let _ = printer.await;
    out
}

pub async fn run(s: &mut Session, cmd: BotsCmd) -> Result<(), String> {
    let _ = s.load_models(false).await;
    let c = ctx(s);
    match cmd {
        BotsCmd::List => {
            let list = c.bots.list();
            if list.is_empty() {
                println!("No bots yet. Create one: blaze bots new Nova --title \"Writes tests\" --soul \"You are careful and brief.\"");
            }
            for b in list {
                println!("{} {:<20} {:<12} {:<22} {}", b.avatar, b.name, b.engine.label(), if b.model.is_empty() { "(default model)".into() } else { b.model.clone() }, b.title);
            }
            for r in c.rooms.list() {
                println!("# {:<20} {} bots", r.name, r.members.len());
            }
            Ok(())
        }
        BotsCmd::New { name, title, soul, engine, model, folder, approval, computer } => {
            let mut b = Bot::new(&name);
            b.title = title.unwrap_or_default();
            b.soul = match soul {
                Some(sl) if sl.starts_with('@') => std::fs::read_to_string(&sl[1..]).map_err(|e| format!("Could not read {}: {e}", &sl[1..]))?,
                Some(sl) => sl,
                None => String::new(),
            };
            b.engine = engine_of(engine.as_deref())?;
            b.model = model.unwrap_or_default();
            if let Some(f) = folder {
                b.folder = blaze_core::agent::project_root(&f).map_err(|e| e.to_string())?.display().to_string();
            }
            b.approval = approval.as_deref().map(|a| Approval::parse(a).ok_or("approval must be ask, edits or full")).transpose()?.unwrap_or(Approval::Ask);
            b.computer = computer;
            let b = c.bots.save(b).map_err(|e| e.to_string())?;
            println!("Created {} ({}). Talk to it: blaze bots chat {}", b.name, b.engine.label(), b.name);
            Ok(())
        }
        BotsCmd::Show { bot } => {
            let b = c.bots.get(&bot).ok_or("No such bot")?;
            println!("{} {}  {}\nengine: {}  model: {}  approval: {}  folder: {}", b.avatar, b.name, b.title, b.engine.label(), if b.model.is_empty() { "default" } else { &b.model }, b.approval.label(), c.bots.workdir(&b).display());
            if !b.soul.is_empty() {
                println!("\n{}\n", b.soul);
            }
            for r in &b.routines {
                println!("routine {:<20} {}{}", r.title, r.schedule.describe(), if r.enabled { "" } else { " (paused)" });
            }
            for t in blaze_core::chats::list_in(&c.bots.threads_dir(&b.id)).into_iter().take(10) {
                println!("thread {:<16} {}", t.id, t.title);
            }
            let m = c.bots.memory(&b.id);
            if !m.is_empty() {
                println!("\nmemory:\n{m}");
            }
            Ok(())
        }
        BotsCmd::Rm { bot } => {
            let b = c.bots.get(&bot).ok_or("No such bot")?;
            c.bots.delete(&b.id).map_err(|e| e.to_string())?;
            println!("Deleted {}.", b.name);
            Ok(())
        }
        BotsCmd::Chat { bot, message, mut thread } => {
            let b = c.bots.get(&bot).ok_or("No such bot")?;
            let one_shot = message.is_some();
            let mut msg = message;
            loop {
                let text = match msg.take() {
                    Some(m) => m,
                    None => {
                        print!("you> ");
                        let _ = std::io::stdout().flush();
                        let mut l = String::new();
                        if std::io::stdin().read_line(&mut l).unwrap_or(0) == 0 || l.trim().is_empty() || l.trim() == "/quit" {
                            break;
                        }
                        l.trim().to_string()
                    }
                };
                let th = thread.clone();
                let bid = b.id.clone();
                let r = print_run(|etx, htx, cancel| {
                    let c = c.clone();
                    async move {
                        let (atx, mut arx) = mpsc::unbounded_channel::<AgentEvent>();
                        let fwd = tokio::spawn({
                            let bid = bid.clone();
                            async move {
                                while let Some(e) = arx.recv().await {
                                    let _ = etx.send((bid.clone(), e));
                                }
                            }
                        });
                        let r = runner::thread_turn(&c, &bid, th.as_deref(), &text, &atx, &htx, &cancel).await;
                        drop(atx);
                        let _ = fwd.await;
                        r
                    }
                })
                .await?;
                thread = Some(r.0);
                if one_shot || !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                    break;
                }
            }
            if let Some(t) = thread {
                eprintln!("thread {t} · continue with: blaze bots chat {} --thread {t}", b.name);
            }
            Ok(())
        }
        BotsCmd::Routine { bot, title, prompt, daily, weekdays, every, cron } => {
            use blaze_core::bots::schedule::Schedule;
            let sched = match (daily, weekdays, every, cron) {
                (Some(t), None, None, None) => Schedule::Daily { time: t },
                (None, Some(t), None, None) => Schedule::Weekdays { time: t },
                (None, None, Some(h), None) => Schedule::Every { hours: h },
                (None, None, None, Some(e)) => Schedule::Cron { expr: e },
                _ => return Err("Give exactly one of --daily, --weekdays, --every or --cron".into()),
            };
            let b = c.bots.get(&bot).ok_or("No such bot")?;
            let r = bots::Routine { id: format!("t{}", &uuid::Uuid::new_v4().simple().to_string()[..8]), title, prompt, schedule: sched.clone(), enabled: true, catch_up: false, last_run: bots::now_secs(), thread: None };
            c.bots.update(&b.id, |x| x.routines.push(r)).map_err(|e| e.to_string())?;
            println!("Added: {}. Routines run while Blaze IDE or `blaze serve` is running.", sched.describe());
            Ok(())
        }
        BotsCmd::Room { name, bots: members } => {
            let mut ids = Vec::new();
            for m in &members {
                ids.push(c.bots.get(m).ok_or(format!("No bot called {m}"))?.id);
            }
            let r = Room::new(&name, ids);
            c.rooms.save(&r).map_err(|e| e.to_string())?;
            println!("Room {} created. Talk: blaze bots say \"{}\" \"@Name ...\" (no @ = the lead answers)", r.name, r.name);
            Ok(())
        }
        BotsCmd::Say { room, message } => {
            let r = find_room(&c.rooms, &room).ok_or("No such room")?;
            print_run(|etx, htx, cancel| {
                let c = c.clone();
                let rid = r.id.clone();
                async move { runner::room_turn(&c, &rid, &message, &etx, &htx, &cancel).await }
            })
            .await?;
            Ok(())
        }
    }
}
