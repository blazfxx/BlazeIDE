//! Skills: folders with a `SKILL.md` (name + description frontmatter, then
//! instructions, optionally with extra files). The same format Claude Code,
//! Codex and OpenCode use, so theirs import as they are.
//!
//! The library lives in the app's config folder (shared by Blaze IDE and
//! `blaze`): `skills/<name>/SKILL.md`, plus `skills/library.json`.
//!
//! A model only ever sees one line per skill ([`index`]); it opens a skill
//! with the `use_skill` tool when the task calls for it.

mod starters;
pub mod tool;

pub use starters::STARTERS;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

pub const MAX_SKILLS: usize = 400;
pub const MAX_NAME: usize = 64;
const MAX_DESCRIPTION: usize = 1000;
/// One SKILL.md (or one file a model reads).
pub const MAX_TEXT: u64 = 200_000;
const MAX_FILE: u64 = 2_000_000;
const MAX_SKILL_BYTES: u64 = 12_000_000;
const MAX_SKILL_FILES: usize = 200;
/// The list of skills sent with every message stays under this.
const INDEX_CAP: usize = 12_000;
const INDEX_DESC: usize = 240;

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// Where it came from ("Claude Code", "Starter", "You"...).
    pub source: String,
    /// Offered to the IDE chat, the CLI and phone chats. Bots choose their own.
    pub chat: bool,
    pub files: usize,
    pub bytes: u64,
}

/// A skill found on this computer that can be imported.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    pub name: String,
    pub description: String,
    pub source: String,
    /// The skill's folder, or the single .md file of a slash command.
    pub path: String,
    /// "new", "same" (already in the library) or "differs".
    pub status: String,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct State {
    chat_off: BTreeSet<String>,
    origin: BTreeMap<String, String>,
}

/// A safe folder name: lower case letters, digits, "-" and "_".
pub fn slug(s: &str) -> String {
    let mut out = String::new();
    for c in s.trim().chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let mut out = out.trim_matches('-').to_string();
    out.truncate(MAX_NAME);
    out.trim_end_matches('-').to_string()
}

fn valid_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= MAX_NAME && slug(s) == s && !is_reserved(s)
}

/// Names Windows cannot use as folders, and the library's own file.
fn is_reserved(s: &str) -> bool {
    matches!(s, "con" | "prn" | "aux" | "nul" | "library") || (s.len() == 4 && (s.starts_with("com") || s.starts_with("lpt")) && s.ends_with(|c: char| c.is_ascii_digit()))
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        return serde_json::from_str::<String>(v).unwrap_or_else(|_| v[1..v.len() - 1].to_string());
    }
    if v.len() >= 2 && v.starts_with('\'') && v.ends_with('\'') {
        return v[1..v.len() - 1].replace("''", "'");
    }
    v.to_string()
}

/// `name` and `description` from the YAML frontmatter, and the text after it.
/// Only the two scalar fields are read (plain, quoted, folded or block).
pub fn parse(text: &str) -> (Option<String>, Option<String>, &str) {
    let t = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(rest) = t.strip_prefix("---").and_then(|r| r.strip_prefix('\n').or_else(|| r.strip_prefix("\r\n"))) else {
        return (None, None, t);
    };
    let mut end = None;
    let mut at = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end() == "---" {
            end = Some((at, at + line.len()));
            break;
        }
        at += line.len();
    }
    let Some((head_end, body_start)) = end else { return (None, None, t) };
    let lines: Vec<&str> = rest[..head_end].lines().collect();
    let field = |key: &str| -> Option<String> {
        let i = lines.iter().position(|l| l.strip_prefix(key).is_some_and(|r| r.starts_with(':')))?;
        let first = lines[i][key.len() + 1..].trim();
        let more = lines[i + 1..].iter().take_while(|l| l.starts_with(' ') || l.starts_with('\t') || l.trim().is_empty()).map(|l| l.trim());
        let block = matches!(first, "" | ">" | ">-" | ">+" | "|" | "|-" | "|+");
        let mut parts: Vec<&str> = if block { Vec::new() } else { vec![first] };
        parts.extend(more.filter(|l| !l.is_empty()));
        let v = unquote(&parts.join(" "));
        (!v.is_empty()).then_some(v)
    };
    (field("name"), field("description"), &rest[body_start..])
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>().trim_end())
    }
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A description for a file that has none: its first line of prose.
fn first_line(body: &str) -> String {
    body.lines().map(|l| l.trim().trim_start_matches('#').trim()).find(|l| !l.is_empty()).unwrap_or("").to_string()
}

fn describe(text: &str) -> (Option<String>, String) {
    let (name, desc, body) = parse(text);
    let desc = desc.unwrap_or_else(|| first_line(body));
    (name, clip(&one_line(&desc), MAX_DESCRIPTION))
}

/// A SKILL.md from its parts.
pub fn compose(name: &str, description: &str, body: &str) -> String {
    format!("---\nname: {name}\ndescription: {}\n---\n\n{}\n", serde_json::to_string(&one_line(description)).unwrap_or_default(), body.trim())
}

/// Files that must not travel with a skill (keys, version control, installs).
fn skipped(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with('.') || matches!(n.as_str(), "node_modules" | "__pycache__" | "venv" | "target" | "dist-info") || n.ends_with(".pem") || n.ends_with(".key") || n.ends_with(".p12") || n == "id_rsa" || n == "id_ed25519" || n == "credentials.json"
}

/// The files of a skill folder (relative path, size). Links inside the folder
/// are not followed: they could point anywhere on the computer.
fn files_of(dir: &Path) -> Vec<(String, u64)> {
    fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Vec<(String, u64)>, total: &mut u64) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = entries.filter_map(|e| e.ok()).collect();
        // SKILL.md first, so the caps can never drop it.
        entries.sort_by_key(|e| (e.file_name() != "SKILL.md", e.file_name()));
        for e in entries {
            if out.len() >= MAX_SKILL_FILES {
                return;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            let Ok(kind) = e.file_type() else { continue };
            if skipped(&name) || kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                if depth < 6 {
                    walk(root, &e.path(), depth + 1, out, total);
                }
            } else if let Ok(meta) = e.metadata() {
                if meta.len() > MAX_FILE || *total + meta.len() > MAX_SKILL_BYTES {
                    continue;
                }
                *total += meta.len();
                if let Ok(rel) = e.path().strip_prefix(root) {
                    out.push((rel.to_string_lossy().replace('\\', "/"), meta.len()));
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, 0, &mut out, &mut 0);
    out
}

fn read_capped(p: &Path, max: u64) -> Option<String> {
    let meta = fs::metadata(p).ok()?;
    if !meta.is_file() || meta.len() > max {
        return None;
    }
    String::from_utf8(fs::read(p).ok()?).ok()
}

/// What an imported path would put in SKILL.md, with its name and description.
fn candidate(path: &Path) -> Option<(String, String, String)> {
    let is_dir = path.is_dir();
    let file = if is_dir { path.join("SKILL.md") } else { path.to_path_buf() };
    let text = read_capped(&file, MAX_TEXT)?;
    let (named, description) = describe(&text);
    let stem = if is_dir { path.file_name()? } else { path.file_stem()? }.to_string_lossy().into_owned();
    let name = [named.unwrap_or_default(), stem].iter().map(|s| slug(s)).find(|s| valid_name(s))?;
    if is_dir {
        return Some((name, description, text));
    }
    // A slash command or prompt file becomes a skill of its own.
    let (_, _, body) = parse(&text);
    let description = if description.is_empty() { format!("The /{name} command") } else { description };
    let mut body = body.trim().to_string();
    if body.contains("$ARGUMENTS") || body.contains("$1") {
        body = format!("(This was a slash command. Where it says $ARGUMENTS or $1, use what the user asked for.)\n\n{body}");
    }
    let md = compose(&name, &description, &body);
    Some((name, description, md))
}

/// Skill folders under `root` (a folder with SKILL.md is not searched deeper).
/// A linked skill folder is followed: tools install skills that way.
fn skill_dirs(root: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if out.len() >= MAX_SKILLS * 3 {
        return;
    }
    if root.join("SKILL.md").is_file() {
        out.push(root.to_path_buf());
        return;
    }
    if depth == 0 {
        return;
    }
    let Ok(entries) = fs::read_dir(root) else { return };
    let mut dirs: Vec<PathBuf> = entries.filter_map(|e| e.ok()).filter(|e| !skipped(&e.file_name().to_string_lossy())).map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    for d in dirs {
        skill_dirs(&d, depth - 1, out);
    }
}

/// Markdown files under `root` (slash commands and prompts).
fn command_files(root: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(root) else { return };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok()).filter(|e| !skipped(&e.file_name().to_string_lossy())).map(|e| e.path()).collect();
    paths.sort();
    for p in paths {
        if p.is_dir() {
            if depth > 0 {
                command_files(&p, depth - 1, out);
            }
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("md")) && !p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("README.md")) && out.len() < MAX_SKILLS {
            out.push(p);
        }
    }
}

/// (label, folder, is a folder of command files)
fn sources(home: &Path, project: Option<&Path>) -> Vec<(&'static str, PathBuf, bool)> {
    let mut v = vec![
        ("Claude Code", home.join(".claude/skills"), false),
        ("Claude Code command", home.join(".claude/commands"), true),
        ("Claude plugin", home.join(".claude/plugins/cache"), false),
        ("Codex", home.join(".codex/skills"), false),
        ("Codex prompt", home.join(".codex/prompts"), true),
        ("OpenCode", home.join(".config/opencode/skills"), false),
        ("OpenCode", home.join(".config/opencode/skill"), false),
        ("OpenCode command", home.join(".config/opencode/commands"), true),
        ("OpenCode command", home.join(".config/opencode/command"), true),
        ("Agents (shared)", home.join(".agents/skills"), false),
    ];
    if let Some(p) = project {
        for d in [".claude/skills", ".codex/skills", ".opencode/skills", ".opencode/skill", ".agents/skills"] {
            v.push(("This project", p.join(d), false));
        }
        for d in [".claude/commands", ".opencode/commands", ".opencode/command"] {
            v.push(("This project (command)", p.join(d), true));
        }
    }
    v
}

pub struct Library {
    dir: PathBuf,
}

impl Library {
    /// `config_dir` = the app's config folder (same for IDE and CLI).
    pub fn open(config_dir: &Path) -> Self {
        Library { dir: config_dir.join("skills") }
    }

    fn state(&self) -> State {
        fs::read(self.dir.join("library.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn mutate<R>(&self, f: impl FnOnce(&mut State) -> Result<R>) -> Result<R> {
        fs::create_dir_all(&self.dir)?;
        let lock = fs::OpenOptions::new().create(true).truncate(false).write(true).open(self.dir.join(".lock"))?;
        lock.lock()?;
        let mut s = self.state();
        let out = f(&mut s);
        if out.is_ok() {
            let tmp = self.dir.join(format!(".library.{}.tmp", std::process::id()));
            fs::write(&tmp, serde_json::to_vec_pretty(&s)?)?;
            fs::rename(&tmp, self.dir.join("library.json"))?;
        }
        let _ = lock.unlock();
        out
    }

    /// The folder of an installed skill.
    pub fn folder(&self, name: &str) -> Option<PathBuf> {
        let d = self.dir.join(name);
        (valid_name(name) && d.join("SKILL.md").is_file()).then_some(d)
    }

    fn entry(&self, name: &str, state: &State) -> Option<Skill> {
        let dir = self.folder(name)?;
        let text = read_capped(&dir.join("SKILL.md"), MAX_TEXT)?;
        let files = files_of(&dir);
        Some(Skill {
            name: name.to_string(),
            description: describe(&text).1,
            source: state.origin.get(name).cloned().unwrap_or_else(|| "You".into()),
            chat: !state.chat_off.contains(name),
            files: files.len(),
            bytes: files.iter().map(|f| f.1).sum(),
        })
    }

    pub fn list(&self) -> Vec<Skill> {
        let state = self.state();
        let Ok(entries) = fs::read_dir(&self.dir) else { return Vec::new() };
        let mut v: Vec<Skill> = entries.filter_map(|e| e.ok()).filter_map(|e| self.entry(&e.file_name().to_string_lossy(), &state)).collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    /// The raw SKILL.md of a skill.
    pub fn text(&self, name: &str) -> Result<String> {
        let dir = self.folder(name).ok_or_else(|| Error::msg(format!("No skill called {name}")))?;
        read_capped(&dir.join("SKILL.md"), MAX_TEXT).ok_or_else(|| Error::msg("SKILL.md could not be read"))
    }

    pub fn files(&self, name: &str) -> Vec<String> {
        self.folder(name).map(|d| files_of(&d).into_iter().map(|f| f.0).collect()).unwrap_or_default()
    }

    /// One extra file of a skill, as text.
    pub fn file(&self, name: &str, rel: &str) -> Result<String> {
        let dir = self.folder(name).ok_or_else(|| Error::msg(format!("No skill called {name}")))?;
        let rel = rel.trim().trim_start_matches("./").replace('\\', "/");
        // Only files the listing shows: that rules out "..", links and keys.
        if !files_of(&dir).iter().any(|f| f.0 == rel) {
            return Err(Error::msg(format!("{name} has no file {rel}")));
        }
        read_capped(&dir.join(&rel), MAX_TEXT).ok_or_else(|| Error::msg(format!("{rel} is not a text file under 200 KB")))
    }

    /// Create a skill, or replace the SKILL.md of `old` (renaming it when the
    /// frontmatter name changed). Returns the name.
    pub fn save(&self, old: Option<&str>, text: &str) -> Result<String> {
        if text.len() as u64 > MAX_TEXT {
            return Err(Error::msg("A SKILL.md is limited to 200 KB"));
        }
        let (name, description, _) = parse(text);
        let name = name.map(|n| slug(&n)).filter(|n| valid_name(n)).ok_or_else(|| Error::msg("Give the skill a name: a \"name:\" line between the --- lines at the top"))?;
        if description.is_none() {
            return Err(Error::msg("Add a \"description:\" line saying when to use this skill: the AI picks skills by it"));
        }
        let old = old.filter(|o| self.folder(o).is_some());
        if old != Some(name.as_str()) && self.dir.join(&name).exists() {
            return Err(Error::msg(format!("A skill called {name} already exists")));
        }
        if old.is_none() && self.list().len() >= MAX_SKILLS {
            return Err(Error::msg(format!("The library is limited to {MAX_SKILLS} skills")));
        }
        let dir = self.dir.join(&name);
        if let Some(o) = old.filter(|o| *o != name) {
            fs::rename(self.dir.join(o), &dir)?;
        }
        fs::create_dir_all(&dir)?;
        crate::files::write_text(&dir.join("SKILL.md"), text)?;
        let was = old.map(str::to_string);
        self.mutate(|s| {
            if let Some(o) = was.filter(|o| *o != name) {
                if s.chat_off.remove(&o) {
                    s.chat_off.insert(name.clone());
                }
                if let Some(v) = s.origin.remove(&o) {
                    s.origin.insert(name.clone(), v);
                }
            }
            Ok(())
        })?;
        Ok(name)
    }

    pub fn remove(&self, name: &str) -> Result<()> {
        let dir = self.folder(name).ok_or_else(|| Error::msg(format!("No skill called {name}")))?;
        fs::remove_dir_all(dir)?;
        self.mutate(|s| {
            s.chat_off.remove(name);
            s.origin.remove(name);
            Ok(())
        })
    }

    pub fn set_chat(&self, names: &[String], on: bool) -> Result<()> {
        self.mutate(|s| {
            for n in names.iter().filter(|n| valid_name(n)) {
                if on {
                    s.chat_off.remove(n);
                } else {
                    s.chat_off.insert(n.clone());
                }
            }
            Ok(())
        })
    }

    pub fn add_starter(&self, name: &str) -> Result<String> {
        let (_, text) = STARTERS.iter().find(|s| s.0 == name).ok_or_else(|| Error::msg("No such starter skill"))?;
        let n = self.save(None, text)?;
        self.mutate(|s| {
            s.origin.insert(n.clone(), "Starter".into());
            Ok(())
        })?;
        Ok(n)
    }

    fn status(&self, name: &str, md: &str) -> String {
        match self.text(name) {
            Ok(t) if t == md => "same",
            Ok(_) => "differs",
            Err(_) => "new",
        }
        .into()
    }

    fn found(&self, path: &Path, source: &str) -> Option<Found> {
        let (name, description, md) = candidate(path)?;
        Some(Found { status: self.status(&name, &md), name, description, source: source.to_string(), path: path.display().to_string() })
    }

    /// Skills and slash commands other tools keep on this computer. The same
    /// skill found twice (same name and text) is listed once.
    pub fn discover(&self, home: &Path, project: Option<&Path>) -> Vec<Found> {
        let mut out: Vec<Found> = Vec::new();
        let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
        for (label, root, commands) in sources(home, project) {
            let mut paths = Vec::new();
            if commands {
                command_files(&root, 2, &mut paths);
            } else {
                // Plugins sit deeper: cache/<market>/<plugin>/<version>/skills/<name>.
                skill_dirs(&root, if label == "Claude plugin" { 6 } else { 3 }, &mut paths);
            }
            for p in paths {
                let Some((name, _, md)) = candidate(&p) else { continue };
                if seen.insert((name, md)) {
                    out.extend(self.found(&p, label));
                }
            }
        }
        out
    }

    /// Skills inside a folder the user picked (itself, or folders below it).
    pub fn scan_folder(&self, dir: &Path) -> Vec<Found> {
        let mut paths = Vec::new();
        skill_dirs(dir, 4, &mut paths);
        paths.iter().filter_map(|p| self.found(p, "Folder")).collect()
    }

    /// Copy found skills into the library. A skill whose name is taken by a
    /// different one is replaced when `replace`, else added as "name-2".
    pub fn import(&self, items: &[Found], replace: bool) -> Result<Vec<String>> {
        let mut added = Vec::new();
        let mut count = self.list().len();
        for item in items {
            let path = PathBuf::from(&item.path);
            let Some((base, _, mut md)) = candidate(&path) else { continue };
            let mut name = base.clone();
            match self.status(&name, &md).as_str() {
                "same" => continue,
                "differs" if !replace => {
                    let free = (2..100).map(|i| format!("{}-{i}", clip_name(&base))).find(|n| !self.dir.join(n).exists());
                    let Some(n) = free else { continue };
                    name = n;
                }
                _ => {}
            }
            if !self.dir.join(&name).exists() {
                if count >= MAX_SKILLS {
                    break;
                }
                count += 1;
            }
            if path.is_dir() && path.starts_with(&self.dir) {
                continue;
            }
            // Copy to a temporary folder, then swap it in.
            fs::create_dir_all(&self.dir)?;
            let tmp = self.dir.join(format!(".import-{}", uuid::Uuid::new_v4().simple()));
            fs::create_dir_all(&tmp)?;
            if path.is_dir() {
                for (rel, _) in files_of(&path) {
                    let to = tmp.join(&rel);
                    if let Some(parent) = to.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::copy(path.join(&rel), to)?;
                }
            }
            if name != base {
                md = rename_in(&md, &name);
            }
            fs::write(tmp.join("SKILL.md"), md)?;
            let dest = self.dir.join(&name);
            if dest.exists() {
                fs::remove_dir_all(&dest)?;
            }
            fs::rename(&tmp, &dest)?;
            let source = item.source.clone();
            self.mutate(|s| {
                s.origin.insert(name.clone(), source);
                Ok(())
            })?;
            added.push(name);
        }
        Ok(added)
    }
}

fn clip_name(n: &str) -> &str {
    &n[..n.len().min(MAX_NAME - 3)]
}

/// Change the frontmatter name (or add frontmatter) after a rename.
fn rename_in(md: &str, name: &str) -> String {
    let (old, description, body) = parse(md);
    if old.is_some() {
        let mut done = false;
        let lines: Vec<String> = md
            .split('\n')
            .map(|l| {
                if !done && l.starts_with("name:") {
                    done = true;
                    format!("name: {name}")
                } else {
                    l.to_string()
                }
            })
            .collect();
        return lines.join("\n");
    }
    compose(name, &description.unwrap_or_else(|| first_line(body)), body)
}

/// The list of skills a model gets with every message: one line each.
pub fn index(skills: &[Skill]) -> Option<String> {
    if skills.is_empty() {
        return None;
    }
    let mut s = String::from("<skills>\nYou have these skills: instructions for specific kinds of work. When a task matches one, call use_skill with its name BEFORE doing the task, then follow it. Do not mention skills that you did not use.\n");
    for (i, k) in skills.iter().enumerate() {
        let line = format!("- {}: {}\n", k.name, clip(&k.description, INDEX_DESC));
        if s.len() + line.len() > INDEX_CAP {
            let rest: Vec<&str> = skills[i..].iter().map(|k| k.name.as_str()).collect();
            s.push_str(&format!("- More (names only): {}\n", clip(&rest.join(", "), 3000)));
            break;
        }
        s.push_str(&line);
    }
    s.push_str("</skills>");
    Some(s)
}

#[cfg(test)]
mod tests;
