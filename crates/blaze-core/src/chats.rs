//! Saved AI conversations: one JSON file per chat under
//! `<project>/.blazeide/chats/<id>.json`. The id comes from the UI, so it is
//! validated to a safe filename before it ever touches a path.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::error::{Error, Result};
use crate::files::write_text;

const MAX_CHAT_BYTES: usize = 20 * 1024 * 1024;

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChatSummary {
    pub id: String,
    pub title: String,
    pub updated_at: f64,
    pub message_count: usize,
}

fn chats_dir(root: &Path) -> PathBuf {
    root.join(".blazeide").join("chats")
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn chat_path(root: &Path, id: &str) -> Result<PathBuf> {
    if !valid_id(id) {
        return Err(Error::msg("Invalid chat id"));
    }
    Ok(chats_dir(root).join(format!("{id}.json")))
}

pub fn list(root: &Path) -> Vec<ChatSummary> {
    let Ok(rd) = fs::read_dir(chats_dir(root)) else { return Vec::new() };
    let mut out: Vec<ChatSummary> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| {
            let v: Value = serde_json::from_slice(&fs::read(e.path()).ok()?).ok()?;
            Some(ChatSummary {
                id: v.get("id")?.as_str()?.to_string(),
                title: v.get("title").and_then(Value::as_str).unwrap_or("Untitled").to_string(),
                updated_at: v.get("updatedAt").and_then(Value::as_f64).unwrap_or(0.0),
                message_count: v.get("messages").and_then(Value::as_array).map_or(0, Vec::len),
            })
        })
        .collect();
    out.sort_by(|a, b| b.updated_at.total_cmp(&a.updated_at));
    out
}

pub fn load(root: &Path, id: &str) -> Result<Option<Value>> {
    let p = chat_path(root, id)?;
    let Ok(bytes) = fs::read(p) else { return Ok(None) };
    Ok(serde_json::from_slice(&bytes).ok())
}

pub fn save(root: &Path, chat: &Value) -> Result<()> {
    let id = chat.get("id").and_then(Value::as_str).ok_or_else(|| Error::msg("Chat has no id"))?;
    let p = chat_path(root, id)?;
    let body = serde_json::to_string(chat)?;
    if body.len() > MAX_CHAT_BYTES {
        return Err(Error::msg("This chat is too large to save"));
    }
    write_text(&p, &body)
}

pub fn delete(root: &Path, id: &str) -> Result<()> {
    let p = chat_path(root, id)?;
    match fs::remove_file(p) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn roundtrip_and_order() {
        let d = tempfile::tempdir().unwrap();
        save(d.path(), &json!({"id":"a1","title":"First","updatedAt":1.0,"messages":[1,2]})).unwrap();
        save(d.path(), &json!({"id":"b2","title":"Second","updatedAt":5.0,"messages":[]})).unwrap();
        let l = list(d.path());
        assert_eq!(l.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), vec!["b2", "a1"]);
        assert_eq!(l[1].message_count, 2);
        assert_eq!(load(d.path(), "a1").unwrap().unwrap()["title"], "First");
        assert!(load(d.path(), "zzz").unwrap().is_none());
        delete(d.path(), "a1").unwrap();
        assert_eq!(list(d.path()).len(), 1);
    }

    #[test]
    fn rejects_traversal_ids() {
        let d = tempfile::tempdir().unwrap();
        assert!(save(d.path(), &json!({"id":"../../evil"})).is_err());
        assert!(load(d.path(), "../x").is_err());
        assert!(delete(d.path(), "a/b").is_err());
        assert!(save(d.path(), &json!({"title":"no id"})).is_err());
    }
}
