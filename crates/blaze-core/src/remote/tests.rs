use super::*;
use crate::agent::Approval;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Fake OpenAI endpoint: each POST gets the next canned SSE body.
async fn fake_llm(bodies: Vec<String>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
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
    format!("http://{addr}")
}

fn sse_text(t: &str) -> String {
    format!("data: {}\n\ndata: [DONE]\n\n", json!({"choices":[{"delta":{"content":t}}]}))
}

fn sse_tool(id: &str, name: &str, args: Value) -> String {
    format!(
        "data: {}\n\ndata: [DONE]\n\n",
        json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}}]}}]})
    )
}

struct FakeHost {
    store: Store,
    llm: String,
    project: PathBuf,
    cfg_dir: PathBuf,
}

impl Host for FakeHost {
    fn store(&self) -> &Store {
        &self.store
    }
    fn backend(&self) -> Backend {
        Backend {
            http: reqwest::Client::new(),
            http_custom: reqwest::Client::new(),
            v1_base: self.llm.clone(),
            key: Some("sk-test".into()),
            routing: String::new(),
            providers: vec![],
            provider_key: Arc::new(|_| None),
            models: vec![],
        }
    }
    fn agent_config(&self, root: &Path, model: &str, mode: &str) -> AgentConfig {
        let mut c = AgentConfig::new(root.to_path_buf(), model, mode);
        c.approval = Approval::Ask;
        c
    }
    fn projects(&self) -> Vec<PathBuf> {
        vec![self.project.clone()]
    }
    fn models(&self) -> Vec<ModelEntry> {
        vec![]
    }
    fn default_model(&self) -> String {
        "glm-5.3".into()
    }
    fn label(&self) -> String {
        "Test machine".into()
    }
    fn bots(&self) -> Option<crate::bots::runner::BotCtx> {
        let llm = self.llm.clone();
        Some(crate::bots::runner::BotCtx {
            config_dir: self.cfg_dir.clone(),
            bots: Arc::new(crate::bots::Bots::open(&self.cfg_dir)),
            rooms: Arc::new(crate::bots::room::Rooms::open(&self.cfg_dir)),
            backend: Arc::new(move || Backend { http: reqwest::Client::new(), http_custom: reqwest::Client::new(), v1_base: llm.clone(), key: Some("sk".into()), routing: String::new(), providers: vec![], provider_key: Arc::new(|_| None), models: vec![] }),
            agent_config: Arc::new(|r, m, mo| AgentConfig::new(r.to_path_buf(), m, mo)),
            default_model: Arc::new(|| "glm-5.3".into()),
            computer: None,
        })
    }
}

struct Client {
    http: reqwest::Client,
    base: String,
    cookie: Option<String>,
}

impl Client {
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value, Option<String>) {
        let url = format!("{}{}", self.base, path);
        let mut r = if method == "GET" { self.http.get(&url) } else { self.http.post(&url).header("x-blaze", "1") };
        if let Some(c) = &self.cookie {
            r = r.header("cookie", c);
        }
        if let Some(b) = body {
            r = r.json(&b);
        }
        let resp = r.send().await.unwrap();
        let status = resp.status().as_u16();
        let cookie = resp.headers().get("set-cookie").map(|v| v.to_str().unwrap().to_string());
        let v = resp.json::<Value>().await.unwrap_or(Value::Null);
        (status, v, cookie)
    }
}

async fn setup(llm: Vec<String>) -> (tempfile::TempDir, tempfile::TempDir, Arc<Server>, Running, Client) {
    let cfg_dir = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    let project = dunce::canonicalize(proj.path()).unwrap();
    std::fs::write(project.join("a.txt"), "one\n").unwrap();
    let host = FakeHost { store: Store::open(cfg_dir.path()), llm: fake_llm(llm).await, project, cfg_dir: cfg_dir.path().to_path_buf() };
    auth::set_password(&host.store, "a long enough password").unwrap();
    auth::update(&host.store, |c| {
        c.enabled = true;
        Ok(())
    })
    .unwrap();
    let server = Server::new(Arc::new(host));
    let running = serve(server.clone(), 0, false).await.unwrap();
    let client = Client { http: reqwest::Client::new(), base: format!("http://{}", running.addr), cookie: None };
    (cfg_dir, proj, server, running, client)
}

#[tokio::test]
async fn sign_in_rules() {
    let (_c, _p, server, _r, mut cl) = setup(vec![]).await;
    // Static app and health are public; the API is not.
    let page = reqwest::get(format!("{}/", cl.base)).await.unwrap();
    assert_eq!(page.status(), 200);
    assert!(page.headers()["content-security-policy"].to_str().unwrap().contains("script-src 'self'"));
    assert_eq!(cl.call("GET", "/healthz", None).await.1["id"], server.server_id.as_str());
    assert_eq!(cl.call("GET", "/api/projects", None).await.0, 401);
    // Changing requests need the app's header.
    let r = cl.http.post(format!("{}/api/login", cl.base)).json(&json!({"password":"x"})).send().await.unwrap();
    assert_eq!(r.status(), 403);
    // Cross-site requests are refused.
    let r = cl.http.post(format!("{}/api/login", cl.base)).header("x-blaze", "1").header("origin", "https://evil.example").json(&json!({"password":"a long enough password"})).send().await.unwrap();
    assert_eq!(r.status(), 403);
    // Wrong password, then right one.
    assert_eq!(cl.call("POST", "/api/login", Some(json!({"password":"nope"}))).await.0, 401);
    let (st, _, cookie) = cl.call("POST", "/api/login", Some(json!({"password":"a long enough password"}))).await;
    assert_eq!(st, 200);
    let cookie = cookie.unwrap();
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict") && cookie.contains("Path=/;"));
    assert!(!cookie.contains("Secure"), "plain http on the LAN cannot use Secure cookies");
    cl.cookie = Some(cookie.split(';').next().unwrap().to_string());
    let (st, v, _) = cl.call("GET", "/api/projects", None).await;
    assert_eq!(st, 200);
    assert_eq!(v["projects"].as_array().unwrap().len(), 1);
    // Projects that were never opened on this machine are off limits.
    assert_eq!(cl.call("GET", "/api/chats?root=%2Fetc", None).await.0, 404);
    // Lockout after 5 wrong passwords (one used above).
    let anon = Client { http: reqwest::Client::new(), base: cl.base.clone(), cookie: None };
    for _ in 0..4 {
        anon.call("POST", "/api/login", Some(json!({"password":"wrong"}))).await;
    }
    assert_eq!(anon.call("POST", "/api/login", Some(json!({"password":"a long enough password"}))).await.0, 429);
    // Logout revokes the session.
    assert_eq!(cl.call("POST", "/api/logout", Some(json!({}))).await.0, 200);
    assert_eq!(cl.call("GET", "/api/projects", None).await.0, 401);
}

#[tokio::test]
async fn pairing_code_works_once() {
    let (_c, _p, server, _r, cl) = setup(vec![]).await;
    let code = server.pair_code();
    let (st, _, cookie) = cl.call("POST", "/api/pair", Some(json!({"code": code}))).await;
    assert_eq!(st, 200);
    assert!(cookie.is_some());
    assert_eq!(cl.call("POST", "/api/pair", Some(json!({"code": code}))).await.0, 401);
}

#[tokio::test]
async fn turn_with_approval_from_the_phone() {
    let diff = "<<<<<<< SEARCH\none\n=======\ntwo\n>>>>>>> REPLACE";
    let (_c, proj, server, _r, mut cl) = setup(vec![sse_tool("c1", "apply_diff", json!({"path":"a.txt","diff":diff})), sse_text("Changed it.")]).await;
    let (_, _, cookie) = cl.call("POST", "/api/login", Some(json!({"password":"a long enough password"}))).await;
    cl.cookie = Some(cookie.unwrap().split(';').next().unwrap().to_string());
    let root = dunce::canonicalize(proj.path()).unwrap().display().to_string();
    let (st, v, _) = cl.call("POST", "/api/send", Some(json!({"root": root, "text": "change one to two"}))).await;
    assert_eq!(st, 200, "{v}");
    let chat_id = v["chatId"].as_str().unwrap().to_string();
    // Wait for the approval card, answer it from the "phone".
    let mut seq = 0u64;
    let mut card = None;
    let mut ended = None;
    for _ in 0..40 {
        let (_, v, _) = cl.call("GET", &format!("/api/events?after={seq}"), None).await;
        seq = v["seq"].as_u64().unwrap();
        for e in v["events"].as_array().unwrap() {
            if e["type"] == "pending" {
                card = Some(e["item"].clone());
            }
            if e["type"] == "run_end" {
                ended = Some(e.clone());
            }
        }
        if let Some(c) = card.take() {
            assert_eq!(c["kind"], "approve");
            assert_eq!(c["summary"], "Edit a.txt");
            assert_eq!(cl.call("POST", "/api/answer", Some(json!({"id": c["id"], "allow": true}))).await.0, 200);
            // A second answer is refused.
            assert_eq!(cl.call("POST", "/api/answer", Some(json!({"id": c["id"], "allow": false}))).await.0, 410);
        }
        if ended.is_some() {
            break;
        }
    }
    let ended = ended.expect("run ended");
    assert!(ended["error"].is_null(), "{ended}");
    assert_eq!(std::fs::read_to_string(proj.path().join("a.txt")).unwrap(), "two\n");
    // The chat is saved in the IDE's format and readable by the phone.
    let (_, v, _) = cl.call("GET", &format!("/api/chat?root={}&id={chat_id}", urlencode(&root)), None).await;
    let msgs = v["messages"].as_array().unwrap();
    assert_eq!(msgs[0]["text"], "change one to two");
    assert_eq!(msgs.last().unwrap()["text"], "Changed it.");
    assert!(msgs.iter().any(|m| m["tools"].as_array().is_some_and(|t| t.iter().any(|x| x == "Edit a.txt"))));
    assert_eq!(server.active_runs(), 0);
}

fn urlencode(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

#[tokio::test]
async fn relay_requests_get_their_own_cookie_path_and_only_reach_the_api() {
    let (_c, _p, server, _r, _cl) = setup(vec![]).await;
    let b64 = base64::engine::general_purpose::STANDARD;
    use base64::Engine;
    let login = relay::Incoming {
        id: "1".into(),
        method: "POST".into(),
        path: "/api/login".into(),
        headers: vec![("content-type".into(), "application/json".into()), ("x-blaze".into(), "1".into())],
        body: b64.encode(br#"{"password":"a long enough password"}"#),
        ip: "203.0.113.9".into(),
    };
    let rep = relay::answer(&server, "abc123", login).await;
    assert_eq!(rep.status, 200);
    let cookie = rep.headers.iter().find(|(k, _)| k == "set-cookie").unwrap().1.clone();
    assert!(cookie.contains("Path=/r/abc123/") && cookie.contains("Secure"), "{cookie}");
    let page = relay::Incoming { id: "2".into(), method: "GET".into(), path: "/".into(), headers: vec![], body: String::new(), ip: String::new() };
    assert_eq!(relay::answer(&server, "abc123", page).await.status, 400, "the relay never serves desktop pages");
}

async fn wait_end(cl: &Client, chat: &str) -> Value {
    let mut seq = 0u64;
    for _ in 0..40 {
        let (_, v, _) = cl.call("GET", &format!("/api/events?after={seq}"), None).await;
        seq = v["seq"].as_u64().unwrap();
        for e in v["events"].as_array().unwrap() {
            if e["type"] == "run_end" && e["chatId"] == chat {
                return e.clone();
            }
        }
    }
    panic!("run did not end");
}

#[tokio::test]
async fn bots_and_rooms_from_the_phone() {
    let (cfg_dir, _p, _server, _r, mut cl) = setup(vec![sse_text("Hi, I'm Nova."), sse_text("Room answer from Nova.")]).await;
    let bots = crate::bots::Bots::open(cfg_dir.path());
    let nova = bots.save(crate::bots::Bot::new("Nova")).unwrap();
    let rooms = crate::bots::room::Rooms::open(cfg_dir.path());
    let room = crate::bots::room::Room::new("Team", vec![nova.id.clone()]);
    rooms.save(&room).unwrap();
    let (_, _, cookie) = cl.call("POST", "/api/login", Some(json!({"password":"a long enough password"}))).await;
    cl.cookie = Some(cookie.unwrap().split(';').next().unwrap().to_string());
    let (_, v, _) = cl.call("GET", "/api/bots", None).await;
    assert_eq!(v["bots"][0]["name"], "Nova");
    assert_eq!(v["rooms"][0]["name"], "Team");
    let (st, v, _) = cl.call("POST", "/api/bot/send", Some(json!({"bot": nova.id, "text": "hello"}))).await;
    assert_eq!(st, 200, "{v}");
    let thread = v["chatId"].as_str().unwrap().to_string();
    let end = wait_end(&cl, &thread).await;
    assert!(end["error"].is_null(), "{end}");
    let (_, v, _) = cl.call("GET", &format!("/api/bot/thread?bot={}&id={thread}", nova.id), None).await;
    assert_eq!(v["messages"][1]["text"], "Hi, I'm Nova.");
    let (_, v, _) = cl.call("GET", &format!("/api/bot/threads?bot={}", nova.id), None).await;
    assert_eq!(v["threads"].as_array().unwrap().len(), 1);
    let (st, v, _) = cl.call("POST", "/api/room/send", Some(json!({"room": room.id, "text": "status?"}))).await;
    assert_eq!(st, 200, "{v}");
    let end = wait_end(&cl, &format!("room:{}", room.id)).await;
    assert!(end["error"].is_null(), "{end}");
    let (_, v, _) = cl.call("GET", &format!("/api/room?id={}", room.id), None).await;
    assert_eq!(v["messages"][1]["author"], "Nova");
    assert_eq!(v["messages"][1]["text"], "Room answer from Nova.");
}
