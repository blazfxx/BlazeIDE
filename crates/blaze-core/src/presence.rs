//! "Working on <what> - BlazeCLI - blazeapi.org" as the user's Discord status
//! (Rich Presence), for Blaze IDE and the blaze CLI.
//!
//! Talks to the Discord desktop app over its local IPC socket
//! (`discord-ipc-N`: a Unix socket on macOS/Linux, a named pipe on Windows);
//! nothing goes over the network. If Discord is not running, updates are
//! dropped and the connection is retried quietly. Updates are throttled
//! (Discord allows about 5 per 20 s).

use std::io::{Read, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Discord application id ("BlazeCLI" in the Discord Developer Portal).
/// Empty = presence off. `BLAZE_DISCORD_APP_ID` overrides it.
pub const DISCORD_APP_ID: &str = "1574957922285523186";

pub fn app_id() -> String {
    std::env::var("BLAZE_DISCORD_APP_ID").ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| DISCORD_APP_ID.to_string())
}

/// The visible text: `details` = "Working on <what>", `state` = "BlazeCLI - blazeapi.org".
pub fn line(what: &str) -> (String, String) {
    let w: String = what.trim().chars().take(100).collect();
    let details = if w.is_empty() { "Working".to_string() } else { format!("Working on {w}") };
    (details, "BlazeCLI - blazeapi.org".to_string())
}

/// What the user is working on, from an agent tool summary ("Edit src/app.py"
/// -> "app.py"), else the project name.
pub fn what_from_summary(summary: &str, project: &str) -> String {
    for verb in ["Edit ", "Write ", "Read "] {
        if let Some(p) = summary.strip_prefix(verb) {
            let name = p.trim().rsplit(['/', '\\']).next().unwrap_or(p).trim();
            if !name.is_empty() {
                return name.to_string();
            }
        }
    }
    project.to_string()
}

enum Msg {
    Set(String),
    Clear,
}

/// Handle to the background presence thread.
pub struct Presence {
    tx: mpsc::Sender<Msg>,
}

impl Presence {
    /// None when no application id is configured.
    pub fn start() -> Option<Presence> {
        let id = app_id();
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let (tx, rx) = mpsc::channel::<Msg>();
        std::thread::Builder::new().name("discord-presence".into()).spawn(move || run(&id, rx)).ok()?;
        Some(Presence { tx })
    }

    /// Show "Working on <what>".
    pub fn set(&self, what: &str) {
        let _ = self.tx.send(Msg::Set(what.to_string()));
    }

    pub fn clear(&self) {
        let _ = self.tx.send(Msg::Clear);
    }
}

#[cfg(unix)]
type Conn = std::os::unix::net::UnixStream;
#[cfg(windows)]
type Conn = std::fs::File;

#[cfg(unix)]
fn connect() -> Option<Conn> {
    let mut dirs: Vec<String> = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"].iter().filter_map(|k| std::env::var(k).ok()).collect();
    dirs.push("/tmp".into());
    let subs = ["", "app/com.discordapp.Discord/", "snap.discord/", ".flatpak/com.discordapp.Discord/xdg-run/"];
    for d in &dirs {
        for sub in subs {
            for i in 0..10 {
                let p = format!("{}/{sub}discord-ipc-{i}", d.trim_end_matches('/'));
                if let Ok(s) = std::os::unix::net::UnixStream::connect(&p) {
                    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
                    let _ = s.set_nonblocking(false);
                    return Some(s);
                }
            }
        }
    }
    None
}

#[cfg(windows)]
fn connect() -> Option<Conn> {
    (0..10).find_map(|i| std::fs::OpenOptions::new().read(true).write(true).open(format!(r"\\.\pipe\discord-ipc-{i}")).ok())
}

fn frame(op: u32, body: &serde_json::Value) -> Vec<u8> {
    let data = serde_json::to_vec(body).unwrap_or_default();
    let mut f = Vec::with_capacity(8 + data.len());
    f.extend_from_slice(&op.to_le_bytes());
    f.extend_from_slice(&(data.len() as u32).to_le_bytes());
    f.extend_from_slice(&data);
    f
}

/// Read one frame (the handshake answer). Unix only: a Windows pipe has no
/// read timeout, and nothing later needs the answers.
#[cfg(unix)]
fn read_frame(c: &mut Conn) -> bool {
    let _ = c.set_read_timeout(Some(Duration::from_secs(3)));
    let mut head = [0u8; 8];
    if c.read_exact(&mut head).is_err() {
        return false;
    }
    let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
    let mut body = vec![0u8; len.min(1 << 20)];
    c.read_exact(&mut body).is_ok() && u32::from_le_bytes([head[0], head[1], head[2], head[3]]) == 1
}

#[cfg(windows)]
fn read_frame(_c: &mut Conn) -> bool {
    true
}

fn handshake(id: &str) -> Option<Conn> {
    let mut c = connect()?;
    c.write_all(&frame(0, &serde_json::json!({ "v": 1, "client_id": id }))).ok()?;
    read_frame(&mut c).then_some(c)
}

pub fn activity_payload(what: Option<&str>, start: i64) -> serde_json::Value {
    let activity = what.map(|w| {
        let (details, state) = line(w);
        serde_json::json!({
            "details": details,
            "state": state,
            "timestamps": { "start": start },
            "assets": { "large_image": "blaze", "large_text": "blazeapi.org" },
            "buttons": [{ "label": "blazeapi.org", "url": "https://blazeapi.org" }],
        })
    });
    serde_json::json!({
        "cmd": "SET_ACTIVITY",
        "args": { "pid": std::process::id(), "activity": activity },
        "nonce": uuid::Uuid::new_v4().to_string(),
    })
}

fn run(id: &str, rx: mpsc::Receiver<Msg>) {
    let start = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let mut conn: Option<Conn> = None;
    let mut want: Option<Option<String>> = None; // Some(None) = clear
    let mut sent: Option<Option<String>> = None;
    let mut last_send = Instant::now() - Duration::from_secs(60);
    let mut last_try = Instant::now() - Duration::from_secs(60);
    loop {
        match rx.recv_timeout(Duration::from_millis(800)) {
            Ok(Msg::Set(w)) => want = Some(Some(w)),
            Ok(Msg::Clear) => want = Some(None),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if let Some(c) = conn.as_mut() {
                    let _ = c.write_all(&frame(1, &activity_payload(None, start)));
                }
                return;
            }
        }
        if want.is_none() || want == sent || last_send.elapsed() < Duration::from_secs(4) {
            continue;
        }
        if conn.is_none() {
            if last_try.elapsed() < Duration::from_secs(15) {
                continue;
            }
            last_try = Instant::now();
            conn = handshake(id);
        }
        let Some(c) = conn.as_mut() else { continue };
        let w = want.clone().unwrap_or(None);
        if c.write_all(&frame(1, &activity_payload(w.as_deref(), start))).is_ok() {
            sent = want.clone();
            last_send = Instant::now();
        } else {
            conn = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_payload() {
        assert_eq!(line("musicvid"), ("Working on musicvid".to_string(), "BlazeCLI - blazeapi.org".to_string()));
        assert_eq!(what_from_summary("Edit src/app.py", "proj"), "app.py");
        assert_eq!(what_from_summary("Run: npm test", "proj"), "proj");
        let p = activity_payload(Some("calc.py"), 5);
        assert_eq!(p["cmd"], "SET_ACTIVITY");
        assert_eq!(p["args"]["activity"]["details"], "Working on calc.py");
        assert!(activity_payload(None, 5)["args"]["activity"].is_null());
        let f = frame(1, &serde_json::json!({"a":1}));
        assert_eq!(&f[..4], &1u32.to_le_bytes());
        assert_eq!(u32::from_le_bytes([f[4], f[5], f[6], f[7]]) as usize, f.len() - 8);
    }

    /// A fake Discord on a Unix socket: handshake answered, activity received.
    #[cfg(unix)]
    #[test]
    fn talks_to_a_fake_discord() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("discord-ipc-0");
        let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
        let g2 = got.clone();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            loop {
                let mut head = [0u8; 8];
                if s.read_exact(&mut head).is_err() {
                    break;
                }
                let op = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
                let len = u32::from_le_bytes([head[4], head[5], head[6], head[7]]) as usize;
                let mut body = vec![0u8; len];
                s.read_exact(&mut body).unwrap();
                g2.lock().unwrap().push(serde_json::from_slice(&body).unwrap());
                if op == 0 {
                    s.write_all(&frame(1, &serde_json::json!({"cmd":"DISPATCH","evt":"READY"}))).unwrap();
                }
            }
        });
        std::env::set_var("XDG_RUNTIME_DIR", dir.path());
        std::env::set_var("BLAZE_DISCORD_APP_ID", "123456789012345678");
        let p = Presence::start().unwrap();
        p.set("musicvid");
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(100));
            if got.lock().unwrap().len() >= 2 {
                break;
            }
        }
        let g = got.lock().unwrap();
        assert_eq!(g[0]["client_id"], "123456789012345678");
        assert_eq!(g[1]["args"]["activity"]["details"], "Working on musicvid");
        assert_eq!(g[1]["args"]["activity"]["state"], "BlazeCLI - blazeapi.org");
    }
}
