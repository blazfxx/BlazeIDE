use super::*;
use crate::agent::Approval;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn fake_llm(bodies: Vec<String>) -> (String, Arc<Mutex<Vec<Value>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    tokio::spawn(async move {
        let mut bodies = bodies.into_iter();
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { break };
            let mut buf = Vec::new();
            let mut tmp = [0u8; 65536];
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
                        seen2.lock().unwrap().push(serde_json::from_slice(&buf[h + 4..h + 4 + len]).unwrap_or(Value::Null));
                        break;
                    }
                }
            }
            let body = bodies.next().unwrap_or_else(|| text("done"));
            let resp = format!("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}", body.len(), body);
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.shutdown().await;
        }
    });
    (format!("http://{addr}"), seen)
}

fn text(t: &str) -> String {
    format!("data: {}\n\ndata: [DONE]\n\n", json!({"choices":[{"delta":{"content":t}}]}))
}

fn tool(id: &str, name: &str, args: Value) -> String {
    format!("data: {}\n\ndata: [DONE]\n\n", json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}}]}}]}))
}

fn ctx(dir: &Path, base: String) -> BotCtx {
    BotCtx {
        config_dir: dir.to_path_buf(),
        bots: Arc::new(Bots::open(dir)),
        rooms: Arc::new(Rooms::open(dir)),
        backend: Arc::new(move || Backend {
            http: reqwest::Client::new(),
            http_custom: reqwest::Client::new(),
            v1_base: base.clone(),
            key: Some("sk-test".into()),
            routing: String::new(),
            providers: vec![],
            provider_key: Arc::new(|_| None),
            models: vec![],
        }),
        agent_config: Arc::new(|root, model, mode| AgentConfig::new(root.to_path_buf(), model, mode)),
        default_model: Arc::new(|| "glm-5.3".into()),
        computer: None,
    }
}

fn chans() -> (mpsc::UnboundedSender<AgentEvent>, mpsc::Sender<HostRequest>) {
    let (etx, mut erx) = mpsc::unbounded_channel();
    tokio::spawn(async move { while erx.recv().await.is_some() {} });
    let (htx, mut hrx) = mpsc::channel(4);
    tokio::spawn(async move {
        while let Some(r) = hrx.recv().await {
            if let HostRequest::Approve { reply, .. } = r {
                let _ = reply.send(true);
            }
        }
    });
    (etx, htx)
}

#[tokio::test]
async fn bot_thread_remembers_and_checks_back() {
    let d = tempfile::tempdir().unwrap();
    let (base, seen) = fake_llm(vec![
        tool("c1", "remember", json!({"note": "User's cat is called Miso"})),
        tool("c2", "check_back", json!({"in_minutes": 30, "note": "ask how Miso is"})),
        text("Noted, and I'll check back."),
    ])
    .await;
    let c = ctx(d.path(), base);
    let mut bot = Bot::new("Nova");
    bot.soul = "You are cheerful.".into();
    bot.approval = Approval::Full;
    let bot = c.bots.save(bot).unwrap();
    let (etx, htx) = chans();
    let (tid, out) = thread_turn(&c, &bot.id, None, "My cat is Miso", &etx, &htx, &CancellationToken::new()).await.unwrap();
    assert!(out.error.is_none(), "{:?}", out.error);
    assert!(c.bots.memory(&bot.id).contains("Miso"));
    let b = c.bots.get(&bot.id).unwrap();
    assert_eq!(b.check_backs.len(), 1);
    assert_eq!(b.check_backs[0].thread, tid);
    // The persona and the bot tools were sent; the thread is in the bot's folder.
    {
        let first = seen.lock().unwrap()[0].clone();
        let sys = first["messages"][0]["content"].as_str().unwrap();
        assert!(sys.contains("You are Nova") && sys.contains("You are cheerful."));
        let names: Vec<&str> = first["tools"].as_array().unwrap().iter().map(|t| t["function"]["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"remember") && names.contains(&"check_back") && !names.contains(&"hand_off"));
    }
    let thread = load_thread(&c, &b, &tid).unwrap();
    assert_eq!(thread.extra.get("bot").and_then(Value::as_str), Some(bot.id.as_str()));
    assert_eq!(thread.messages.last().unwrap().content, "Noted, and I'll check back.");
    // The second turn sees the memory.
    let (base2, seen2) = fake_llm(vec![text("Miso!")]).await;
    let c2 = ctx(d.path(), base2);
    thread_turn(&c2, &bot.id, Some(&tid), "what's my cat called?", &etx, &htx, &CancellationToken::new()).await.unwrap();
    assert!(seen2.lock().unwrap()[0]["messages"][0]["content"].as_str().unwrap().contains("User's cat is called Miso"));
}

#[tokio::test]
async fn room_lead_hands_off() {
    let d = tempfile::tempdir().unwrap();
    // lead round 1 -> hand_off; Orbit answers; lead round 2 answers the user.
    let (base, seen) = fake_llm(vec![
        tool("h1", "hand_off", json!({"bot": "orbit", "task": "List three launch risks"})),
        text("1. Payments 2. DNS 3. Load"),
        text("Orbit found three risks: payments, DNS and load."),
    ])
    .await;
    let c = ctx(d.path(), base);
    let lead = c.bots.save(Bot::new("Nova")).unwrap();
    let orbit = c.bots.save(Bot::new("Orbit")).unwrap();
    let mut room = Room::new("Launch", vec![lead.id.clone(), orbit.id.clone()]);
    room.instructions = "Ship on Friday.".into();
    c.rooms.save(&room).unwrap();
    let (etx, mut erx) = mpsc::unbounded_channel();
    let speakers = tokio::spawn(async move {
        let mut who = Vec::new();
        while let Some((b, _)) = erx.recv().await {
            if who.last() != Some(&b) {
                who.push(b);
            }
        }
        who
    });
    let (_e, htx) = chans();
    let r = room_turn(&c, &room.id, "What could go wrong?", &etx, &htx, &CancellationToken::new()).await.unwrap();
    drop(etx);
    let authors: Vec<&str> = r.messages.iter().map(|m| m.author.as_str()).collect();
    assert_eq!(authors, vec!["user", orbit.id.as_str(), lead.id.as_str()]);
    assert!(r.messages[2].text.contains("three risks"));
    assert_eq!(r.messages[2].tools, vec![format!("hand_off {}", json!({"bot": "orbit", "task": "List three launch risks"}))]);
    let reqs = seen.lock().unwrap().clone();
    // Orbit got the task attributed to the lead, and the room instructions.
    let orbit_req = &reqs[1];
    let msgs = orbit_req["messages"].as_array().unwrap();
    assert!(msgs.last().unwrap()["content"].as_str().unwrap().starts_with("[Nova] List three launch risks"));
    assert!(msgs[0]["content"].as_str().unwrap().contains("Ship on Friday."));
    assert!(!orbit_req["tools"].as_array().unwrap().iter().any(|t| t["function"]["name"] == "hand_off"), "no nested hand-offs");
    // The lead got Orbit's answer back as the tool result.
    assert!(reqs[2]["messages"].as_array().unwrap().iter().any(|m| m["role"] == "tool" && m["content"].as_str().unwrap().contains("Orbit answered")));
    assert_eq!(speakers.await.unwrap(), vec![lead.id.clone(), orbit.id.clone(), lead.id.clone()]);
    // Saved.
    assert_eq!(c.rooms.get(&room.id).unwrap().messages.len(), 3);
}

#[cfg(unix)]
#[tokio::test]
async fn claude_engine_with_a_fake_cli() {
    use std::os::unix::fs::PermissionsExt;
    let d = tempfile::tempdir().unwrap();
    let script = d.path().join("fake-claude");
    std::fs::write(
        &script,
        r#"#!/bin/sh
echo "$@" > "$(dirname "$0")/args.txt"
echo '{"type":"system","subtype":"init","session_id":"sess-1"}'
echo '{"type":"assistant","message":{"content":[{"type":"text","text":"Hello from Claude."}]}}'
echo '{"type":"result","subtype":"success","result":"Hello from Claude.","session_id":"sess-1","usage":{"input_tokens":3,"output_tokens":4}}'
"#,
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let c = ctx(d.path(), "http://127.0.0.1:9".into());
    let mut bot = Bot::new("Claudette");
    bot.engine = Engine::Claude { bin: Some(script.display().to_string()) };
    bot.approval = Approval::Edits;
    let bot = c.bots.save(bot).unwrap();
    let (etx, htx) = chans();
    let (tid, out) = thread_turn(&c, &bot.id, None, "hi", &etx, &htx, &CancellationToken::new()).await.unwrap();
    assert!(out.error.is_none(), "{:?}", out.error);
    assert_eq!(out.output_tokens, 4);
    let args = std::fs::read_to_string(d.path().join("args.txt")).unwrap();
    assert!(args.contains("-p hi") && args.contains("--permission-mode acceptEdits") && args.contains("--append-system-prompt You are Claudette"), "{args}");
    let t = load_thread(&c, &c.bots.get(&bot.id).unwrap(), &tid).unwrap();
    assert_eq!(t.messages.last().unwrap().content, "Hello from Claude.");
    // Second turn resumes the same CLI session.
    thread_turn(&c, &bot.id, Some(&tid), "again", &etx, &htx, &CancellationToken::new()).await.unwrap();
    let args = std::fs::read_to_string(d.path().join("args.txt")).unwrap();
    assert!(args.contains("--resume sess-1") && args.contains("-p again"), "{args}");
}

#[tokio::test]
async fn scheduler_runs_a_due_routine_once() {
    let d = tempfile::tempdir().unwrap();
    let (base, seen) = fake_llm(vec![text("Morning report: all good.")]).await;
    let c = Arc::new(ctx(d.path(), base));
    let mut bot = Bot::new("Reporter");
    bot.created = now_secs() - 86_400;
    bot.routines.push(super::super::Routine {
        id: "r1".into(),
        title: "Report".into(),
        prompt: "Write the morning report".into(),
        schedule: schedule::Schedule::Every { hours: 1 },
        enabled: true,
        catch_up: true,
        last_run: now_secs() - 7200,
        thread: None,
    });
    let bot = c.bots.save(bot).unwrap();
    let (_etx, htx) = chans();
    let cancel = CancellationToken::new();
    let done = Arc::new(Mutex::new(Vec::new()));
    let d2 = done.clone();
    let on: Arc<dyn Fn(SchedEvent) + Send + Sync> = Arc::new(move |e| d2.lock().unwrap().push(format!("{e:?}")));
    let h = tokio::spawn(scheduler(c.clone(), htx, cancel.clone(), on));
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if done.lock().unwrap().iter().any(|e| e.starts_with("Finished")) {
            break;
        }
    }
    cancel.cancel();
    let _ = h.await;
    assert!(done.lock().unwrap().iter().any(|e| e.starts_with("Finished") && e.contains("error: None")), "{:?}", done.lock().unwrap());
    let b = c.bots.get(&bot.id).unwrap();
    assert!(b.routines[0].last_run > now_secs() - 60);
    assert!(b.routines[0].thread.is_some(), "the routine got its own thread");
    assert!(seen.lock().unwrap()[0]["messages"].as_array().unwrap().last().unwrap()["content"].as_str().unwrap().contains("[Routine \"Report\""));
}
