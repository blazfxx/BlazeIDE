//! The skills library in Blaze IDE: commands for the Skills window.

use std::path::PathBuf;
use std::sync::Mutex;

use blaze_core::skills::{self, Found, Library, Skill};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::AppState;

type CmdResult<T> = Result<T, String>;

/// What the window has been shown: only these can be imported, so the page
/// cannot ask to copy any other folder of the computer into the library.
static OFFERED: Mutex<Vec<Found>> = Mutex::new(Vec::new());

fn offer(found: &[Found]) {
    let mut o = OFFERED.lock().expect("offered");
    o.retain(|x| !found.iter().any(|f| f.path == x.path));
    o.extend_from_slice(found);
}

fn lib(app: &AppHandle) -> CmdResult<Library> {
    Ok(Library::open(&app.path().app_config_dir().map_err(|e| e.to_string())?))
}

#[tauri::command]
pub fn skills_list(app: AppHandle) -> CmdResult<Vec<Skill>> {
    Ok(lib(&app)?.list())
}

#[tauri::command]
pub fn skills_get(app: AppHandle, name: String) -> CmdResult<Value> {
    let l = lib(&app)?;
    let text = l.text(&name).map_err(|e| e.to_string())?;
    Ok(json!({ "text": text, "files": l.files(&name), "folder": l.folder(&name).map(|p| p.display().to_string()) }))
}

#[tauri::command]
pub fn skills_save(app: AppHandle, old: Option<String>, text: String) -> CmdResult<String> {
    lib(&app)?.save(old.as_deref(), &text).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn skills_remove(app: AppHandle, name: String) -> CmdResult<()> {
    lib(&app)?.remove(&name).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn skills_set_chat(app: AppHandle, names: Vec<String>, on: bool) -> CmdResult<()> {
    lib(&app)?.set_chat(&names, on).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn skills_starters(app: AppHandle) -> CmdResult<Vec<Value>> {
    let have = lib(&app)?.list();
    Ok(skills::STARTERS
        .iter()
        .map(|(name, text)| json!({ "name": name, "description": skills::parse(text).1.unwrap_or_default(), "added": have.iter().any(|s| s.name == *name) }))
        .collect())
}

#[tauri::command]
pub fn skills_add_starter(app: AppHandle, name: String) -> CmdResult<String> {
    lib(&app)?.add_starter(&name).map_err(|e| e.to_string())
}

/// Skills of Claude Code, Codex and OpenCode on this computer (and in the
/// open project).
#[tauri::command]
pub async fn skills_discover(app: AppHandle, state: State<'_, AppState>, project: Option<String>) -> CmdResult<Vec<Found>> {
    let l = lib(&app)?;
    let home = app.path().home_dir().map_err(|e| e.to_string())?;
    // Only a folder the user opened as a project.
    let project = project.filter(|p| state.store.recent_projects().iter().any(|r| r == p)).map(PathBuf::from);
    let found = tauri::async_runtime::spawn_blocking(move || l.discover(&home, project.as_deref())).await.map_err(|e| e.to_string())?;
    offer(&found);
    Ok(found)
}

/// Let the user pick a folder and list the skills in it.
#[tauri::command]
pub async fn skills_pick_folder(app: AppHandle) -> CmdResult<Option<Vec<Found>>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().set_title("Choose a skill folder, or a folder of skills").pick_folder(move |p| {
        let _ = tx.send(p);
    });
    let Some(picked) = rx.await.ok().flatten() else { return Ok(None) };
    let dir = picked.into_path().map_err(|e| e.to_string())?;
    let l = lib(&app)?;
    let found = tauri::async_runtime::spawn_blocking(move || l.scan_folder(&dir)).await.map_err(|e| e.to_string())?;
    offer(&found);
    Ok(Some(found))
}

#[tauri::command]
pub async fn skills_import(app: AppHandle, paths: Vec<String>, replace: bool) -> CmdResult<Vec<String>> {
    let l = lib(&app)?;
    let items: Vec<Found> = OFFERED.lock().expect("offered").iter().filter(|f| paths.contains(&f.path)).cloned().collect();
    tauri::async_runtime::spawn_blocking(move || l.import(&items, replace)).await.map_err(|e| e.to_string())?.map_err(|e| e.to_string())
}

/// A use_skill call from the IDE chat.
#[tauri::command]
pub fn skills_use(app: AppHandle, arguments: String) -> CmdResult<String> {
    let l = lib(&app)?;
    let allowed: Vec<String> = l.list().into_iter().filter(|s| s.chat).map(|s| s.name).collect();
    Ok(skills::tool::run(&l, &allowed, &arguments))
}
