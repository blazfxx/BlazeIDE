//! `blaze -p "prompt"`: one turn without the full-screen UI. The answer goes
//! to stdout; steps go to stderr. Approvals are asked on the terminal when
//! there is one, and refused otherwise (pass `-a full` or `-a edits` in
//! scripts). `--json` prints every event as one JSON line instead.

use std::io::{IsTerminal, Write};
use std::path::Path;

use blaze_core::agent::{self, AgentEvent, Approval, HostRequest};
use blaze_core::ai::ChatMessage;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::session::{Chat, Session};

#[allow(clippy::too_many_arguments)]
pub async fn run(s: &mut Session, root: &Path, model: &str, mode: &str, approval: Approval, prompt: &str, json: bool, resume: Option<&str>) -> Result<(), String> {
    if s.key.is_none() && !model.starts_with("custom:") {
        return Err("Not signed in. Run `blaze login` (or set BLAZE_API_KEY).".into());
    }
    if let Err(e) = s.load_models(false).await {
        if !json {
            eprintln!("(model list unavailable: {e})");
        }
    }
    let mut chat = match resume {
        Some(id) => Chat::load(root, id)?,
        None => Chat::new(root, mode, model),
    };
    chat.model = model.to_string();
    chat.mode = mode.to_string();
    let user_index = chat.messages.len();
    chat.messages.push(ChatMessage { role: "user".into(), content: prompt.to_string(), ..Default::default() });

    let backend = s.backend();
    let cfg = s.agent_config(root, model, mode, approval);
    let (etx, mut erx) = mpsc::unbounded_channel::<AgentEvent>();
    let (htx, mut hrx) = mpsc::channel::<HostRequest>(4);
    let cancel = CancellationToken::new();
    let interactive = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();

    // Ctrl+C stops the turn (a second one exits).
    let c2 = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            c2.cancel();
            if tokio::signal::ctrl_c().await.is_ok() {
                blaze_core::web::chrome::close_all().await;
                std::process::exit(130);
            }
        }
    });

    // Approvals and questions.
    tokio::spawn(async move {
        while let Some(req) = hrx.recv().await {
            match req {
                HostRequest::Approve { summary, reply, .. } => {
                    let ok = interactive && ask_line(&format!("Allow? {summary} [y/N] ")).await.is_some_and(|a| matches!(a.trim().to_lowercase().as_str(), "y" | "yes"));
                    if !interactive {
                        eprintln!("refused (no terminal to ask; use -a auto or -a yolo): {summary}");
                    }
                    let _ = reply.send(ok);
                }
                HostRequest::Ask { questions, reply } => {
                    let mut answers = Vec::new();
                    for (q, opts) in questions {
                        let line = if opts.is_empty() { format!("{q}\n> ") } else { format!("{q} ({})\n> ", opts.join(" / ")) };
                        answers.push(if interactive { ask_line(&line).await.unwrap_or_default() } else { String::new() });
                    }
                    let _ = reply.send(answers);
                }
            }
        }
    });

    let printer = tokio::spawn(async move {
        let mut out = std::io::stdout();
        let mut err = std::io::stderr();
        let mut ended_with_newline = true;
        while let Some(ev) = erx.recv().await {
            if json {
                let _ = writeln!(out, "{}", serde_json::to_string(&ev).unwrap_or_default());
                continue;
            }
            match ev {
                AgentEvent::Text { text } => {
                    let _ = write!(out, "{text}");
                    let _ = out.flush();
                    ended_with_newline = text.ends_with('\n');
                }
                AgentEvent::ToolStart { summary, .. } => {
                    if !ended_with_newline {
                        let _ = writeln!(out);
                        ended_with_newline = true;
                    }
                    let _ = writeln!(err, "● {summary}");
                }
                AgentEvent::ToolEnd { result, is_error: true, .. } => {
                    let _ = writeln!(err, "  ✗ {}", first_line(&result));
                }
                AgentEvent::FileChanged { rel, added, removed, created } => {
                    let _ = writeln!(err, "  {} {rel}  +{added} -{removed}", if created { "new" } else { "edit" });
                }
                AgentEvent::Notice { text } => {
                    let _ = writeln!(err, "{text}");
                }
                AgentEvent::Error { message, .. } => {
                    let _ = writeln!(err, "error: {message}");
                }
                _ => {}
            }
        }
        if !json && !ended_with_newline {
            let _ = writeln!(out);
        }
    });

    let outcome = agent::run_turn(&backend, &cfg, &mut chat.messages, &etx, &htx, &cancel).await;
    drop(etx);
    let _ = printer.await;
    chat.add_checkpoint(user_index, &outcome.changed);
    if let Err(e) = chat.save() {
        eprintln!("(could not save the chat: {e})");
    } else if !json {
        eprintln!("chat {} saved · resume with: blaze -r {}", chat.id, chat.id);
    }
    match outcome.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

fn first_line(s: &str) -> String {
    let l = s.lines().next().unwrap_or("");
    if l.chars().count() > 200 { format!("{}…", l.chars().take(200).collect::<String>()) } else { l.to_string() }
}

async fn ask_line(prompt: &str) -> Option<String> {
    let prompt = prompt.to_string();
    tokio::task::spawn_blocking(move || {
        eprint!("{prompt}");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        std::io::stdin().read_line(&mut line).ok()?;
        Some(line)
    })
    .await
    .ok()
    .flatten()
}
