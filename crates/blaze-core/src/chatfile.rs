//! A chat file in the IDE's format (`StoredChat` in src/shared/types.ts):
//! `<project>/.blazeide/chats/<id>.json`. Used by the CLI, phone turns and
//! BlBots so any of them, and the IDE, can continue any chat.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::agent::{self, FileBefore};
use crate::ai::ChatMessage;

pub const DEFAULT_MODEL: &str = "claude-sonnet-5";

/// A chat saved in `<project>/.blazeide/chats/<id>.json`, shared with the IDE.
pub struct ChatFile {
    /// Working folder for the agent's tools.
    pub root: PathBuf,
    /// Where the file is kept (the project's .blazeide/chats, or a bot's threads folder).
    pub dir: PathBuf,
    /// Extra fields kept in the file (e.g. `bot`), never interpreted by the IDE.
    pub extra: serde_json::Map<String, Value>,
    pub id: String,
    pub title: String,
    pub created_at: f64,
    pub mode: String,
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub checkpoints: Vec<Value>,
}

impl ChatFile {
    pub fn new(root: &Path, mode: &str, model: &str) -> Self {
        ChatFile {
            root: root.to_path_buf(),
            dir: crate::chats::dir_for(root),
            extra: serde_json::Map::new(),
            id: format!("c{}", &uuid::Uuid::new_v4().simple().to_string()[..12]),
            title: "New chat".into(),
            created_at: agent::now_ms(),
            mode: mode.into(),
            model: model.into(),
            messages: Vec::new(),
            checkpoints: Vec::new(),
        }
    }

    pub fn load(root: &Path, id: &str) -> Result<Self, String> {
        Self::load_from(root, &crate::chats::dir_for(root), id)
    }

    /// A chat kept in `dir` whose tools work in `root`.
    pub fn load_from(root: &Path, dir: &Path, id: &str) -> Result<Self, String> {
        let v = crate::chats::load_in(dir, id).map_err(|e| e.to_string())?.ok_or_else(|| format!("No chat {id} here"))?;
        let known = ["id", "title", "createdAt", "updatedAt", "mode", "model", "messages", "checkpoints"];
        let extra = v.as_object().map(|o| o.iter().filter(|(k, _)| !known.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default();
        Ok(ChatFile {
            root: root.to_path_buf(),
            dir: dir.to_path_buf(),
            extra,
            id: id.to_string(),
            title: v.get("title").and_then(Value::as_str).unwrap_or("Untitled").to_string(),
            created_at: v.get("createdAt").and_then(Value::as_f64).unwrap_or_else(agent::now_ms),
            mode: v.get("mode").and_then(Value::as_str).unwrap_or("agent").to_string(),
            model: v.get("model").and_then(Value::as_str).unwrap_or(DEFAULT_MODEL).to_string(),
            messages: v.get("messages").cloned().and_then(|m| serde_json::from_value(m).ok()).unwrap_or_default(),
            checkpoints: v.get("checkpoints").and_then(Value::as_array).cloned().unwrap_or_default(),
        })
    }

    pub fn add_checkpoint(&mut self, user_index: usize, changed: &[FileBefore]) {
        if let Some(c) = agent::checkpoint(user_index, changed) {
            self.checkpoints.push(c);
        }
    }

    pub fn save(&mut self) -> Result<(), String> {
        if self.messages.is_empty() {
            return Ok(());
        }
        if self.title == "New chat" {
            if let Some(first) = self.messages.iter().find(|m| m.role == "user") {
                self.title = agent::title_from(&first.content);
            }
        }
        let mut v = agent::stored_chat(&self.id, &self.title, self.created_at, &self.mode, &self.model, &self.messages, &self.checkpoints);
        if let Some(o) = v.as_object_mut() {
            for (k, x) in &self.extra {
                o.entry(k.clone()).or_insert_with(|| x.clone());
            }
        }
        crate::chats::save_in(&self.dir, &v).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_in_the_ide_format() {
        let d = tempfile::tempdir().unwrap();
        let mut c = ChatFile::new(d.path(), "agent", "glm-5.3");
        c.messages.push(ChatMessage { role: "user".into(), content: "Fix the login form\nplease".into(), ..Default::default() });
        c.messages.push(ChatMessage { role: "assistant".into(), content: "Done.".into(), ..Default::default() });
        c.add_checkpoint(0, &[FileBefore { rel: "a.ts".into(), before: None }]);
        c.save().unwrap();
        let raw = crate::chats::load(d.path(), &c.id).unwrap().unwrap();
        for k in ["id", "title", "createdAt", "updatedAt", "mode", "model", "messages", "checkpoints"] {
            assert!(raw.get(k).is_some(), "missing {k}");
        }
        assert_eq!(raw["checkpoints"][0]["files"][0]["before"], Value::Null);
        let back = ChatFile::load(d.path(), &c.id).unwrap();
        assert_eq!(back.title, "Fix the login form");
        assert_eq!(back.messages.len(), 2);
        assert_eq!(back.model, "glm-5.3");
        let mut empty = ChatFile::new(d.path(), "ask", "m");
        empty.save().unwrap();
        assert!(ChatFile::load(d.path(), &empty.id).is_err());
    }
}
