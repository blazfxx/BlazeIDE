//! Rooms: one conversation with the user and several bots. A message that
//! @mentions bots goes to them, in order; otherwise the lead answers and may
//! hand work to the others (`hand_off`).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RoomMsg {
    /// "user" or a bot id.
    pub author: String,
    pub text: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Room {
    pub id: String,
    pub name: String,
    pub members: Vec<String>,
    #[serde(default)]
    pub lead: Option<String>,
    #[serde(default)]
    pub instructions: String,
    /// Shared working folder ("" = the lead's own).
    #[serde(default)]
    pub folder: String,
    #[serde(default)]
    pub messages: Vec<RoomMsg>,
    #[serde(default)]
    pub created: i64,
    #[serde(default)]
    pub updated: i64,
}

impl Room {
    pub fn new(name: &str, members: Vec<String>) -> Self {
        let lead = members.first().cloned();
        let now = super::now_secs();
        Room { id: format!("r{}", &uuid::Uuid::new_v4().simple().to_string()[..10]), name: name.trim().to_string(), members, lead, instructions: String::new(), folder: String::new(), messages: Vec::new(), created: now, updated: now }
    }

    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() || self.name.chars().count() > 80 || self.name.contains('\n') {
            return Err(Error::msg("A room needs a one-line name"));
        }
        if self.members.is_empty() || self.members.len() > 12 {
            return Err(Error::msg("A room has 1 to 12 bots"));
        }
        if let Some(l) = &self.lead {
            if !self.members.contains(l) {
                return Err(Error::msg("The lead must be a member"));
            }
        }
        if self.instructions.len() > 8000 {
            return Err(Error::msg("Room instructions are limited to 8 KB"));
        }
        Ok(())
    }
}

/// Which bots a message is for: the @mentioned ones (by name, in order of
/// appearance), else the lead.
pub fn targets(text: &str, room: &Room, names: &[(String, String)]) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut hits: Vec<(usize, String)> = names
        .iter()
        .filter(|(id, _)| room.members.contains(id))
        .filter_map(|(id, name)| {
            let tag = format!("@{}", name.to_lowercase());
            lower.find(&tag).filter(|i| {
                // whole name: next char is not a letter/digit
                lower[i + tag.len()..].chars().next().is_none_or(|c| !c.is_alphanumeric())
            }).map(|i| (i, id.clone()))
        })
        .collect();
    hits.sort();
    hits.dedup_by(|a, b| a.1 == b.1);
    if hits.is_empty() {
        room.lead.clone().or_else(|| room.members.first().cloned()).into_iter().collect()
    } else {
        hits.into_iter().map(|(_, id)| id).collect()
    }
}

pub struct Rooms {
    dir: PathBuf,
}

impl Rooms {
    pub fn open(config_dir: &Path) -> Self {
        Rooms { dir: config_dir.join("blbots").join("rooms") }
    }

    fn path(&self, id: &str) -> Result<PathBuf> {
        if id.is_empty() || id.len() > 40 || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(Error::msg("Invalid room id"));
        }
        Ok(self.dir.join(format!("{id}.json")))
    }

    pub fn list(&self) -> Vec<Room> {
        let Ok(rd) = fs::read_dir(&self.dir) else { return Vec::new() };
        let mut v: Vec<Room> = rd.flatten().filter_map(|e| serde_json::from_slice(&fs::read(e.path()).ok()?).ok()).collect();
        v.sort_by_key(|r| std::cmp::Reverse(r.updated));
        v
    }

    pub fn get(&self, id: &str) -> Option<Room> {
        serde_json::from_slice(&fs::read(self.path(id).ok()?).ok()?).ok()
    }

    pub fn save(&self, room: &Room) -> Result<()> {
        room.validate()?;
        fs::create_dir_all(&self.dir)?;
        let body = serde_json::to_string(room)?;
        if body.len() > 20 * 1024 * 1024 {
            return Err(Error::msg("This room's history is too large; start a new room"));
        }
        crate::files::write_text(&self.path(&room.id)?, &body)
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        match fs::remove_file(self.path(id)?) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mention_routing() {
        let names = vec![("b1".to_string(), "Nova".to_string()), ("b2".to_string(), "Orbit".to_string()), ("b3".to_string(), "Novak".to_string())];
        let mut r = Room::new("Team", vec!["b1".into(), "b2".into(), "b3".into()]);
        r.lead = Some("b2".into());
        assert_eq!(targets("what do you think?", &r, &names), vec!["b2"]);
        assert_eq!(targets("@nova and @Orbit please", &r, &names), vec!["b1", "b2"]);
        assert_eq!(targets("@Novak only", &r, &names), vec!["b3"], "@Nova must not match @Novak");
        assert_eq!(targets("@Orbit then @nova", &r, &names), vec!["b2", "b1"]);
    }

    #[test]
    fn store() {
        let d = tempfile::tempdir().unwrap();
        let rooms = Rooms::open(d.path());
        let mut r = Room::new("Launch", vec!["b1".into()]);
        r.messages.push(RoomMsg { author: "user".into(), text: "hi".into(), tools: vec![], at: 1 });
        rooms.save(&r).unwrap();
        assert_eq!(rooms.get(&r.id).unwrap().messages.len(), 1);
        assert_eq!(rooms.list().len(), 1);
        assert!(rooms.get("../x").is_none());
        let mut bad = r.clone();
        bad.lead = Some("zz".into());
        assert!(rooms.save(&bad).is_err());
        rooms.delete(&r.id).unwrap();
        assert!(rooms.list().is_empty());
    }
}
