//! A bot's private Linux desktop: a Docker container (XFCE, a browser) the
//! bot can see and use through screenshots, clicks and typing, plus a shell
//! inside it. Needs Docker on the user's computer.
//!
//! - The image is built locally from the Dockerfile below the first time
//!   (a few minutes); nothing is pulled from a private registry.
//! - Each container: 4 GB RAM, 2 CPUs, the viewer (noVNC) on 127.0.0.1 only,
//!   and only the bot's own working folder mounted (at /home/bot/work).
//! - Idle desktops are stopped after 30 minutes.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::{json, Map, Value};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::agent::{Approval, ExtraTools, ToolOutput};
use crate::ai::ToolCall;

pub const DOCKERFILE: &str = include_str!("desktop.Dockerfile");
const IDLE: Duration = Duration::from_secs(30 * 60);

pub fn image_tag() -> String {
    use sha2::Digest;
    let h = sha2::Sha256::digest(DOCKERFILE.as_bytes());
    format!("blbots-desktop:{}", h.iter().take(6).map(|b| format!("{b:02x}")).collect::<String>())
}

pub fn container_name(bot_id: &str) -> String {
    format!("blbots-{bot_id}")
}

fn docker() -> Command {
    let mut c = Command::new(super::engines::find_bin("docker", None).unwrap_or_else(|| PathBuf::from("docker")));
    c.stdin(Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    {
        c.creation_flags(0x0800_0000);
    }
    c
}

async fn out(mut c: Command, timeout: Duration) -> Result<String, String> {
    c.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = c.spawn().map_err(|e| format!("Docker is not available: {e}. Install Docker Desktop to give bots a computer."))?;
    let o = tokio::time::timeout(timeout, child.wait_with_output()).await.map_err(|_| "Docker took too long".to_string())?.map_err(|e| e.to_string())?;
    if o.status.success() {
        Ok(String::from_utf8_lossy(&o.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&o.stderr).trim().chars().take(600).collect())
    }
}

pub async fn docker_ok() -> Result<(), String> {
    let mut c = docker();
    c.args(["version", "--format", "{{.Server.Version}}"]);
    out(c, Duration::from_secs(15)).await.map(|_| ()).map_err(|e| format!("Docker is not running ({e}). Start Docker Desktop, then try again."))
}

async fn ensure_image() -> Result<String, String> {
    let tag = image_tag();
    let mut c = docker();
    c.args(["image", "inspect", &tag]);
    if out(c, Duration::from_secs(20)).await.is_ok() {
        return Ok(tag);
    }
    let mut c = docker();
    c.args(["build", "-t", &tag, "-"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().map_err(|e| format!("Docker is not available: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(DOCKERFILE.as_bytes()).await.map_err(|e| e.to_string())?;
    }
    let o = tokio::time::timeout(Duration::from_secs(1800), child.wait_with_output()).await.map_err(|_| "Building the desktop image took too long".to_string())?.map_err(|e| e.to_string())?;
    if !o.status.success() {
        return Err(format!("Could not build the desktop image: {}", String::from_utf8_lossy(&o.stderr).lines().rev().take(5).collect::<Vec<_>>().join(" | ")));
    }
    Ok(tag)
}

fn last_use() -> &'static Mutex<HashMap<String, Instant>> {
    static M: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    M.get_or_init(|| {
        // Stop desktops nobody used for a while.
        tokio::spawn(async {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                let idle: Vec<String> = last_use().lock().expect("computer").iter().filter(|(_, t)| t.elapsed() > IDLE).map(|(n, _)| n.clone()).collect();
                for name in idle {
                    last_use().lock().expect("computer").remove(&name);
                    let mut c = docker();
                    c.args(["stop", "-t", "5", &name]);
                    let _ = out(c, Duration::from_secs(60)).await;
                }
            }
        });
        Mutex::new(HashMap::new())
    })
}

/// Start (or reuse) the bot's desktop. Returns the noVNC viewer address.
pub async fn ensure(bot_id: &str, workdir: &Path) -> Result<String, String> {
    docker_ok().await?;
    let name = container_name(bot_id);
    last_use().lock().expect("computer").insert(name.clone(), Instant::now());
    let mut c = docker();
    c.args(["inspect", "-f", "{{.State.Running}}", &name]);
    match out(c, Duration::from_secs(20)).await.map(|s| s.trim().to_string()) {
        Ok(s) if s == "true" => {}
        Ok(_) => {
            let mut c = docker();
            c.args(["start", &name]);
            out(c, Duration::from_secs(60)).await?;
        }
        Err(_) => {
            let tag = ensure_image().await?;
            let mount = format!("{}:/home/bot/work", workdir.display());
            let mut c = docker();
            c.args(["run", "-d", "--name", &name, "--memory", "4g", "--cpus", "2", "--shm-size", "512m", "--pids-limit", "1024", "--security-opt", "no-new-privileges", "-p", "127.0.0.1::6080", "-v", &mount, "--label", "blbots=1", &tag]);
            out(c, Duration::from_secs(120)).await?;
            // Give the desktop a moment to come up.
            tokio::time::sleep(Duration::from_secs(4)).await;
        }
    }
    let mut c = docker();
    c.args(["port", &name, "6080/tcp"]);
    let p = out(c, Duration::from_secs(20)).await?;
    let port = p.lines().find_map(|l| l.rsplit(':').next().and_then(|x| x.trim().parse::<u16>().ok())).ok_or("Could not find the desktop viewer port")?;
    Ok(format!("http://127.0.0.1:{port}/vnc.html?autoconnect=1&resize=scale"))
}

pub async fn stop(bot_id: &str) -> Result<(), String> {
    let mut c = docker();
    c.args(["stop", "-t", "5", &container_name(bot_id)]);
    out(c, Duration::from_secs(60)).await.map(|_| ())
}

async fn exec(name: &str, script: &str, timeout: Duration) -> Result<String, String> {
    let mut c = docker();
    c.args(["exec", "-u", "bot", "-e", "DISPLAY=:1", name, "bash", "-lc", script]);
    out(c, timeout).await
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The `computer` and `vm_exec` tools for one bot.
pub struct Computer {
    pub bot_id: String,
    pub workdir: PathBuf,
}

impl Computer {
    async fn screenshot(&self, name: &str) -> Result<String, String> {
        let b64 = exec(name, "scrot -o -q 70 /tmp/shot.jpg >/dev/null 2>&1 && base64 -w0 /tmp/shot.jpg", Duration::from_secs(30)).await?;
        let raw = base64::engine::general_purpose::STANDARD.decode(b64.trim()).map_err(|_| "The screenshot was not readable".to_string())?;
        if raw.len() > 6 * 1024 * 1024 {
            return Err("The screenshot was too large".into());
        }
        Ok(format!("data:image/jpeg;base64,{}", b64.trim()))
    }

    async fn act(&self, args: &Map<String, Value>) -> Result<ToolOutput, String> {
        let viewer = ensure(&self.bot_id, &self.workdir).await?;
        let name = container_name(&self.bot_id);
        let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let n = |k: &str| args.get(k).and_then(Value::as_i64).unwrap_or(0).clamp(0, 10_000);
        let action = s("action");
        let script = match action.as_str() {
            "screenshot" => String::new(),
            "click" | "double_click" | "right_click" => {
                let (btn, reps) = match action.as_str() { "right_click" => (3, 1), "double_click" => (1, 2), _ => (1, 1) };
                format!("xdotool mousemove {} {} click --repeat {reps} {btn}", n("x"), n("y"))
            }
            "move" => format!("xdotool mousemove {} {}", n("x"), n("y")),
            "type" => format!("xdotool type --delay 15 -- {}", sh_quote(&s("text").chars().take(4000).collect::<String>())),
            "key" => {
                let k = s("keys");
                if k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || "+_- ".contains(c)) {
                    return Err("keys must look like ctrl+l or Return".into());
                }
                format!("xdotool key {k}")
            }
            "scroll" => {
                let btn = if s("direction") == "up" { 4 } else { 5 };
                format!("xdotool click --repeat {} {btn}", n("amount").clamp(1, 30))
            }
            "open" => format!("(nohup firefox-esr {} >/dev/null 2>&1 &)", sh_quote(&s("url"))),
            other => return Err(format!("Unknown action {other}: use screenshot, click, double_click, right_click, move, type, key, scroll or open")),
        };
        if !script.is_empty() {
            exec(&name, &script, Duration::from_secs(30)).await?;
            tokio::time::sleep(Duration::from_millis(600)).await;
        }
        // Every action returns a fresh look at the screen.
        let shot = self.screenshot(&name).await?;
        Ok(ToolOutput { text: format!("Done: {action}. Screenshot attached (1280x800). Watch live: {viewer}"), images: vec![shot] })
    }
}

impl ExtraTools for Computer {
    fn schemas(&self) -> Vec<Value> {
        vec![
            json!({ "type": "function", "function": {
                "name": "computer",
                "description": "Use your own Linux desktop (1280x800, XFCE, Firefox). Every action returns a screenshot so you can see the result. Coordinates are pixels from the top-left. Your working folder is at /home/bot/work.",
                "parameters": { "type": "object", "properties": {
                    "action": { "type": "string", "enum": ["screenshot", "click", "double_click", "right_click", "move", "type", "key", "scroll", "open"] },
                    "x": { "type": "integer" }, "y": { "type": "integer" },
                    "text": { "type": "string", "description": "type: the text" },
                    "keys": { "type": "string", "description": "key: e.g. Return, ctrl+l, alt+F4" },
                    "direction": { "type": "string", "enum": ["up", "down"] }, "amount": { "type": "integer" },
                    "url": { "type": "string", "description": "open: a web address to open in Firefox" }
                }, "required": ["action"] } } }),
            json!({ "type": "function", "function": {
                "name": "vm_exec",
                "description": "Run a shell command inside your own Linux desktop (not on the user's computer). Your working folder is /home/bot/work.",
                "parameters": { "type": "object", "properties": { "command": { "type": "string" } }, "required": ["command"] } } }),
        ]
    }

    fn handles(&self, name: &str) -> bool {
        matches!(name, "computer" | "vm_exec")
    }

    fn needs_approval(&self, level: Approval, call: &ToolCall) -> bool {
        // Looking is free; doing things asks at the Ask level only (the
        // desktop is isolated from the user's computer).
        level == Approval::Ask && call.name == "vm_exec"
    }

    fn run<'a>(&'a self, call: &'a ToolCall) -> Pin<Box<dyn Future<Output = ToolOutput> + Send + 'a>> {
        Box::pin(async move {
            let args: Map<String, Value> = serde_json::from_str(&call.arguments).unwrap_or_default();
            let res = if call.name == "vm_exec" {
                match ensure(&self.bot_id, &self.workdir).await {
                    Ok(_) => {
                        let cmd = args.get("command").and_then(Value::as_str).unwrap_or("");
                        exec(&container_name(&self.bot_id), &format!("cd ~/work && timeout 120 bash -lc {} 2>&1 | head -c 65536", sh_quote(cmd)), Duration::from_secs(130)).await.map(ToolOutput::from)
                    }
                    Err(e) => Err(e),
                }
            } else {
                self.act(&args).await
            };
            res.unwrap_or_else(|e| ToolOutput::from(format!("Error: {e}")))
        })
    }
}

pub fn for_bot(bot: &super::Bot, workdir: PathBuf) -> Option<Arc<dyn ExtraTools>> {
    bot.computer.then(|| Arc::new(Computer { bot_id: bot.id.clone(), workdir }) as Arc<dyn ExtraTools>)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_tag() {
        assert_eq!(container_name("b123"), "blbots-b123");
        assert!(image_tag().starts_with("blbots-desktop:") && image_tag().len() == "blbots-desktop:".len() + 12);
        assert_eq!(sh_quote("it's"), "'it'\\''s'");
        assert!(DOCKERFILE.contains("xdotool") && DOCKERFILE.contains("6080"));
    }

    /// Needs Docker; run in CI with `cargo test -p blaze-core desktop_e2e -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn desktop_e2e() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("hello.txt"), "hi from the host").unwrap();
        let c = Computer { bot_id: format!("e2e{}", std::process::id()), workdir: d.path().to_path_buf() };
        let call = |name: &str, args: Value| ToolCall { id: "1".into(), name: name.into(), arguments: args.to_string(), extra: None };
        let shot = c.run(&call("computer", json!({"action": "screenshot"}))).await;
        assert!(shot.text.starts_with("Done"), "{}", shot.text);
        assert!(shot.images[0].starts_with("data:image/jpeg;base64,"));
        let click = c.run(&call("computer", json!({"action": "click", "x": 640, "y": 400}))).await;
        assert!(click.text.starts_with("Done"), "{}", click.text);
        let typed = c.run(&call("computer", json!({"action": "type", "text": "it's ok"}))).await;
        assert!(typed.text.starts_with("Done"), "{}", typed.text);
        let ls = c.run(&call("vm_exec", json!({"command": "cat hello.txt; whoami"}))).await;
        assert!(ls.text.contains("hi from the host") && ls.text.contains("bot"), "{}", ls.text);
        stop(&c.bot_id).await.unwrap();
        let mut rm = docker();
        rm.args(["rm", "-f", &container_name(&c.bot_id)]);
        let _ = out(rm, Duration::from_secs(60)).await;
    }
}
