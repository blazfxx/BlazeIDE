//! Discord status for Blaze IDE: "Working on <file or project> - BlazeCLI -
//! blazeapi.org" (see blaze_core::presence). The window reports what is open.

use std::sync::Mutex;

use blaze_core::presence::Presence;
use serde_json::Value;
use tauri::{AppHandle, Manager};

use crate::AppState;

static PRESENCE: Mutex<Option<Presence>> = Mutex::new(None);

fn enabled(app: &AppHandle) -> bool {
    app.state::<AppState>().store.settings().get("discordStatus").and_then(Value::as_bool).unwrap_or(true)
}

/// Start (or stop) to match the setting.
pub fn sync(app: &AppHandle) {
    let mut p = PRESENCE.lock().expect("presence");
    if enabled(app) {
        if p.is_none() {
            *p = Presence::start();
        }
    } else if let Some(old) = p.take() {
        old.clear();
    }
}

#[tauri::command]
pub fn presence_set(app: AppHandle, what: String) {
    sync(&app);
    if let Some(p) = PRESENCE.lock().expect("presence").as_ref() {
        let w: String = what.chars().take(100).collect();
        p.set(if w.trim().is_empty() { "a new project" } else { w.trim() });
    }
}
