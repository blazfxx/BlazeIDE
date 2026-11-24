//! Bots on the user's own agent CLIs: Claude Code (`claude`), OpenAI Codex
//! (`codex`) and a Grok CLI (`grok`). They use the user's own logins and
//! subscriptions, not BlazeAPI. Each bot keeps one CLI session per thread or
//! room, so the CLI remembers the conversation itself; we send only what is
//! new since the bot last spoke.
//!
//! Approval levels map onto each CLI's own modes (these CLIs run without a
//! terminal here, so nothing can be asked mid-turn):
//!   Ask   -> Claude `dontAsk` (anything that would prompt is refused),
//!            Codex `read-only` sandbox
//!   Edits -> Claude `acceptEdits`, Codex `workspace-write`
//!   Full  -> Claude `bypassPermissions`, Codex `danger-full-access`

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{Bot, Bots, Engine};
use crate::agent::{AgentConfig, AgentEvent, Approval, TurnOutcome};
use crate::ai::ChatMessage;

/// Find a CLI: the bot's own path, else on PATH (plus the usual install
/// folders GUI apps do not have on their PATH).
pub fn find_bin(name: &str, custom: Option<&str>) -> Option<PathBuf> {
    if let Some(c) = custom.map(str::trim).filter(|c| !c.is_empty()) {
        let p = PathBuf::from(c);
        return p.is_file().then_some(p);
    }
    let exe = if cfg!(windows) { format!("{name}.cmd") } else { name.to_string() };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(h) = dirs::home_dir() {
        for d in [".local/bin", ".npm-global/bin", ".bun/bin", ".claude/local", ".volta/bin", "AppData/Roaming/npm"] {
            dirs.push(h.join(d));
        }
    }
    dirs.push(PathBuf::from("/opt/homebrew/bin"));
    dirs.push(PathBuf::from("/usr/local/bin"));
    for d in dirs {
        for cand in [d.join(&exe), d.join(name), d.join(format!("{name}.exe"))] {
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

/// Which CLIs this computer has (for the bot editor).
pub fn available() -> Vec<(String, Option<String>)> {
    ["claude", "codex", "grok"].iter().map(|n| (n.to_string(), find_bin(n, None).map(|p| p.display().to_string()))).collect()
}

fn sessions_path(bots: &Bots, bot: &str) -> PathBuf {
    bots.dir().join(bot).join("sessions.json")
}

fn load_sessions(bots: &Bots, bot: &str) -> HashMap<String, String> {
    std::fs::read(sessions_path(bots, bot)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_session(bots: &Bots, bot: &str, key: &str, id: &str) {
    let mut m = load_sessions(bots, bot);
    m.insert(key.to_string(), id.to_string());
    if let Ok(b) = serde_json::to_vec(&m) {
        let _ = std::fs::write(sessions_path(bots, bot), b);
    }
}

/// What is new for the bot since it last answered.
pub fn new_input(conv: &[ChatMessage]) -> String {
    let start = conv.iter().rposition(|m| m.role == "assistant").map(|i| i + 1).unwrap_or(0);
    conv[start..].iter().filter(|m| m.role == "user").map(|m| m.content.trim()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n\n")
}

/// Parse one line of Claude Code's `--output-format stream-json`.
pub fn parse_claude(line: &str, out: &mut Vec<AgentEvent>, session: &mut Option<String>, final_text: &mut String) {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return };
    if let Some(s) = v.get("session_id").and_then(Value::as_str) {
        *session = Some(s.to_string());
    }
    match v.get("type").and_then(Value::as_str) {
        Some("assistant") => {
            for part in v.pointer("/message/content").and_then(Value::as_array).into_iter().flatten() {
                match part.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        let t = part.get("text").and_then(Value::as_str).unwrap_or("");
                        if !t.is_empty() {
                            final_text.push_str(t);
                            final_text.push('\n');
                            out.push(AgentEvent::Text { text: format!("{t}\n") });
                        }
                    }
                    Some("tool_use") => {
                        let name = part.get("name").and_then(Value::as_str).unwrap_or("tool");
                        let input = part.get("input").cloned().unwrap_or(Value::Null);
                        let detail = ["command", "file_path", "path", "pattern", "url"].iter().find_map(|k| input.get(*k).and_then(Value::as_str)).unwrap_or("");
                        let id = part.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                        out.push(AgentEvent::ToolStart { id, name: name.to_string(), summary: format!("{name} {detail}").trim().chars().take(200).collect() });
                    }
                    _ => {}
                }
            }
        }
        Some("user") => {
            for part in v.pointer("/message/content").and_then(Value::as_array).into_iter().flatten() {
                if part.get("type").and_then(Value::as_str) == Some("tool_result") {
                    let id = part.get("tool_use_id").and_then(Value::as_str).unwrap_or("").to_string();
                    let is_error = part.get("is_error").and_then(Value::as_bool).unwrap_or(false);
                    let result = match part.get("content") {
                        Some(Value::String(s)) => s.clone(),
                        Some(Value::Array(a)) => a.iter().filter_map(|x| x.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
                        _ => String::new(),
                    };
                    out.push(AgentEvent::ToolEnd { id, name: String::new(), result: result.chars().take(2000).collect(), is_error });
                }
            }
        }
        Some("result") => {
            if let Some(u) = v.get("usage") {
                let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
                out.push(AgentEvent::Usage { input_tokens: n("input_tokens") + n("cache_read_input_tokens") + n("cache_creation_input_tokens"), output_tokens: n("output_tokens") });
            }
            if v.get("is_error").and_then(Value::as_bool) == Some(true) || v.get("subtype").and_then(Value::as_str).is_some_and(|s| s.starts_with("error")) {
                let msg = v.get("result").and_then(Value::as_str).unwrap_or("Claude Code stopped with an error").to_string();
                out.push(AgentEvent::Error { message: msg, status: None });
            } else if final_text.trim().is_empty() {
                if let Some(r) = v.get("result").and_then(Value::as_str) {
                    final_text.push_str(r);
                }
            }
        }
        _ => {}
    }
}

/// Parse one line of `codex exec --json`.
pub fn parse_codex(line: &str, out: &mut Vec<AgentEvent>, session: &mut Option<String>, final_text: &mut String) {
    let Ok(v) = serde_json::from_str::<Value>(line) else { return };
    let item = v.get("item");
    let s = |x: Option<&Value>, k: &str| x.and_then(|i| i.get(k)).and_then(Value::as_str).unwrap_or("").to_string();
    match v.get("type").and_then(Value::as_str) {
        Some("thread.started") => *session = Some(s(Some(&v), "thread_id")).filter(|t| !t.is_empty()),
        Some("item.started") if s(item, "type") == "command_execution" => {
            out.push(AgentEvent::ToolStart { id: s(item, "id"), name: "run_command".into(), summary: format!("Run: {}", s(item, "command")).chars().take(200).collect() });
        }
        Some("item.completed") => match s(item, "type").as_str() {
            "agent_message" => {
                let t = s(item, "text");
                if !t.is_empty() {
                    final_text.push_str(&t);
                    final_text.push('\n');
                    out.push(AgentEvent::Text { text: format!("{t}\n") });
                }
            }
            "command_execution" => {
                let code = item.and_then(|i| i.get("exit_code")).and_then(Value::as_i64).unwrap_or(0);
                out.push(AgentEvent::ToolEnd { id: s(item, "id"), name: "run_command".into(), result: s(item, "aggregated_output").chars().take(2000).collect(), is_error: code != 0 });
            }
            "file_change" => {
                for c in item.and_then(|i| i.get("changes")).and_then(Value::as_array).into_iter().flatten() {
                    let path = c.get("path").and_then(Value::as_str).unwrap_or("").to_string();
                    let created = c.get("kind").and_then(Value::as_str) == Some("add");
                    out.push(AgentEvent::FileChanged { rel: path, added: 0, removed: 0, created });
                }
            }
            "error" => out.push(AgentEvent::Error { message: s(item, "message"), status: None }),
            _ => {}
        },
        Some("turn.completed") => {
            if let Some(u) = v.get("usage") {
                let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
                out.push(AgentEvent::Usage { input_tokens: n("input_tokens"), output_tokens: n("output_tokens") });
            }
        }
        Some("turn.failed") | Some("error") => {
            let msg = v.pointer("/error/message").and_then(Value::as_str).or_else(|| v.get("message").and_then(Value::as_str)).unwrap_or("Codex stopped with an error");
            out.push(AgentEvent::Error { message: msg.to_string(), status: None });
        }
        _ => {}
    }
}

fn base_cmd(bin: &Path, cwd: &Path) -> Command {
    let mut c = Command::new(bin);
    c.current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    {
        c.creation_flags(0x0800_0000);
    }
    c
}

#[allow(clippy::too_many_arguments)]
pub async fn run(engine: &Engine, bots: &Bots, bot: &Bot, cfg: &AgentConfig, conversation: &mut Vec<ChatMessage>, session_key: &str, events: &mpsc::UnboundedSender<AgentEvent>, cancel: &CancellationToken) -> TurnOutcome {
    let mut outcome = TurnOutcome { rounds: 1, ..Default::default() };
    let fail = |outcome: &mut TurnOutcome, msg: String| {
        let _ = events.send(AgentEvent::Error { message: msg.clone(), status: None });
        outcome.error = Some(msg);
        let _ = events.send(AgentEvent::Done { stopped: false, rounds: 1 });
    };
    let (name, custom) = match engine {
        Engine::Claude { bin } => ("claude", bin.as_deref()),
        Engine::Codex { bin } => ("codex", bin.as_deref()),
        Engine::Grok { bin } => ("grok", bin.as_deref()),
        Engine::Blaze => unreachable!("Blaze bots use the agent"),
    };
    let Some(bin) = find_bin(name, custom) else {
        fail(&mut outcome, format!("{} is not installed on this computer (or not on the PATH). Install it and sign in, or set its path in the bot's settings.", engine.label()));
        return outcome;
    };
    let input = new_input(conversation);
    if input.is_empty() {
        fail(&mut outcome, "Nothing new to answer.".into());
        return outcome;
    }
    let sessions = load_sessions(bots, &bot.id);
    let prior = sessions.get(session_key).cloned();
    let persona = cfg.extra_system.clone().unwrap_or_default();
    let model = bot.model.trim();
    let mut cmd = base_cmd(&bin, &cfg.root);
    let is_claude = matches!(engine, Engine::Claude { .. });
    let is_codex = matches!(engine, Engine::Codex { .. });
    if is_claude {
        cmd.arg("-p").arg(&input).args(["--output-format", "stream-json", "--verbose", "--permission-prompts", "none"]);
        cmd.arg("--permission-mode").arg(match bot.approval {
            Approval::Ask => "dontAsk",
            Approval::Edits => "acceptEdits",
            Approval::Full => "bypassPermissions",
        });
        cmd.arg("--append-system-prompt").arg(&persona);
        if !model.is_empty() {
            cmd.arg("--model").arg(model);
        }
        if let Some(s) = &prior {
            cmd.arg("--resume").arg(s);
        }
    } else if is_codex {
        cmd.arg("exec");
        if let Some(s) = &prior {
            cmd.arg("resume").arg(s);
        }
        cmd.args(["--json", "--skip-git-repo-check"]);
        cmd.arg("-s").arg(match bot.approval {
            Approval::Ask => "read-only",
            Approval::Edits => "workspace-write",
            Approval::Full => "danger-full-access",
        });
        if prior.is_none() {
            cmd.arg("-C").arg(&cfg.root);
        }
        if !model.is_empty() {
            cmd.arg("-m").arg(model);
        }
        // Codex has no system-prompt flag: the persona leads a new session.
        let first = if prior.is_none() { format!("{persona}\n\n---\n\n{input}") } else { input.clone() };
        cmd.arg(first);
    } else {
        // Grok CLI: headless prompt, plain text answer.
        cmd.arg("-p").arg(format!("{persona}\n\n---\n\n{input}"));
        if !model.is_empty() {
            cmd.arg("-m").arg(model);
        }
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            fail(&mut outcome, format!("Could not start {}: {e}", engine.label()));
            return outcome;
        }
    };
    let stdout = child.stdout.take().expect("stdout");
    let mut stderr = child.stderr.take().expect("stderr");
    let err_task = tokio::spawn(async move {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s).await;
        s
    });
    let mut lines = BufReader::new(stdout).lines();
    let mut session: Option<String> = None;
    let mut text = String::new();
    let mut evs = Vec::new();
    let mut plain = String::new();
    loop {
        let next = tokio::select! {
            l = lines.next_line() => l,
            _ = cancel.cancelled() => {
                let _ = child.kill().await;
                outcome.stopped = true;
                break;
            }
        };
        let Ok(Some(line)) = next else { break };
        if is_claude {
            parse_claude(&line, &mut evs, &mut session, &mut text);
        } else if is_codex {
            parse_codex(&line, &mut evs, &mut session, &mut text);
        } else {
            plain.push_str(&line);
            plain.push('\n');
            evs.push(AgentEvent::Text { text: format!("{line}\n") });
        }
        for e in evs.drain(..) {
            if let AgentEvent::Usage { input_tokens, output_tokens } = &e {
                outcome.input_tokens += input_tokens;
                outcome.output_tokens += output_tokens;
            }
            if let AgentEvent::Error { message, .. } = &e {
                outcome.error = Some(message.clone());
            }
            let _ = events.send(e);
        }
    }
    let status = child.wait().await.ok();
    let err_text = err_task.await.unwrap_or_default();
    if !is_claude && !is_codex {
        text = plain;
    }
    if let Some(s) = &session {
        save_session(bots, &bot.id, session_key, s);
    }
    if !outcome.stopped && status.is_some_and(|s| !s.success()) && outcome.error.is_none() {
        let tail: String = err_text.lines().rev().take(3).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join(" ");
        let msg = format!("{} exited with an error{}", engine.label(), if tail.trim().is_empty() { ".".into() } else { format!(": {}", tail.chars().take(400).collect::<String>()) });
        let _ = events.send(AgentEvent::Error { message: msg.clone(), status: None });
        outcome.error = Some(msg);
    }
    if !text.trim().is_empty() {
        conversation.push(ChatMessage { role: "assistant".into(), content: text.trim().to_string(), ..Default::default() });
    }
    let _ = events.send(AgentEvent::Done { stopped: outcome.stopped, rounds: 1 });
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_stream_json() {
        let lines = [
            r#"{"type":"system","subtype":"init","session_id":"s-123","tools":[]}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Looking."},{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]},"session_id":"s-123"}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"a.txt","is_error":false}]}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"There is a.txt."}]}}"#,
            r#"{"type":"result","subtype":"success","result":"There is a.txt.","session_id":"s-123","usage":{"input_tokens":10,"cache_read_input_tokens":5,"output_tokens":7}}"#,
        ];
        let (mut out, mut session, mut text) = (Vec::new(), None, String::new());
        for l in lines {
            parse_claude(l, &mut out, &mut session, &mut text);
        }
        assert_eq!(session.as_deref(), Some("s-123"));
        assert_eq!(text.trim(), "Looking.\nThere is a.txt.");
        assert!(out.iter().any(|e| matches!(e, AgentEvent::ToolStart { summary, .. } if summary == "Bash ls")));
        assert!(out.iter().any(|e| matches!(e, AgentEvent::ToolEnd { result, is_error: false, .. } if result == "a.txt")));
        assert!(out.iter().any(|e| matches!(e, AgentEvent::Usage { input_tokens: 15, output_tokens: 7 })));
    }

    #[test]
    fn codex_jsonl() {
        let lines = [
            r#"{"type":"thread.started","thread_id":"th-9"}"#,
            r#"{"type":"turn.started"}"#,
            r#"{"type":"item.started","item":{"id":"i1","type":"command_execution","command":"cargo test","status":"in_progress"}}"#,
            r#"{"type":"item.completed","item":{"id":"i1","type":"command_execution","command":"cargo test","aggregated_output":"ok","exit_code":0}}"#,
            r#"{"type":"item.completed","item":{"id":"i2","type":"file_change","changes":[{"path":"src/a.rs","kind":"update"}]}}"#,
            r#"{"type":"item.completed","item":{"id":"i3","type":"agent_message","text":"Tests pass."}}"#,
            r#"{"type":"turn.completed","usage":{"input_tokens":100,"output_tokens":20}}"#,
        ];
        let (mut out, mut session, mut text) = (Vec::new(), None, String::new());
        for l in lines {
            parse_codex(l, &mut out, &mut session, &mut text);
        }
        assert_eq!(session.as_deref(), Some("th-9"));
        assert_eq!(text.trim(), "Tests pass.");
        assert!(out.iter().any(|e| matches!(e, AgentEvent::ToolStart { summary, .. } if summary == "Run: cargo test")));
        assert!(out.iter().any(|e| matches!(e, AgentEvent::FileChanged { rel, .. } if rel == "src/a.rs")));
        assert!(out.iter().any(|e| matches!(e, AgentEvent::Usage { input_tokens: 100, output_tokens: 20 })));
    }

    #[test]
    fn only_new_input_is_sent() {
        let m = |r: &str, c: &str| ChatMessage { role: r.into(), content: c.into(), ..Default::default() };
        let conv = vec![m("user", "old"), m("assistant", "reply"), m("user", "[Orbit] did X"), m("user", "now do Y")];
        assert_eq!(new_input(&conv), "[Orbit] did X\n\nnow do Y");
        assert_eq!(new_input(&[m("user", "first")]), "first");
    }
}
