//! The `use_skill` tool, and adding skills to an agent turn.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::{json, Map, Value};

use super::{index, Library, Skill};
use crate::agent::{AgentConfig, Approval, ExtraTools, ToolOutput};
use crate::ai::ToolCall;

pub const NAME: &str = "use_skill";

pub fn schema() -> Value {
    json!({ "type": "function", "function": {
        "name": NAME,
        "description": "Open one of your skills to read its instructions. Call it with just the name first; if the instructions point to another file of the skill, call it again with that file.",
        "parameters": { "type": "object", "properties": {
            "name": { "type": "string", "description": "The skill's name, from your skill list" },
            "file": { "type": "string", "description": "Optional: a file that belongs to the skill, e.g. reference/api.md" }
        }, "required": ["name"] }
    } })
}

/// Answer a use_skill call. `allowed` = the skills this chat or bot has.
pub fn run(lib: &Library, allowed: &[String], arguments: &str) -> String {
    let args: Map<String, Value> = serde_json::from_str(arguments).unwrap_or_default();
    let arg = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
    let (name, file) = (arg("name"), arg("file"));
    if !allowed.contains(&name) {
        return format!("Error: you have no skill called \"{name}\". Your skills: {}", allowed.join(", "));
    }
    if !file.is_empty() && file != "SKILL.md" {
        return lib.file(&name, &file).unwrap_or_else(|e| format!("Error: {e}"));
    }
    let text = match lib.text(&name) {
        Ok(t) => t,
        Err(e) => return format!("Error: {e}"),
    };
    let others: Vec<String> = lib.files(&name).into_iter().filter(|f| f != "SKILL.md").collect();
    let mut out = format!("<skill name=\"{name}\">\n{}\n</skill>", super::parse(&text).2.trim());
    if !others.is_empty() {
        let folder = lib.folder(&name).map(|p| p.display().to_string()).unwrap_or_default();
        out.push_str(&format!(
            "\nFiles of this skill (read one with use_skill and \"file\"): {}\nThe skill's folder is {folder} — to run one of its scripts, use that full path.",
            others.join(", ")
        ));
    }
    out
}

/// Skills as extra tools of an agent turn, in front of any others.
pub struct SkillTools {
    lib: Library,
    allowed: Vec<String>,
    inner: Option<Arc<dyn ExtraTools>>,
}

impl ExtraTools for SkillTools {
    fn schemas(&self) -> Vec<Value> {
        let mut v = vec![schema()];
        if let Some(i) = &self.inner {
            v.extend(i.schemas());
        }
        v
    }
    fn handles(&self, name: &str) -> bool {
        name == NAME || self.inner.as_ref().is_some_and(|i| i.handles(name))
    }
    fn needs_approval(&self, level: Approval, name: &str) -> bool {
        // Reading a skill the user added never asks.
        name != NAME && self.inner.as_ref().is_some_and(|i| i.needs_approval(level, name))
    }
    fn run<'a>(&'a self, call: &'a ToolCall) -> Pin<Box<dyn Future<Output = ToolOutput> + Send + 'a>> {
        match &self.inner {
            Some(i) if call.name != NAME => i.run(call),
            _ => Box::pin(async move { run(&self.lib, &self.allowed, &call.arguments).into() }),
        }
    }
}

fn add(cfg: &mut AgentConfig, config_dir: &Path, skills: Vec<Skill>) {
    // Ask mode has no tools, so a list of skills would be no use there.
    if cfg.mode == "ask" {
        return;
    }
    let Some(list) = index(&skills) else { return };
    cfg.extra_system = Some(match cfg.extra_system.take() {
        Some(s) => format!("{s}\n\n{list}"),
        None => list,
    });
    let allowed = skills.into_iter().map(|s| s.name).collect();
    cfg.extra_tools = Some(Arc::new(SkillTools { lib: Library::open(config_dir), allowed, inner: cfg.extra_tools.take() }));
}

/// The skills switched on for chat (IDE, CLI, phone).
pub fn for_chat(cfg: &mut AgentConfig, config_dir: &Path) {
    let skills = Library::open(config_dir).list().into_iter().filter(|s| s.chat).collect();
    add(cfg, config_dir, skills);
}

/// The skills a bot was given.
pub fn for_bot(cfg: &mut AgentConfig, config_dir: &Path, names: &[String]) {
    if names.is_empty() {
        return;
    }
    let skills = Library::open(config_dir).list().into_iter().filter(|s| names.contains(&s.name)).collect();
    add(cfg, config_dir, skills);
}
