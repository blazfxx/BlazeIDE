//! BlBots in Blaze IDE: commands for the BlBots tab, live events to the
//! window, approval cards, and the routine scheduler while the app runs.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use blaze_core::agent::{AgentConfig, AgentEvent, Backend, HostRequest};
use blaze_core::bots::room::{Room, Rooms};
use blaze_core::bots::runner::{self, BotCtx, SchedEvent};
use blaze_core::bots::{self, Bot, Bots};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::{secrets, AppState};

type CmdResult<T> = Result<T, String>;

enum Reply {
    Approve(oneshot::Sender<bool>),
    Ask(oneshot::Sender<Vec<String>>),
}

static RUNS: Mutex<Option<HashMap<String, CancellationToken>>> = Mutex::new(None);
static PENDING: Mutex<Option<HashMap<String, (String, Value, Reply)>>> = Mutex::new(None);

fn runs<R>(f: impl FnOnce(&mut HashMap<String, CancellationToken>) -> R) -> R {
    f(RUNS.lock().expect("runs").get_or_insert_with(HashMap::new))
}

fn pending<R>(f: impl FnOnce(&mut HashMap<String, (String, Value, Reply)>) -> R) -> R {
    f(PENDING.lock().expect("pending").get_or_insert_with(HashMap::new))
}

pub fn ctx(app: &AppHandle) -> BotCtx {
    let dir = app.path().app_config_dir().unwrap_or_default();
    let bots_store = Arc::new(Bots::open(&dir));
    let (a1, a2, a3) = (app.clone(), app.clone(), app.clone());
    let b2 = bots_store.clone();
    BotCtx {
        config_dir: dir.clone(),
        bots: bots_store,
        rooms: Arc::new(Rooms::open(&dir)),
        backend: Arc::new(move || {
            let st = a1.state::<AppState>();
            let models = st.models.read().expect("models lock").clone();
            let b = Backend {
                http: st.http.clone(),
                http_custom: st.http_custom.clone(),
                v1_base: st.v1_base.clone(),
                key: st.api_key(),
                routing: crate::commands::routing_for(&st),
                providers: st.store.providers(),
                provider_key: Arc::new(secrets::load_provider_key),
                models,
            };
            b
        }),
        agent_config: Arc::new(move |root, model, mode| {
            let st = a2.state::<AppState>();
            let s = st.store.settings();
            let str_of = |k: &str| s.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            let bool_of = |k: &str| s.get(k).and_then(Value::as_bool).unwrap_or(false);
            let mut c = AgentConfig::new(root.to_path_buf(), model, mode);
            let effort = str_of("reasoningEffort");
            c.reasoning_effort = Some(if effort.is_empty() { "medium".into() } else { effort });
            c.image_gen = bool_of("aiMediaGen");
            c.video_gen = bool_of("aiVideoGen");
            c.image_model = str_of("imageModel");
            c.video_model = str_of("videoModel");
            c
        }),
        default_model: Arc::new(move || {
            let st = a3.state::<AppState>();
            let m = st.store.settings().get("selectedModel").and_then(Value::as_str).unwrap_or("").to_string();
            if m.is_empty() { blaze_core::chatfile::DEFAULT_MODEL.into() } else { m }
        }),
        computer: Some(Arc::new(move |bot: &Bot| bots::computer::for_bot(bot, b2.workdir(bot)))),
    }
}

/// Approval cards from bot runs go to the window (BlBots tab).
fn host_channel(app: &AppHandle, run_id: &str, chat_id: &str) -> mpsc::Sender<HostRequest> {
    let (tx, mut rx) = mpsc::channel::<HostRequest>(4);
    let (app, run_id, chat_id) = (app.clone(), run_id.to_string(), chat_id.to_string());
    tauri::async_runtime::spawn(async move {
        while let Some(req) = rx.recv().await {
            let id = format!("p{}", &uuid_simple()[..12]);
            let (view, reply) = match req {
                HostRequest::Approve { call, summary, reply } => (json!({ "id": id, "kind": "approve", "runId": run_id, "chatId": chat_id, "tool": call.name, "summary": summary }), Reply::Approve(reply)),
                HostRequest::Ask { questions, reply } => {
                    let qs: Vec<Value> = questions.iter().map(|(q, o)| json!({ "question": q, "options": o })).collect();
                    (json!({ "id": id, "kind": "ask", "runId": run_id, "chatId": chat_id, "questions": qs }), Reply::Ask(reply))
                }
            };
            pending(|p| p.insert(id, (run_id.clone(), view.clone(), reply)));
            let _ = app.emit("bot-pending", view);
        }
    });
    tx
}

fn uuid_simple() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

fn finish(app: &AppHandle, run_id: &str, chat_id: &str, error: Option<String>, stopped: bool) {
    runs(|r| r.remove(run_id));
    pending(|p| p.retain(|_, (rid, _, _)| rid != run_id));
    let _ = app.emit("bot-run-end", json!({ "runId": run_id, "chatId": chat_id, "error": error, "stopped": stopped }));
}

// ---- commands -------------------------------------------------------------

#[tauri::command]
pub fn bots_list(app: AppHandle) -> Value {
    let c = ctx(&app);
    let rooms: Vec<Value> = c.rooms.list().iter().map(|r| json!({ "id": r.id, "name": r.name, "members": r.members, "lead": r.lead, "instructions": r.instructions, "folder": r.folder, "updated": r.updated })).collect();
    let engines: Vec<Value> = bots::engines::available().into_iter().map(|(n, p)| json!({ "name": n, "path": p })).collect();
    json!({ "bots": c.bots.list(), "rooms": rooms, "engines": engines })
}

#[tauri::command]
pub fn bots_save(app: AppHandle, bot: Bot) -> CmdResult<Bot> {
    let c = ctx(&app);
    let mut bot = bot;
    if !bot.folder.trim().is_empty() {
        bot.folder = blaze_core::agent::project_root(std::path::Path::new(bot.folder.trim())).map_err(|e| e.to_string())?.display().to_string();
    }
    // Keep fields the editor does not show (check-backs, routine run times).
    if let Some(old) = c.bots.get(&bot.id) {
        bot.check_backs = old.check_backs;
        for r in &mut bot.routines {
            if let Some(o) = old.routines.iter().find(|o| o.id == r.id) {
                r.last_run = o.last_run;
                r.thread = o.thread.clone();
            } else if r.last_run == 0 {
                r.last_run = bots::now_secs();
            }
        }
    }
    c.bots.save(bot).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn bots_delete(app: AppHandle, id: String) -> CmdResult<()> {
    let _ = bots::computer::stop(&id).await;
    ctx(&app).bots.delete(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn bots_duplicate(app: AppHandle, id: String) -> CmdResult<Bot> {
    ctx(&app).bots.duplicate(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn bots_threads(app: AppHandle, bot: String) -> CmdResult<Value> {
    let c = ctx(&app);
    let b = c.bots.get(&bot).ok_or("No such bot")?;
    Ok(json!({ "threads": blaze_core::chats::list_in(&c.bots.threads_dir(&b.id)), "memory": c.bots.memory(&b.id) }))
}

#[tauri::command]
pub fn bots_thread(app: AppHandle, bot: String, id: String) -> CmdResult<Value> {
    let c = ctx(&app);
    let b = c.bots.get(&bot).ok_or("No such bot")?;
    let t = runner::load_thread(&c, &b, &id)?;
    let pend: Vec<Value> = pending(|p| p.values().filter(|(_, v, _)| v["chatId"] == id.as_str()).map(|(_, v, _)| v.clone()).collect());
    Ok(json!({ "id": t.id, "title": t.title, "messages": blaze_core::remote::phone_view(&t.messages), "pending": pend }))
}

#[tauri::command]
pub fn bots_set_memory(app: AppHandle, bot: String, text: String) -> CmdResult<()> {
    ctx(&app).bots.set_memory(&bot, &text).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn bots_send(app: AppHandle, bot: String, thread: Option<String>, text: String) -> CmdResult<Value> {
    let text = text.trim().to_string();
    if text.is_empty() || text.len() > 100_000 {
        return Err("Write a message first".into());
    }
    let c = ctx(&app);
    let b = c.bots.get(&bot).ok_or("No such bot")?;
    let thread = thread.filter(|t| !t.is_empty()).unwrap_or_else(|| format!("c{}", &uuid_simple()[..12]));
    let run_id = format!("r{}", &uuid_simple()[..12]);
    let cancel = CancellationToken::new();
    runs(|r| r.insert(run_id.clone(), cancel.clone()));
    let host = host_channel(&app, &run_id, &thread);
    let (app2, rid, tid, bid) = (app.clone(), run_id.clone(), thread.clone(), b.id.clone());
    tauri::async_runtime::spawn(async move {
        let (etx, mut erx) = mpsc::unbounded_channel::<AgentEvent>();
        let (a3, r3, t3, b3) = (app2.clone(), rid.clone(), tid.clone(), bid.clone());
        let fwd = tauri::async_runtime::spawn(async move {
            while let Some(ev) = erx.recv().await {
                let _ = a3.emit("bot-event", json!({ "runId": r3, "chatId": t3, "bot": b3, "ev": ev }));
            }
        });
        let r = runner::thread_turn(&c, &bid, Some(&tid), &text, &etx, &host, &cancel).await;
        drop(etx);
        let _ = fwd.await;
        let error = match r {
            Ok((_, out)) => out.error,
            Err(e) => Some(e),
        };
        finish(&app2, &rid, &tid, error, cancel.is_cancelled());
    });
    Ok(json!({ "runId": run_id, "chatId": thread }))
}

#[tauri::command]
pub fn bots_cancel(run_id: String) {
    if let Some(c) = runs(|r| r.get(&run_id).cloned()) {
        c.cancel();
    }
}

#[tauri::command]
pub fn bots_answer(id: String, allow: Option<bool>, answers: Option<Vec<String>>) -> CmdResult<()> {
    let (_, _, reply) = pending(|p| p.remove(&id)).ok_or("Already answered")?;
    match reply {
        Reply::Approve(tx) => {
            let _ = tx.send(allow.unwrap_or(false));
        }
        Reply::Ask(tx) => {
            let _ = tx.send(answers.unwrap_or_default());
        }
    }
    Ok(())
}

#[tauri::command]
pub fn rooms_save(app: AppHandle, room: Value) -> CmdResult<Value> {
    let c = ctx(&app);
    let mut r: Room = match room.get("id").and_then(Value::as_str).filter(|s| !s.is_empty()).and_then(|id| c.rooms.get(id)) {
        Some(existing) => existing,
        None => Room::new("Room", vec![]),
    };
    let s = |k: &str| room.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    r.name = s("name");
    r.instructions = s("instructions");
    r.folder = s("folder");
    r.members = room.get("members").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
    r.lead = room.get("lead").and_then(Value::as_str).filter(|x| !x.is_empty()).map(str::to_string).or_else(|| r.members.first().cloned());
    r.updated = bots::now_secs();
    c.rooms.save(&r).map_err(|e| e.to_string())?;
    Ok(json!({ "id": r.id }))
}

#[tauri::command]
pub fn rooms_delete(app: AppHandle, id: String) -> CmdResult<()> {
    ctx(&app).rooms.delete(&id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn rooms_get(app: AppHandle, id: String) -> CmdResult<Value> {
    let c = ctx(&app);
    let r = c.rooms.get(&id).ok_or("No such room")?;
    let names: HashMap<String, String> = c.bots.list().into_iter().map(|b| (b.id, b.name)).collect();
    let chat_id = format!("room:{}", r.id);
    let msgs: Vec<Value> = r.messages.iter().map(|m| if m.author == "user" { json!({ "role": "user", "text": m.text }) } else { json!({ "role": "assistant", "text": m.text, "tools": m.tools, "author": names.get(&m.author).cloned().unwrap_or_else(|| "bot".into()) }) }).collect();
    let pend: Vec<Value> = pending(|p| p.values().filter(|(_, v, _)| v["chatId"] == chat_id.as_str()).map(|(_, v, _)| v.clone()).collect());
    Ok(json!({ "id": r.id, "name": r.name, "messages": msgs, "pending": pend }))
}

#[tauri::command]
pub fn rooms_send(app: AppHandle, room: String, text: String) -> CmdResult<Value> {
    let text = text.trim().to_string();
    if text.is_empty() || text.len() > 100_000 {
        return Err("Write a message first".into());
    }
    let c = ctx(&app);
    let r = c.rooms.get(&room).ok_or("No such room")?;
    let chat_id = format!("room:{}", r.id);
    let run_id = format!("r{}", &uuid_simple()[..12]);
    let cancel = CancellationToken::new();
    runs(|x| x.insert(run_id.clone(), cancel.clone()));
    let host = host_channel(&app, &run_id, &chat_id);
    let (app2, rid, cid, room_id) = (app.clone(), run_id.clone(), chat_id.clone(), r.id.clone());
    tauri::async_runtime::spawn(async move {
        let (etx, mut erx) = mpsc::unbounded_channel::<(String, AgentEvent)>();
        let (a3, r3, c3) = (app2.clone(), rid.clone(), cid.clone());
        let fwd = tauri::async_runtime::spawn(async move {
            while let Some((bot, ev)) = erx.recv().await {
                let _ = a3.emit("bot-event", json!({ "runId": r3, "chatId": c3, "bot": bot, "ev": ev }));
            }
        });
        let res = runner::room_turn(&c, &room_id, &text, &etx, &host, &cancel).await;
        drop(etx);
        let _ = fwd.await;
        finish(&app2, &rid, &cid, res.err(), cancel.is_cancelled());
    });
    Ok(json!({ "runId": run_id, "chatId": chat_id }))
}

#[tauri::command]
pub async fn bots_computer(app: AppHandle, bot: String) -> CmdResult<String> {
    let c = ctx(&app);
    let b = c.bots.get(&bot).ok_or("No such bot")?;
    if !b.computer {
        return Err("Turn on this bot's computer in its settings first".into());
    }
    bots::computer::ensure(&b.id, &c.bots.workdir(&b)).await
}

/// Start the routine/check-back scheduler for as long as the app runs.
pub fn start_scheduler(app: &AppHandle) {
    let c = Arc::new(ctx(app));
    let host = host_channel(app, "scheduler", "scheduler");
    let a = app.clone();
    let on: Arc<dyn Fn(SchedEvent) + Send + Sync> = Arc::new(move |e| {
        let v = match e {
            SchedEvent::Started { bot, thread, title } => json!({ "kind": "started", "bot": bot, "thread": thread, "title": title }),
            SchedEvent::Finished { bot, thread, title, error } => json!({ "kind": "finished", "bot": bot, "thread": thread, "title": title, "error": error }),
        };
        let _ = a.emit("bot-sched", v);
    });
    tauri::async_runtime::spawn(runner::scheduler(c, host, CancellationToken::new(), on));
}
