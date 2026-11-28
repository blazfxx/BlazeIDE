//! `blaze skills`: the same library Blaze IDE uses.

use std::path::PathBuf;

use blaze_core::skills::{Found, Library, STARTERS};
use clap::Subcommand;

use crate::config;

#[derive(Subcommand)]
pub enum SkillsCmd {
    /// The skills in your library.
    List,
    /// Claude Code, Codex and OpenCode skills found on this computer.
    Found,
    /// Copy found skills into your library (names, or --all).
    Import {
        names: Vec<String>,
        #[arg(long)]
        all: bool,
        /// Replace library skills that differ, instead of adding "name-2".
        #[arg(long)]
        replace: bool,
    },
    /// Import the skill(s) in a folder.
    Add { folder: PathBuf },
    /// Add a starter skill (without a name: list them).
    Starter { name: Option<String> },
    /// Offer a skill in chats, or not.
    Chat {
        name: String,
        #[arg(value_parser = ["on", "off"])]
        state: String,
    },
    /// Delete a skill from the library.
    Remove { name: String },
}

fn err(e: blaze_core::error::Error) -> String {
    e.to_string()
}

fn short(s: &str) -> String {
    let t: String = s.chars().take(90).collect();
    if t.len() < s.len() { format!("{t}…") } else { t }
}

/// Names come from files of other tools: no control characters on a terminal.
fn clean(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

fn show_found(found: &[Found]) {
    for f in found {
        let mark = match f.status.as_str() {
            "same" => "(in library)",
            "differs" => "(differs from library)",
            _ => "",
        };
        println!("{:<28} {:<22} {} {mark}", f.name, f.source, clean(&short(&f.description)));
    }
}

fn found_here(lib: &Library) -> Vec<Found> {
    let home = dirs::home_dir().unwrap_or_default();
    let cwd = std::env::current_dir().ok().filter(|d| *d != home);
    lib.discover(&home, cwd.as_deref())
}

pub fn run(cmd: SkillsCmd) -> Result<(), String> {
    let lib = Library::open(&config::config_dir());
    match cmd {
        SkillsCmd::List => {
            let all = lib.list();
            if all.is_empty() {
                println!("No skills yet. `blaze skills found` shows what can be imported; `blaze skills starter` lists starters.");
            }
            for s in all {
                println!("{:<28} {:<20} {} {}", s.name, clean(&s.source), if s.chat { "chat " } else { "     " }, clean(&short(&s.description)));
            }
        }
        SkillsCmd::Found => {
            let f = found_here(&lib);
            if f.is_empty() {
                println!("No Claude Code, Codex or OpenCode skills found. Use `blaze skills add <folder>`.");
            }
            show_found(&f);
        }
        SkillsCmd::Import { names, all, replace } => {
            if names.is_empty() && !all {
                return Err("Name the skills to import, or use --all".into());
            }
            let mut items = found_here(&lib);
            if !all {
                for n in &names {
                    if !items.iter().any(|f| &f.name == n) {
                        return Err(format!("No skill called {n} was found (see `blaze skills found`)"));
                    }
                }
                items.retain(|f| names.contains(&f.name));
            }
            let added = lib.import(&items, replace).map_err(err)?;
            println!("Imported {} skill(s){}{}", added.len(), if added.is_empty() { "" } else { ": " }, added.join(", "));
        }
        SkillsCmd::Add { folder } => {
            let found = lib.scan_folder(&folder);
            if found.is_empty() {
                return Err(format!("No SKILL.md found in {}", folder.display()));
            }
            let added = lib.import(&found, false).map_err(err)?;
            println!("Imported {} skill(s): {}", added.len(), added.join(", "));
        }
        SkillsCmd::Starter { name: None } => {
            for (name, text) in STARTERS {
                println!("{name:<20} {}", short(&blaze_core::skills::parse(text).1.unwrap_or_default()));
            }
        }
        SkillsCmd::Starter { name: Some(n) } => println!("Added {}", lib.add_starter(&n).map_err(err)?),
        SkillsCmd::Chat { name, state } => {
            if lib.folder(&name).is_none() {
                return Err(format!("No skill called {name}"));
            }
            lib.set_chat(std::slice::from_ref(&name), state == "on").map_err(err)?;
            println!("{name}: chat {state}");
        }
        SkillsCmd::Remove { name } => {
            lib.remove(&name).map_err(err)?;
            println!("Removed {name}");
        }
    }
    Ok(())
}
