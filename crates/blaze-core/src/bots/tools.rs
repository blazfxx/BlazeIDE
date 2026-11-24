//! Tools every bot gets on top of the agent's file/command tools:
//! remember, check_back, and (lead of a room) hand_off.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::{json, Map, Value};

use super::{now_secs, Bots, CheckBack};
use crate::agent::{Approval, ExtraTools, ToolOutput};
use crate::ai::ToolCall;

pub type HandOff = Arc<dyn Fn(String, String) -> Pin<Box<dyn Future<Output = String> + Send>> + Send + Sync>;

pub struct BotTools {
    pub bots: Arc<Bots>,
    pub bot_id: String,
    pub thread_id: Option<String>,
    /// Set for the lead of a room: run another member with a task.
    pub hand_off: Option<HandOff>,
    /// Extra tools from the bot's computer (private desktop).
    pub computer: Option<Arc<dyn ExtraTools>>,
}

const MAX_CHECK_BACKS: usize = 10;

fn schema(name: &str, desc: &str, props: Value, req: &[&str]) -> Value {
    json!({ "type": "function", "function": { "name": name, "description": desc, "parameters": { "type": "object", "properties": props, "required": req } } })
}

impl ExtraTools for BotTools {
    fn schemas(&self) -> Vec<Value> {
        let mut v = vec![schema(
            "remember",
            "Save a short note to your long-term memory (facts about the user, their preferences, project decisions). It is shown to you in every future conversation. Keep notes short; do not store secrets.",
            json!({ "note": { "type": "string", "description": "One short fact to remember" } }),
            &["note"],
        )];
        if self.thread_id.is_some() {
            v.push(schema(
                "check_back",
                "Come back to this conversation later by yourself, e.g. to check whether a build finished or to remind the user. At that time you get a turn with your note. Use minutes from now (1 to 129600).",
                json!({
                    "in_minutes": { "type": "integer", "description": "Minutes from now" },
                    "note": { "type": "string", "description": "What to check or do then" }
                }),
                &["in_minutes", "note"],
            ));
        }
        if self.hand_off.is_some() {
            v.push(schema(
                "hand_off",
                "Give a task to another bot in this room and get its answer back. Pick the bot whose skills fit; write a complete, self-contained task.",
                json!({
                    "bot": { "type": "string", "description": "The other bot's name" },
                    "task": { "type": "string", "description": "What it should do, with all needed context" }
                }),
                &["bot", "task"],
            ));
        }
        if let Some(c) = &self.computer {
            v.extend(c.schemas());
        }
        v
    }

    fn handles(&self, name: &str) -> bool {
        matches!(name, "remember" | "check_back" | "hand_off") || self.computer.as_ref().is_some_and(|c| c.handles(name))
    }

    fn needs_approval(&self, level: Approval, name: &str) -> bool {
        match &self.computer {
            Some(c) if c.handles(name) => c.needs_approval(level, name),
            _ => false,
        }
    }

    fn run<'a>(&'a self, call: &'a ToolCall) -> Pin<Box<dyn Future<Output = ToolOutput> + Send + 'a>> {
        Box::pin(async move {
            if let Some(c) = &self.computer {
                if c.handles(&call.name) {
                    return c.run(call).await;
                }
            }
            let args: Map<String, Value> = serde_json::from_str(&call.arguments).unwrap_or_default();
            let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
            let text = match call.name.as_str() {
                "remember" => match self.bots.remember(&self.bot_id, &s("note")) {
                    Ok(()) => "Saved to memory.".to_string(),
                    Err(e) => format!("Error: {e}"),
                },
                "check_back" => {
                    let mins = args.get("in_minutes").and_then(Value::as_i64).unwrap_or(0);
                    let note = s("note");
                    match (&self.thread_id, (1..=129_600).contains(&mins), note.is_empty()) {
                        (None, _, _) => "Error: check_back works only in a conversation.".into(),
                        (_, false, _) => "Error: in_minutes must be between 1 and 129600 (90 days).".into(),
                        (_, _, true) => "Error: say what to check.".into(),
                        (Some(thread), true, false) => {
                            let at = now_secs() + mins * 60;
                            let cb = CheckBack { id: format!("k{}", &uuid::Uuid::new_v4().simple().to_string()[..8]), at, thread: thread.clone(), note: note.chars().take(500).collect() };
                            match self.bots.update(&self.bot_id, |b| {
                                if b.check_backs.len() < MAX_CHECK_BACKS {
                                    b.check_backs.push(cb);
                                }
                            }) {
                                Ok(b) if b.check_backs.iter().any(|c| c.at == at) => {
                                    let when = super::schedule::zoned(at).map(|z| z.strftime("%a %H:%M").to_string()).unwrap_or_default();
                                    format!("I will check back {when} (in {mins} min). It runs only while Blaze IDE or `blaze serve` is running on this computer.")
                                }
                                Ok(_) => format!("Error: you already have {MAX_CHECK_BACKS} check-backs waiting."),
                                Err(e) => format!("Error: {e}"),
                            }
                        }
                    }
                }
                "hand_off" => match &self.hand_off {
                    Some(h) => {
                        let (bot, task) = (s("bot"), s("task"));
                        if bot.is_empty() || task.is_empty() {
                            "Error: name a bot and a task.".into()
                        } else {
                            h(bot, task).await
                        }
                    }
                    None => "Error: hand_off is only for the lead of a room.".into(),
                },
                other => format!("Error: unknown tool {other}"),
            };
            text.into()
        })
    }
}
