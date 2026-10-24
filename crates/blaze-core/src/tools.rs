//! Agent tools: the OpenAI function schemas sent to the model, and the
//! execution of the ones that run in the core. Every path is resolved with
//! `paths::resolve_in`, so the model can never reach outside the project.
//!
//! apply_diff, ask_question, write_plan and generate_image are handled by the
//! UI (they need the diff view, a prompt, or the editor), not here.

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Map, Value};
use tokio::io::AsyncReadExt;

use crate::error::{Error, Result};
use crate::files::{self, SKIP_DIRS};
use crate::paths::resolve_in;

const READ_CAP: usize = 100 * 1024;
const SEARCH_CAP: usize = 100;
const CMD_TIMEOUT: Duration = Duration::from_secs(120);
const CMD_OUTPUT_CAP: usize = 64 * 1024;

fn tool(name: &str, description: &str, props: Value, required: &[&str]) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": name,
            "description": description,
            "parameters": { "type": "object", "properties": props, "required": required },
        }
    })
}

fn read_file_tool() -> Value {
    tool("read_file", "Read a file in the project. Path is relative to the project root.",
        json!({ "path": { "type": "string", "description": "Relative path of the file to read" } }), &["path"])
}
fn write_file_tool() -> Value {
    tool("write_file", "Create a new file, or replace an existing file in full. Prefer apply_diff for edits to existing files. Creates parent directories.",
        json!({
            "path": { "type": "string", "description": "Relative path of the file to write" },
            "content": { "type": "string", "description": "Full new file content" }
        }), &["path", "content"])
}
fn apply_diff_tool() -> Value {
    tool("apply_diff", "Make a surgical edit to an existing file using one or more SEARCH/REPLACE blocks. Use this for any change to a file that already exists. Format: \"<<<<<<< SEARCH\" then an optional \":start_line:N\" hint, the exact text to find, \"=======\", the replacement text, then \">>>>>>> REPLACE\". The SEARCH text must match the current file content exactly.",
        json!({
            "path": { "type": "string", "description": "Relative path of the file to edit" },
            "diff": { "type": "string", "description": "One or more SEARCH/REPLACE blocks" }
        }), &["path", "diff"])
}
fn list_directory_tool() -> Value {
    tool("list_directory", "List the contents of a project directory.",
        json!({ "path": { "type": "string", "description": "Relative directory path; \".\" for the root" } }), &["path"])
}
fn run_command_tool() -> Value {
    tool("run_command", "Run a shell command in the project root and return its output. The user approves each command unless auto-approve is on.",
        json!({ "command": { "type": "string", "description": "The shell command to run" } }), &["command"])
}
fn search_files_tool() -> Value {
    tool("search_files", "Search file names and contents across the project.",
        json!({ "query": { "type": "string", "description": "Case-insensitive text to search for" } }), &["query"])
}
fn ask_question_tool() -> Value {
    tool("ask_question", "Ask the user one or more questions when you are genuinely blocked on a decision only they can make (e.g. which library, a design choice, whether to overwrite something). The IDE shows the questions with the options you provide; the user picks an option or types a custom answer. Returns their answers. Do not use for things you can decide or discover yourself.",
        json!({
            "questions": {
                "type": "array",
                "description": "The questions to ask.",
                "items": {
                    "type": "object",
                    "properties": {
                        "question": { "type": "string", "description": "The question text" },
                        "options": { "type": "array", "items": { "type": "string" }, "description": "Suggested answer options (the user can also type a custom answer)" }
                    },
                    "required": ["question"]
                }
            }
        }), &["questions"])
}
fn generate_image_tool() -> Value {
    tool("generate_image", "Generate a real image from a text prompt (AI image generation) and save it into the project. Use this to create actual assets for a site you are building (hero images, icons, logos, backgrounds, illustrations) instead of placeholder boxes or external stock URLs. The image is written to the given path; reference it from your HTML/CSS. Be descriptive in the prompt (subject, style, colors, composition).",
        json!({
            "prompt": { "type": "string", "description": "A detailed description of the image to generate" },
            "path": { "type": "string", "description": "Relative path to save the image, ending in .png (e.g. \"assets/hero.png\")" },
            "aspect_ratio": { "type": "string", "description": "Optional aspect ratio: \"1:1\", \"16:9\", \"9:16\", \"4:3\", or \"3:4\". Defaults to 1:1." }
        }), &["prompt", "path"])
}
fn write_plan_tool() -> Value {
    tool("write_plan", "Save the finished implementation plan. Call this ONCE, at the end, after gathering context and asking any clarifying questions. The full plan is saved to the project and opened in the editor; the short summary is shown in chat.",
        json!({
            "title": { "type": "string", "description": "Short descriptive name for what is being built. Used to name the plan file." },
            "summary": { "type": "string", "description": "A SHORT, skimmable overview for the chat: 2-4 sentences or a few bullet points." },
            "markdown": { "type": "string", "description": "The FULL detailed plan as markdown (## headings: Overview, Architecture, Steps, Files, Decisions). Opened in the editor." }
        }), &["title", "summary", "markdown"])
}

/// Tools offered for a mode. Ask mode gets none (returns None).
pub fn tools_for(mode: &str, image_gen: bool) -> Option<Vec<Value>> {
    match mode {
        "plan" => Some(vec![read_file_tool(), list_directory_tool(), search_files_tool(), ask_question_tool(), write_plan_tool()]),
        "agent" => {
            let mut t = vec![read_file_tool(), list_directory_tool(), search_files_tool(), apply_diff_tool(), write_file_tool()];
            if image_gen {
                t.push(generate_image_tool());
            }
            t.push(ask_question_tool());
            t.push(run_command_tool());
            Some(t)
        }
        _ => None,
    }
}

fn arg<'a>(args: &'a Map<String, Value>, key: &str) -> Result<&'a str> {
    args.get(key).and_then(Value::as_str).ok_or_else(|| Error::msg(format!("Missing required argument: {key}")))
}

/// Run one core tool. Always returns a string for the model: failures come
/// back as "Error: ..." rather than as an Err, so the agent can recover.
pub async fn run_tool(name: &str, args_json: &str, root: &Path) -> String {
    let args: Map<String, Value> = match serde_json::from_str::<Value>(if args_json.trim().is_empty() { "{}" } else { args_json }) {
        Ok(Value::Object(m)) => m,
        Ok(_) => Map::new(),
        Err(_) => return "Error: tool arguments were not valid JSON.".into(),
    };
    let res = match name {
        "read_file" => read_file(root, &args),
        "write_file" => write_file(root, &args),
        "list_directory" => list_directory(root, &args),
        "search_files" => search_files(root, &args),
        "run_command" => match arg(&args, "command") {
            Ok(cmd) => Ok(run_command(root, cmd).await),
            Err(e) => Err(e),
        },
        "apply_diff" | "ask_question" | "write_plan" | "generate_image" => {
            Err(Error::msg(format!("{name} is handled by the IDE, not run_tool.")))
        }
        other => Err(Error::msg(format!("unknown tool \"{other}\""))),
    };
    res.unwrap_or_else(|e| format!("Error: {e}"))
}

fn read_file(root: &Path, args: &Map<String, Value>) -> Result<String> {
    let rel = arg(args, "path")?;
    let text = files::read_text(&resolve_in(root, rel)?)?;
    if text.len() > READ_CAP {
        let mut cut = READ_CAP;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        Ok(format!("{}\n...[truncated]", &text[..cut]))
    } else {
        Ok(text)
    }
}

fn write_file(root: &Path, args: &Map<String, Value>) -> Result<String> {
    let rel = arg(args, "path")?;
    let content = arg(args, "content")?;
    files::write_text(&resolve_in(root, rel)?, content)?;
    Ok(format!("Wrote {rel}"))
}

fn list_directory(root: &Path, args: &Map<String, Value>) -> Result<String> {
    let rel = args.get("path").and_then(Value::as_str).unwrap_or(".");
    let dir = if rel.trim().is_empty() || rel.trim() == "." { dunce::canonicalize(root)? } else { resolve_in(root, rel)? };
    let mut names: Vec<String> = std::fs::read_dir(&dir)?
        .filter_map(|e| e.ok())
        .map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            if e.file_type().is_ok_and(|t| t.is_dir()) { format!("{n}/") } else { n }
        })
        .collect();
    names.sort();
    Ok(if names.is_empty() { "(empty directory)".into() } else { names.join("\n") })
}

fn search_files(root: &Path, args: &Map<String, Value>) -> Result<String> {
    let query = arg(args, "query")?;
    let root = dunce::canonicalize(root)?;
    let hits = files::search(&root, query);
    if hits.is_empty() {
        return Ok("No matches found.".into());
    }
    let lines: Vec<String> = hits
        .into_iter()
        .take(SEARCH_CAP)
        .map(|h| {
            let rel = Path::new(&h.path).strip_prefix(&root).map(|p| p.display().to_string()).unwrap_or(h.path);
            format!("{rel}:{}: {}", h.line, h.preview)
        })
        .collect();
    Ok(lines.join("\n"))
}

/// The user's shell, in a form that runs one command string.
pub fn shell_command(cmd: &str) -> tokio::process::Command {
    #[cfg(windows)]
    {
        let mut c = tokio::process::Command::new("powershell.exe");
        c.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", cmd]);
        // Do not flash a console window for every agent command.
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        c
    }
    #[cfg(not(windows))]
    {
        let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/sh".into());
        let mut c = tokio::process::Command::new(shell);
        c.args(["-lc", cmd]);
        c
    }
}

async fn run_command(root: &Path, cmd: &str) -> String {
    let mut c = shell_command(cmd);
    c.current_dir(root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = match c.spawn() {
        Ok(ch) => ch,
        Err(e) => return format!("Error: could not start the shell: {e}"),
    };
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    // Drain both pipes concurrently: reading one to the end first can
    // deadlock when the child fills the other pipe's buffer.
    let read_all = async move {
        let cap = CMD_OUTPUT_CAP as u64 * 4;
        let o = async {
            let mut buf = Vec::new();
            if let Some(s) = stdout.as_mut() {
                let _ = s.take(cap).read_to_end(&mut buf).await;
            }
            buf
        };
        let e = async {
            let mut buf = Vec::new();
            if let Some(s) = stderr.as_mut() {
                let _ = s.take(cap).read_to_end(&mut buf).await;
            }
            buf
        };
        tokio::join!(o, e)
    };
    let outcome = tokio::time::timeout(CMD_TIMEOUT, async {
        let (out, err) = read_all.await;
        let status = child.wait().await;
        (out, err, status)
    })
    .await;
    match outcome {
        Err(_) => format!("[command timed out after {}s and was stopped]", CMD_TIMEOUT.as_secs()),
        Ok((out, err, status)) => {
            let mut text = String::from_utf8_lossy(&out).into_owned();
            let err = String::from_utf8_lossy(&err);
            if !err.trim().is_empty() {
                text.push('\n');
                text.push_str(&err);
            }
            let mut text = text.trim().to_string();
            if text.len() > CMD_OUTPUT_CAP {
                let mut cut = text.len() - CMD_OUTPUT_CAP;
                while !text.is_char_boundary(cut) {
                    cut += 1;
                }
                text = format!("...[earlier output truncated]\n{}", &text[cut..]);
            }
            match status {
                Ok(s) if s.success() => if text.is_empty() { "(no output)".into() } else { text },
                Ok(s) => format!("{}\nexit code {}", if text.is_empty() { "(no output)" } else { &text }, s.code().unwrap_or(-1)),
                Err(e) => format!("{text}\nError: {e}"),
            }
        }
    }
}

/// True when a tool name is in the given mode's tool set; the UI must not be
/// able to run agent tools from Ask mode.
pub fn mode_allows(mode: &str, name: &str) -> bool {
    tools_for(mode, true).is_some_and(|tools| tools.iter().any(|t| t["function"]["name"] == name))
}

/// Directories agents should not wander into when listing for context.
pub fn is_skipped_dir(name: &str) -> bool {
    SKIP_DIRS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn file_tools_stay_in_root() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        assert_eq!(run_tool("write_file", r#"{"path":"src/a.txt","content":"hi there"}"#, root).await, "Wrote src/a.txt");
        assert_eq!(run_tool("read_file", r#"{"path":"src/a.txt"}"#, root).await, "hi there");
        assert!(run_tool("read_file", r#"{"path":"../../etc/passwd"}"#, root).await.starts_with("Error:"));
        assert!(run_tool("write_file", r#"{"path":"/tmp/x","content":"x"}"#, root).await.starts_with("Error:"));
        assert_eq!(run_tool("list_directory", r#"{"path":"."}"#, root).await, "src/");
        assert!(run_tool("search_files", r#"{"query":"THERE"}"#, root).await.contains("src/a.txt:1: hi there"));
        assert!(run_tool("read_file", "not json", root).await.starts_with("Error:"));
        assert!(run_tool("nope", "{}", root).await.contains("unknown tool"));
        assert!(run_tool("apply_diff", "{}", root).await.contains("handled by the IDE"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn command_runs_in_root_with_exit_code() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("marker.txt"), "").unwrap();
        let out = run_tool("run_command", r#"{"command":"ls; echo oops 1>&2; exit 3"}"#, d.path()).await;
        assert!(out.contains("marker.txt"), "{out}");
        assert!(out.contains("oops"));
        assert!(out.ends_with("exit code 3"));
    }

    #[test]
    fn tool_sets() {
        assert!(tools_for("ask", true).is_none());
        let names = |m, g| tools_for(m, g).unwrap().iter().map(|t| t["function"]["name"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        assert!(!names("plan", true).contains(&"run_command".to_string()));
        assert!(names("agent", true).contains(&"generate_image".to_string()));
        assert!(!names("agent", false).contains(&"generate_image".to_string()));
        assert!(mode_allows("agent", "run_command"));
        assert!(!mode_allows("plan", "write_file"));
        assert!(!mode_allows("ask", "read_file"));
    }
}
