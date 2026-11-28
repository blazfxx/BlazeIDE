//! BlBots: a roster of AI bots, each with its own name, personality
//! (`soul`), model, working folder, approval level, memory and routines.
//! Talk to one in its own threads, or put several in a room where a lead
//! hands work to the others. Bots run on BlazeAPI models with the same Rust
//! agent as the CLI, or on a local Claude Code / Codex CLI (`engines`).
//!
//! Design follows OpenMausBot's (Apache-2.0, see NOTICE); the code is our own.
//!
//! Storage (in the app's config folder, shared by Blaze IDE and `blaze`):
//!   blbots/bots.json                 the roster
//!   blbots/<bot id>/memory.md        what the bot chose to remember
//!   blbots/<bot id>/threads/*.json   its conversations (IDE chat format)
//!   blbots/<bot id>/home/            default working folder
//!   blbots/rooms.json, blbots/rooms/<room id>.json

pub mod computer;
pub mod engines;
pub mod room;
pub mod runner;
pub mod schedule;
pub mod tools;

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::agent::Approval;
use crate::error::{Error, Result};

pub const MAX_BOTS: usize = 50;
pub const MAX_NAME: usize = 60;
pub const MAX_SOUL: usize = 24_000;
pub const MAX_MEMORY: usize = 16_000;

/// What runs the bot.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Engine {
    /// A BlazeAPI (or custom provider) model through the Blaze agent.
    #[default]
    Blaze,
    /// The user's own Claude Code CLI (their subscription).
    Claude {
        #[serde(default)]
        bin: Option<String>,
    },
    /// The user's own OpenAI Codex CLI.
    Codex {
        #[serde(default)]
        bin: Option<String>,
    },
    /// The user's own Grok CLI.
    Grok {
        #[serde(default)]
        bin: Option<String>,
    },
}

impl Engine {
    pub fn label(&self) -> &'static str {
        match self {
            Engine::Blaze => "BlazeAPI",
            Engine::Claude { .. } => "Claude Code",
            Engine::Codex { .. } => "Codex",
            Engine::Grok { .. } => "Grok CLI",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Routine {
    pub id: String,
    pub title: String,
    /// What the bot is asked each time.
    pub prompt: String,
    pub schedule: schedule::Schedule,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Run once at start when a time was missed while nothing was running.
    #[serde(default)]
    pub catch_up: bool,
    /// Unix seconds of the last run (0 = never).
    #[serde(default)]
    pub last_run: i64,
    /// The thread routine runs post into.
    #[serde(default)]
    pub thread: Option<String>,
}

fn yes() -> bool {
    true
}

/// Drawn avatar bodies (`Bot::body`); "" = the default bot.
pub const BODIES: &[&str] = &["", "bot", "blob", "circle", "squircle", "pill", "drop", "bell", "hex", "diamond", "star"];
const MAX_AVATAR_IMAGE: usize = 300_000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Bot {
    pub id: String,
    pub name: String,
    /// One emoji or a letter shown as the avatar.
    #[serde(default)]
    pub avatar: String,
    /// Avatar background colour (#rrggbb).
    #[serde(default)]
    pub color: String,
    /// Drawn bot face in the IDE (1..=8; 0 = show `avatar` instead). Apps that
    /// don't draw faces keep showing `avatar`.
    #[serde(default)]
    pub face: u8,
    /// Body shape of the drawn avatar ("bot", "blob", "circle", ...).
    #[serde(default)]
    pub body: String,
    /// Frame behind the avatar: "" (none), "circle", "rounded" or "square".
    #[serde(default)]
    pub frame: String,
    /// Colour of the eyes and mouth: "" / "white" or "black".
    #[serde(default)]
    pub features: String,
    /// An uploaded or generated picture (small data: URL) used instead of the
    /// drawn avatar.
    #[serde(default)]
    pub image: String,
    /// One line under the name ("Writes and fixes tests").
    #[serde(default)]
    pub title: String,
    /// Standing instructions: who the bot is and how it works.
    #[serde(default)]
    pub soul: String,
    #[serde(default)]
    pub engine: Engine,
    /// BlazeAPI model id (Blaze engine) or the CLI's model name.
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub approval: Approval,
    /// Folder the bot works in; empty = its own home folder.
    #[serde(default)]
    pub folder: String,
    /// Agent mode (tools) or ask (talk only).
    #[serde(default = "agent_mode")]
    pub mode: String,
    #[serde(default)]
    pub routines: Vec<Routine>,
    /// One-off "check back later" turns: (unix seconds, thread id, note).
    #[serde(default)]
    pub check_backs: Vec<CheckBack>,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub created: i64,
    /// A private Linux desktop for this bot (Docker), see `computer`.
    #[serde(default)]
    pub computer: bool,
    /// May search and browse the web (see `crate::web`).
    #[serde(default = "yes")]
    pub web: bool,
    /// Skills from the library this bot has (see `crate::skills`).
    #[serde(default)]
    pub skills: Vec<String>,
}

fn agent_mode() -> String {
    "agent".into()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CheckBack {
    pub id: String,
    pub at: i64,
    pub thread: String,
    pub note: String,
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn one_line(s: &str) -> bool {
    !s.chars().any(|c| c.is_control() || c == '\u{2028}' || c == '\u{2029}')
}

impl Bot {
    pub fn new(name: &str) -> Self {
        Bot {
            id: format!("b{}", &uuid::Uuid::new_v4().simple().to_string()[..10]),
            name: name.trim().to_string(),
            avatar: name.trim().chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "B".into()),
            color: "#3a3a3a".into(),
            face: 0,
            body: String::new(),
            frame: String::new(),
            features: String::new(),
            image: String::new(),
            title: String::new(),
            soul: String::new(),
            engine: Engine::Blaze,
            model: String::new(),
            approval: Approval::Ask,
            folder: String::new(),
            mode: "agent".into(),
            routines: Vec::new(),
            check_backs: Vec::new(),
            pinned: false,
            hidden: false,
            created: now_secs(),
            computer: false,
            web: true,
            skills: Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        let name = self.name.trim();
        if name.is_empty() || name.chars().count() > MAX_NAME || !one_line(name) {
            return Err(Error::msg(format!("A bot needs a one-line name of up to {MAX_NAME} characters")));
        }
        if self.title.chars().count() > 200 || !one_line(&self.title) {
            return Err(Error::msg("The subtitle must be one line of up to 200 characters"));
        }
        if self.soul.len() > MAX_SOUL {
            return Err(Error::msg(format!("Instructions are limited to {} KB", MAX_SOUL / 1000)));
        }
        if self.skills.len() > crate::skills::MAX_SKILLS || self.skills.iter().any(|s| s.is_empty() || crate::skills::slug(s) != *s) {
            return Err(Error::msg("Unknown skill"));
        }
        if self.face > 8 {
            return Err(Error::msg("Unknown bot face"));
        }
        if !BODIES.contains(&self.body.as_str()) || !["", "none", "circle", "rounded", "square"].contains(&self.frame.as_str()) || !["", "white", "black"].contains(&self.features.as_str()) {
            return Err(Error::msg("Unknown avatar style"));
        }
        if !self.image.is_empty() {
            let ok_kind = ["data:image/png;base64,", "data:image/jpeg;base64,", "data:image/webp;base64,", "data:image/gif;base64,"].iter().any(|p| self.image.starts_with(p));
            if !ok_kind || self.image.len() > MAX_AVATAR_IMAGE || !self.image.split_once(',').map(|(_, b)| b.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'+' || c == b'/' || c == b'=')).unwrap_or(false) {
                return Err(Error::msg("The avatar picture must be a PNG, JPEG, WebP or GIF under 300 KB"));
            }
        }
        if self.avatar.chars().count() > 4 || !one_line(&self.avatar) {
            return Err(Error::msg("The avatar must be one emoji or letter"));
        }
        if !self.color.is_empty() && !(self.color.len() == 7 && self.color.starts_with('#') && self.color[1..].chars().all(|c| c.is_ascii_hexdigit())) {
            return Err(Error::msg("Colour must look like #3a7bd5"));
        }
        if !matches!(self.mode.as_str(), "agent" | "ask" | "plan") {
            return Err(Error::msg("Unknown mode"));
        }
        for r in &self.routines {
            if r.title.trim().is_empty() || r.prompt.trim().is_empty() {
                return Err(Error::msg("A routine needs a title and what to do"));
            }
            r.schedule.validate()?;
        }
        Ok(())
    }
}

/// The roster and everything that belongs to it.
pub struct Bots {
    dir: PathBuf,
}

#[derive(Default, Serialize, Deserialize)]
struct Roster {
    #[serde(default)]
    bots: Vec<Bot>,
}

impl Bots {
    /// `config_dir` = the app's config folder (same for IDE and CLI).
    pub fn open(config_dir: &Path) -> Self {
        Bots { dir: config_dir.join("blbots") }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn roster_path(&self) -> PathBuf {
        self.dir.join("bots.json")
    }

    fn read(&self) -> Roster {
        fs::read(self.roster_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    /// Change the roster under an exclusive lock (IDE and CLI may both run).
    fn mutate<R>(&self, f: impl FnOnce(&mut Vec<Bot>) -> Result<R>) -> Result<R> {
        fs::create_dir_all(&self.dir)?;
        let lock = fs::OpenOptions::new().create(true).truncate(false).write(true).open(self.dir.join("bots.lock"))?;
        lock.lock()?;
        let mut r = self.read();
        let out = f(&mut r.bots);
        if out.is_ok() {
            let tmp = self.roster_path().with_extension(format!("json.{}.tmp", std::process::id()));
            fs::write(&tmp, serde_json::to_vec_pretty(&r)?)?;
            fs::rename(&tmp, self.roster_path())?;
        }
        let _ = lock.unlock();
        out
    }

    pub fn list(&self) -> Vec<Bot> {
        let mut b = self.read().bots;
        b.sort_by(|a, b| b.pinned.cmp(&a.pinned).then(a.created.cmp(&b.created)));
        b
    }

    pub fn get(&self, id: &str) -> Option<Bot> {
        self.read().bots.into_iter().find(|b| b.id == id || b.name.eq_ignore_ascii_case(id))
    }

    /// Create or replace a bot (matched by id).
    pub fn save(&self, bot: Bot) -> Result<Bot> {
        bot.validate()?;
        let saved = self.mutate(|bots| {
            if bots.iter().any(|b| b.id != bot.id && b.name.eq_ignore_ascii_case(bot.name.trim())) {
                return Err(Error::msg(format!("There is already a bot called {}", bot.name.trim())));
            }
            match bots.iter_mut().find(|b| b.id == bot.id) {
                Some(slot) => *slot = bot.clone(),
                None => {
                    if bots.len() >= MAX_BOTS {
                        return Err(Error::msg(format!("You can have up to {MAX_BOTS} bots")));
                    }
                    bots.push(bot.clone());
                }
            }
            Ok(bot)
        })?;
        fs::create_dir_all(self.threads_dir(&saved.id))?;
        fs::create_dir_all(self.home(&saved.id))?;
        Ok(saved)
    }

    pub fn update(&self, id: &str, f: impl FnOnce(&mut Bot)) -> Result<Bot> {
        self.mutate(|bots| {
            let b = bots.iter_mut().find(|b| b.id == id).ok_or_else(|| Error::msg("No such bot"))?;
            f(b);
            b.validate()?;
            Ok(b.clone())
        })
    }

    /// Delete a bot and its threads and memory (its working folder is the
    /// user's and is left alone, unless it is the bot's own home).
    pub fn delete(&self, id: &str) -> Result<()> {
        self.mutate(|bots| {
            bots.retain(|b| b.id != id);
            Ok(())
        })?;
        let d = self.dir.join(safe_id(id)?);
        if d.starts_with(&self.dir) && d.exists() {
            fs::remove_dir_all(d)?;
        }
        Ok(())
    }

    pub fn duplicate(&self, id: &str) -> Result<Bot> {
        let src = self.get(id).ok_or_else(|| Error::msg("No such bot"))?;
        let mut copy = Bot { id: Bot::new(&src.name).id, created: now_secs(), check_backs: Vec::new(), ..src.clone() };
        let mut n = 2;
        let names: Vec<String> = self.list().into_iter().map(|b| b.name.to_lowercase()).collect();
        loop {
            let cand = format!("{} {n}", src.name);
            if !names.contains(&cand.to_lowercase()) {
                copy.name = cand;
                break;
            }
            n += 1;
        }
        for r in &mut copy.routines {
            r.last_run = 0;
            r.thread = None;
        }
        self.save(copy)
    }

    pub fn threads_dir(&self, id: &str) -> PathBuf {
        self.dir.join(id).join("threads")
    }

    pub fn home(&self, id: &str) -> PathBuf {
        self.dir.join(id).join("home")
    }

    /// The folder the bot's tools work in.
    pub fn workdir(&self, bot: &Bot) -> PathBuf {
        let f = bot.folder.trim();
        if f.is_empty() {
            self.home(&bot.id)
        } else {
            PathBuf::from(f)
        }
    }

    pub fn memory(&self, id: &str) -> String {
        fs::read_to_string(self.dir.join(id).join("memory.md")).unwrap_or_default()
    }

    pub fn set_memory(&self, id: &str, text: &str) -> Result<()> {
        safe_id(id)?;
        let mut t = text.to_string();
        if t.len() > MAX_MEMORY {
            // Keep the newest notes.
            let cut = t.len() - MAX_MEMORY;
            let at = t[cut..].find('\n').map(|i| cut + i + 1).unwrap_or(cut);
            t = t[at..].to_string();
        }
        fs::create_dir_all(self.dir.join(id))?;
        crate::files::write_text(&self.dir.join(id).join("memory.md"), &t)
    }

    pub fn remember(&self, id: &str, note: &str) -> Result<()> {
        let note = note.trim();
        if note.is_empty() {
            return Err(Error::msg("Nothing to remember"));
        }
        let mut m = self.memory(id);
        if !m.is_empty() && !m.ends_with('\n') {
            m.push('\n');
        }
        m.push_str(&format!("- {}\n", note.replace('\n', " ")));
        self.set_memory(id, &m)
    }
}

fn safe_id(id: &str) -> Result<&str> {
    if !id.is_empty() && id.len() <= 40 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        Ok(id)
    } else {
        Err(Error::msg("Invalid id"))
    }
}

/// The system text that makes a model this bot.
pub fn persona(bot: &Bot, memory: &str, roster: &[Bot], room: Option<&room::Room>) -> String {
    let mut s = format!("You are {}, one of the user's BlBots", bot.name.trim());
    if !bot.title.trim().is_empty() {
        s.push_str(&format!(" ({})", bot.title.trim()));
    }
    s.push_str(". Stay in character as described below, keep the user's goals first, and say plainly when you cannot do something.\n");
    if !bot.soul.trim().is_empty() {
        s.push_str("\n<instructions>\n");
        s.push_str(bot.soul.trim());
        s.push_str("\n</instructions>\n");
    }
    if !memory.trim().is_empty() {
        s.push_str("\n<memory>\nThings you chose to remember earlier (use the remember tool to add more):\n");
        s.push_str(memory.trim());
        s.push_str("\n</memory>\n");
    }
    let others: Vec<&Bot> = roster.iter().filter(|b| b.id != bot.id && !b.hidden).collect();
    if let Some(r) = room {
        s.push_str(&format!("\nYou are in the room \"{}\" with the user", r.name));
        let members: Vec<String> = r.members.iter().filter(|m| *m != &bot.id).filter_map(|m| roster.iter().find(|b| &b.id == m)).map(|b| format!("{} ({})", b.name, if b.title.is_empty() { b.engine.label() } else { b.title.as_str() })).collect();
        if !members.is_empty() {
            s.push_str(&format!(" and these bots: {}", members.join(", ")));
        }
        s.push_str(". Messages from other bots appear as \"[Name] text\". ");
        if r.lead.as_deref() == Some(bot.id.as_str()) {
            s.push_str("You lead this room: answer yourself when you can, and use hand_off to give a clear task to the bot best suited for it; you get its answer back.");
        }
        if !r.instructions.trim().is_empty() {
            s.push_str(&format!("\n<room_instructions>\n{}\n</room_instructions>", r.instructions.trim()));
        }
        s.push('\n');
    } else if !others.is_empty() {
        s.push_str(&format!("\nOther bots the user has: {}.\n", others.iter().map(|b| b.name.as_str()).collect::<Vec<_>>().join(", ")));
    }
    s
}

#[cfg(test)]
mod tests {
    #[test]
    fn avatar_style_is_checked() {
        let mut b = Bot::new("Nova");
        b.face = 3; b.body = "hex".into(); b.frame = "circle".into(); b.features = "black".into();
        b.image = "data:image/png;base64,iVBORw0KGgo=".into();
        assert!(b.validate().is_ok());
        for bad in [("body", "triangle"), ("frame", "oval"), ("features", "red")] {
            let mut x = b.clone();
            match bad.0 { "body" => x.body = bad.1.into(), "frame" => x.frame = bad.1.into(), _ => x.features = bad.1.into() }
            assert!(x.validate().is_err(), "{bad:?}");
        }
        let mut x = b.clone();
        x.image = "data:text/html;base64,PHNjcmlwdD4=".into();
        assert!(x.validate().is_err());
        x.image = "data:image/png;base64,<script>".into();
        assert!(x.validate().is_err());
        x.image = format!("data:image/png;base64,{}", "A".repeat(400_000));
        assert!(x.validate().is_err());
        x.face = 9; x.image.clear();
        assert!(x.validate().is_err());
    }

    use super::*;

    #[test]
    fn roster_crud_and_limits() {
        let d = tempfile::tempdir().unwrap();
        let bots = Bots::open(d.path());
        let mut a = Bot::new("Nova");
        a.soul = "You write tests.".into();
        let a = bots.save(a).unwrap();
        assert!(bots.save(Bot::new("nova")).is_err(), "names are unique");
        assert!(bots.save(Bot::new("Bad\nname")).is_err());
        let mut big = Bot::new("Big");
        big.soul = "x".repeat(MAX_SOUL + 1);
        assert!(bots.save(big).is_err());
        let b = bots.duplicate(&a.id).unwrap();
        assert_eq!(b.name, "Nova 2");
        assert_ne!(b.id, a.id);
        assert_eq!(bots.list().len(), 2);
        bots.update(&b.id, |x| x.pinned = true).unwrap();
        assert_eq!(bots.list()[0].id, b.id, "pinned first");
        assert!(bots.get("NOVA").is_some(), "lookup by name");
        bots.remember(&a.id, "User prefers pytest").unwrap();
        bots.remember(&a.id, "Repo uses uv").unwrap();
        assert!(bots.memory(&a.id).contains("- Repo uses uv"));
        bots.set_memory(&a.id, &"line\n".repeat(10_000)).unwrap();
        assert!(bots.memory(&a.id).len() <= MAX_MEMORY);
        assert!(bots.threads_dir(&a.id).is_dir());
        bots.delete(&a.id).unwrap();
        assert!(bots.get(&a.id).is_none());
        assert!(!d.path().join("blbots").join(&a.id).exists());
        assert!(bots.delete("../x").is_err());
    }

    #[test]
    fn persona_text() {
        let mut a = Bot::new("Nova");
        a.title = "Tester".into();
        a.soul = "Be brief.".into();
        let b = Bot::new("Orbit");
        let p = persona(&a, "- likes tea", &[a.clone(), b.clone()], None);
        assert!(p.starts_with("You are Nova, one of the user's BlBots (Tester)"));
        assert!(p.contains("Be brief.") && p.contains("likes tea") && p.contains("Orbit"));
    }
}
