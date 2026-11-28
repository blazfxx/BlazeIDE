//! Tauri commands: the only entry points the UI has. Each one validates its
//! input (paths through the sandbox) and delegates to blaze-core.

use std::path::PathBuf;

use blaze_core::{ai, auth, chats, extensions, files, models, providers, tools};
use serde::Serialize;
use serde_json::{Map, Value};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_updater::UpdaterExt;
use tokio_util::sync::CancellationToken;

use crate::{analytics, secrets, windows, AppState};

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

// --- auth ---------------------------------------------------------------

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SessionPublic {
    user_email: String,
    username: String,
    /// Owner account: may use the "Cheapest" routing (UI-level option).
    admin: bool,
}

/// Accounts that see the owner-only "Cheapest" routing choice.
const ADMIN_USERS: &[&str] = &["blazfoxx"];

fn is_admin_name(name: &str) -> bool {
    ADMIN_USERS.contains(&name.to_lowercase().as_str())
}

fn session_from(v: &Value) -> SessionPublic {
    let username = v.get("username").and_then(Value::as_str).unwrap_or("").to_string();
    SessionPublic {
        user_email: v.get("email").and_then(Value::as_str).unwrap_or("").to_string(),
        admin: is_admin_name(&username),
        username,
    }
}

#[tauri::command]
pub fn auth_get_session(state: State<'_, AppState>) -> Option<SessionPublic> {
    state.api_key()?;
    state.store.account().map(|a| session_from(&a))
}

#[tauri::command]
pub async fn auth_sign_in(app: AppHandle, state: State<'_, AppState>) -> CmdResult<SessionPublic> {
    let cancel = CancellationToken::new();
    if let Some(prev) = state.signin.lock().expect("signin lock").replace(cancel.clone()) {
        prev.cancel();
    }
    let opener = app.clone();
    let signed = auth::sign_in(
        &state.http,
        &state.api_base,
        |url| opener.opener().open_url(url, None::<&str>).map_err(|e| blaze_core::Error::msg(format!("Could not open the browser: {e}"))),
        cancel,
    )
    .await
    .map_err(err)?;
    secrets::save_key(&signed.api_key)?;
    *state.key.write().expect("key lock") = Some(signed.api_key.clone());
    state.models.write().expect("models lock").clear();
    analytics::track(&app, "sign_in", None);
    let account = serde_json::json!({ "email": signed.email, "username": signed.username });
    state.store.set_account(Some(account.clone())).map_err(err)?;
    // Bring the window back to the front after the browser round trip.
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_focus();
    }
    Ok(session_from(&account))
}

#[tauri::command]
pub fn auth_cancel(state: State<'_, AppState>) {
    if let Some(c) = state.signin.lock().expect("signin lock").take() {
        c.cancel();
    }
}

#[tauri::command]
pub async fn auth_logout(state: State<'_, AppState>) -> CmdResult<()> {
    if let Some(key) = state.api_key() {
        auth::revoke_key(&state.http, &state.api_base, &key).await;
    }
    secrets::delete_key();
    *state.key.write().expect("key lock") = None;
    state.models.write().expect("models lock").clear();
    state.store.set_account(None).map_err(err)?;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageInfo {
    balance_usd: f64,
}

#[tauri::command]
pub async fn auth_get_usage(state: State<'_, AppState>) -> CmdResult<Option<UsageInfo>> {
    let Some(key) = state.api_key() else { return Ok(None) };
    Ok(models::fetch_balance(&state.http, &state.v1_base, &key).await.ok().map(|b| UsageInfo { balance_usd: b }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    latest: String,
    min: String,
    current: Option<String>,
    must_update: bool,
    download_url: String,
}

const DOWNLOAD_PAGE: &str = "https://blazeapi.org/ide";

/// Look for a signed update at blazeapi.org. Never fails the UI: a network
/// hiccup just means "no update right now".
#[tauri::command]
pub async fn check_version(app: AppHandle, state: State<'_, AppState>) -> CmdResult<VersionInfo> {
    let current = app.package_info().version.to_string();
    let mut latest = current.clone();
    if let Ok(updater) = app.updater() {
        if let Ok(Ok(Some(update))) = tokio::time::timeout(std::time::Duration::from_secs(8), updater.check()).await {
            latest = update.version.clone();
            *state.update.lock().expect("update lock") = Some(update);
        }
    }
    Ok(VersionInfo { latest, min: current.clone(), current: Some(current), must_update: false, download_url: DOWNLOAD_PAGE.into() })
}

/// Download the update found by `check_version`, verify its signature (the
/// updater refuses anything not signed with our key), install it and restart.
#[tauri::command]
pub async fn install_update(app: AppHandle, state: State<'_, AppState>) -> CmdResult<()> {
    let update = state.update.lock().expect("update lock").take().ok_or_else(|| "No update is ready. Check again in a moment.".to_string())?;
    let mut done: u64 = 0;
    let handle = app.clone();
    update
        .download_and_install(
            move |chunk, total| {
                done += chunk as u64;
                let _ = handle.emit("update-progress", serde_json::json!({ "done": done, "total": total }));
            },
            || {},
        )
        .await
        .map_err(|e| format!("The update could not be installed ({e}). Download it from {DOWNLOAD_PAGE}."))?;
    app.restart()
}

/// Automatic updates (setting `autoUpdate`, default on): check, download and
/// verify in the background, then keep the download until Restart or quit.
/// (macOS used to install in place while running: the running app then no
/// longer matched the bundle on disk, and macOS kept re-asking for folder
/// access because it could not remember the answer for it.)
/// Returns {status: off|current|installed|ready, version?}.
#[tauri::command]
pub async fn auto_update(app: AppHandle, state: State<'_, AppState>) -> CmdResult<Value> {
    if !setting_bool(&state, "autoUpdate", true) {
        return Ok(serde_json::json!({ "status": "off" }));
    }
    let ready = state.update_ready.lock().expect("update lock").as_ref().map(|(u, _)| u.version.clone());
    if let Some(version) = ready {
        return Ok(serde_json::json!({ "status": "ready", "version": version }));
    }
    let Ok(updater) = app.updater() else { return Ok(serde_json::json!({ "status": "current" })) };
    let update = match tokio::time::timeout(std::time::Duration::from_secs(15), updater.check()).await {
        Ok(Ok(Some(u))) => u,
        _ => return Ok(serde_json::json!({ "status": "current" })),
    };
    let version = update.version.clone();
    let mut done: u64 = 0;
    let handle = app.clone();
    let bytes = update
        .download(
            move |chunk, total| {
                done += chunk as u64;
                let _ = handle.emit("update-progress", serde_json::json!({ "done": done, "total": total }));
            },
            || {},
        )
        .await
        .map_err(|e| format!("The update could not be downloaded ({e})."))?;
    *state.update_ready.lock().expect("update lock") = Some((update, bytes));
    Ok(serde_json::json!({ "status": "ready", "version": version }))
}

/// Restart into the new version (installs a waiting Windows update first).
#[tauri::command]
pub async fn apply_update(app: AppHandle, state: State<'_, AppState>) -> CmdResult<()> {
    let ready = state.update_ready.lock().expect("update lock").take();
    if let Some((update, bytes)) = ready {
        update.install(&bytes).map_err(|e| format!("The update could not be installed ({e}). Download it from {DOWNLOAD_PAGE}."))?;
    }
    app.restart()
}

/// Runs once at startup: record the launch and whether this is a fresh version.
pub fn on_launch(app: &AppHandle) {
    let state = app.state::<AppState>();
    let v = app.package_info().version.to_string();
    let last = state.store.settings().get("lastRunVersion").and_then(Value::as_str).map(str::to_string);
    let mut patch = Map::new();
    patch.insert("lastRunVersion".into(), Value::String(v.clone()));
    let _ = state.store.patch_settings(patch);
    analytics::track(app, "app_open", None);
    if last.is_some_and(|l| l != v) {
        analytics::track(app, "update_installed", None);
    }
}

// --- fs -------------------------------------------------------------------

#[tauri::command]
pub async fn fs_open_folder(app: AppHandle, state: State<'_, AppState>) -> CmdResult<Option<String>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().set_title("Open Folder").pick_folder(move |p| {
        let _ = tx.send(p);
    });
    let Some(picked) = rx.await.ok().flatten() else { return Ok(None) };
    let path = picked.into_path().map_err(err)?;
    let root = state.sandbox.grant_root(&path).map_err(err)?;
    let s = root.display().to_string();
    state.store.add_recent_project(&s).map_err(err)?;
    Ok(Some(s))
}

#[tauri::command]
pub async fn fs_read_tree(state: State<'_, AppState>, root: String) -> CmdResult<files::FileNode> {
    let p = state.sandbox.require(&root).map_err(err)?;
    tauri::async_runtime::spawn_blocking(move || files::build_tree(&p)).await.map_err(err)?.map_err(err)
}

#[tauri::command]
pub async fn fs_read_file(state: State<'_, AppState>, path: String) -> CmdResult<String> {
    let p = state.sandbox.require(&path).map_err(err)?;
    files::read_text(&p).map_err(err)
}

#[tauri::command]
pub async fn fs_try_read_file(state: State<'_, AppState>, path: String) -> CmdResult<Option<String>> {
    let p = state.sandbox.require(&path).map_err(err)?;
    Ok(files::read_text(&p).ok())
}

#[tauri::command]
pub async fn fs_write_file(state: State<'_, AppState>, path: String, content: String) -> CmdResult<()> {
    let p = state.sandbox.require(&path).map_err(err)?;
    files::write_text(&p, &content).map_err(err)
}

#[tauri::command]
pub async fn fs_create_file(state: State<'_, AppState>, path: String) -> CmdResult<()> {
    let p = state.sandbox.require(&path).map_err(err)?;
    files::create_file(&p).map_err(err)
}

#[tauri::command]
pub async fn fs_create_folder(state: State<'_, AppState>, path: String) -> CmdResult<()> {
    let p = state.sandbox.require(&path).map_err(err)?;
    files::create_folder(&p).map_err(err)
}

#[tauri::command]
pub async fn fs_rename(state: State<'_, AppState>, old_path: String, new_path: String) -> CmdResult<()> {
    let from = state.sandbox.require(&old_path).map_err(err)?;
    let to = state.sandbox.require(&new_path).map_err(err)?;
    if state.sandbox.is_root(&from) {
        return Err("Refusing to rename a project root from within the IDE".into());
    }
    files::rename(&from, &to).map_err(err)
}

#[tauri::command]
pub async fn fs_delete(state: State<'_, AppState>, path: String) -> CmdResult<()> {
    let p = state.sandbox.require(&path).map_err(err)?;
    if state.sandbox.is_root(&p) {
        return Err("Refusing to delete a project root from within the IDE".into());
    }
    tauri::async_runtime::spawn_blocking(move || files::delete_to_trash(&p)).await.map_err(err)?.map_err(err)
}

#[tauri::command]
pub async fn fs_search(state: State<'_, AppState>, root: String, query: String) -> CmdResult<Vec<files::SearchHit>> {
    let p = state.sandbox.require(&root).map_err(err)?;
    tauri::async_runtime::spawn_blocking(move || files::search(&p, &query)).await.map_err(err)
}

#[tauri::command]
pub async fn fs_replace(state: State<'_, AppState>, root: String, query: String, replacement: String) -> CmdResult<files::ReplaceResult> {
    let p = state.sandbox.require_root(&root).map_err(err)?;
    tauri::async_runtime::spawn_blocking(move || files::replace_all(&p, &query, &replacement)).await.map_err(err)
}

#[tauri::command]
pub fn fs_watch(app: AppHandle, state: State<'_, AppState>, root: String) -> CmdResult<()> {
    let p = state.sandbox.require(&root).map_err(err)?;
    let handle = app.clone();
    state
        .watch
        .watch(&p, std::sync::Arc::new(move || {
            let _ = handle.emit("fs-changed", ());
        }))
        .map_err(err)
}

#[tauri::command]
pub fn fs_unwatch(state: State<'_, AppState>) {
    state.watch.unwatch_all();
}

#[tauri::command]
pub fn get_recent_projects(state: State<'_, AppState>) -> Vec<String> {
    state.store.recent_projects().into_iter().filter(|p| PathBuf::from(p).is_dir()).collect()
}

// --- terminal ---------------------------------------------------------------

#[derive(Serialize, Clone)]
struct TermData {
    id: String,
    data: String,
}

#[tauri::command]
pub fn term_create(window: WebviewWindow, state: State<'_, AppState>, cwd: String) -> CmdResult<String> {
    // A terminal may start in an open project; anything else falls back to home.
    let dir = state.sandbox.require(&cwd).ok().filter(|p| p.is_dir()).or_else(dirs_home).unwrap_or_else(|| PathBuf::from("."));
    let label = window.label().to_string();
    let (w1, l1) = (window.clone(), label.clone());
    let (w2, l2) = (window, label);
    state
        .pty
        .create(
            &dir,
            std::sync::Arc::new(move |id, data| {
                let _ = w1.emit_to(l1.as_str(), "term-data", TermData { id: id.to_string(), data });
            }),
            std::sync::Arc::new(move |id| {
                let _ = w2.emit_to(l2.as_str(), "term-exit", id.to_string());
            }),
        )
        .map_err(err)
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

#[tauri::command]
pub fn term_write(state: State<'_, AppState>, id: String, data: String) {
    state.pty.write(&id, &data);
}

#[tauri::command]
pub fn term_resize(state: State<'_, AppState>, id: String, cols: u16, rows: u16) {
    state.pty.resize(&id, cols, rows);
}

#[tauri::command]
pub fn term_kill(state: State<'_, AppState>, id: String) {
    state.pty.kill(&id);
}

// --- ai ---------------------------------------------------------------------

fn setting_bool(state: &AppState, key: &str, default: bool) -> bool {
    state.store.settings().get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn setting_str(state: &AppState, key: &str) -> String {
    state.store.settings().get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// The routing choice for BlazeAPI requests: "normal", "direct", "cheapest"
/// (owner only) or "" (the account's own setting).
fn routing(state: &AppState) -> String {
    match setting_str(state, "aiRouting").as_str() {
        "normal" => "normal".into(),
        // Direct was retired: it became Normal routing with the trusted sellers.
        "direct" | "trusted" => "trusted".into(),
        "cheapest" => "cheapest".into(),
        "fastest" => "fastest".into(),
        _ => String::new(),
    }
}

pub(crate) fn routing_for(state: &AppState) -> String {
    routing(state)
}

/// Where a model id is served from. `custom:<id>/<model>` is a provider the
/// user added; everything else is BlazeAPI.
enum Route {
    Blaze { key: Option<String> },
    Custom { base: String, key: Option<String>, model: String, wire: String },
}

fn route_for(state: &AppState, model: &str) -> Result<Route, String> {
    match providers::split_model_id(model) {
        None => Ok(Route::Blaze { key: state.api_key() }),
        Some((pid, m)) => {
            let p = state.store.providers().into_iter().find(|p| p.id == pid).ok_or("That custom provider was removed. Pick another model.")?;
            Ok(Route::Custom { base: p.base_url, key: secrets::load_provider_key(pid), model: m.to_string(), wire: p.wire })
        }
    }
}

#[tauri::command]
pub async fn ai_send(app: AppHandle, window: WebviewWindow, state: State<'_, AppState>, req: ai::AiRequest) -> CmdResult<()> {
    let mut req = req;
    let label = window.label().to_string();
    let id = req.request_id.clone();
    let send = |v: Value| {
        let _ = window.emit_to(label.as_str(), "ai-chunk", v);
    };
    let with_id = |ev: &ai::StreamEvent| {
        let mut v = serde_json::to_value(ev).unwrap_or_default();
        v["requestId"] = Value::String(id.clone());
        v
    };
    let done = serde_json::json!({ "requestId": id, "type": "done" });
    let fail = |message: &str, status: Option<u16>| {
        let mut ev = serde_json::json!({ "requestId": id, "type": "error", "message": message });
        if let Some(s) = status {
            ev["status"] = s.into();
        }
        send(ev);
        send(done.clone());
    };
    // Image generation is opt-in: the tool is only offered once the user has
    // turned it on, whatever the page asks for.
    req.media_gen = Some(setting_bool(&state, "aiMediaGen", false) && req.media_gen != Some(false));
    // Video generation costs real money per clip: a separate opt-in.
    req.video_gen = Some(setting_bool(&state, "aiVideoGen", false) && req.video_gen != Some(false));

    // Skills switched on for chat: one line each, plus the use_skill tool.
    if matches!(req.mode.as_str(), "agent" | "plan") {
        if let Ok(dir) = app.path().app_config_dir() {
            let on: Vec<_> = blaze_core::skills::Library::open(&dir).list().into_iter().filter(|s| s.chat).collect();
            if let Some(list) = blaze_core::skills::index(&on) {
                req.extra_system = Some(match req.extra_system.take() {
                    Some(s) => format!("{s}\n\n{list}"),
                    None => list,
                });
                req.extra_tools.push(blaze_core::skills::tool::schema());
            }
        }
    }

    // The web tools, for agent and plan turns.
    if matches!(req.mode.as_str(), "agent" | "plan") && crate::web::enabled(&state) {
        let note = blaze_core::web::tool::NOTE;
        req.extra_system = Some(match req.extra_system.take() {
            Some(s) => format!("{s}\n\n{note}"),
            None => note.to_string(),
        });
        req.extra_tools.extend(blaze_core::web::tool::schemas(&req.mode));
    }

    let route = match route_for(&state, &req.model) {
        Ok(r) => r,
        Err(m) => {
            fail(&m, None);
            return Ok(());
        }
    };
    let cancel = CancellationToken::new();
    state.ai_requests.lock().expect("ai lock").insert(id.clone(), cancel.clone());
    let res = match &route {
        Route::Blaze { key } => {
            let Some(key) = key else {
                state.ai_requests.lock().expect("ai lock").remove(&id);
                fail("Not signed in", Some(401));
                return Ok(());
            };
            analytics::track(&app, "ai_request", Some(&req.model));
            ai::stream_chat(&state.http, &ai::Target::blaze_routed(&state.v1_base, key, &routing(&state)), &req, cancel, |ev| send(with_id(&ev))).await
        }
        Route::Custom { base, key, model, wire } => {
            analytics::track(&app, "ai_request", None);
            // Third-party servers may reject OpenAI's newer parameters, so
            // send the plain request.
            req.model = model.clone();
            req.reasoning_effort = Some("off".into());
            ai::stream_chat(&state.http_custom, &ai::Target::custom(base, key.as_deref()).with_wire(wire), &req, cancel, |ev| send(with_id(&ev))).await
        }
    };
    state.ai_requests.lock().expect("ai lock").remove(&id);
    if let Err(e) = res {
        let mut ev = serde_json::json!({ "requestId": id, "type": "error", "message": e.to_string() });
        if let Some(s) = e.status() {
            ev["status"] = s.into();
        }
        send(ev);
    }
    send(done);
    Ok(())
}

#[tauri::command]
pub fn ai_cancel(state: State<'_, AppState>, request_id: String) {
    if let Some(c) = state.ai_requests.lock().expect("ai lock").remove(&request_id) {
        c.cancel();
    }
}

/// BlazeAPI's list (public when signed out) plus the user's custom providers.
/// The error is set only when BlazeAPI could not be reached and nothing was
/// cached, so the picker can say why it is empty.
async fn load_models(state: &AppState, force: bool) -> (Vec<models::ModelEntry>, Option<String>) {
    let cached = state.models.read().expect("models lock").clone();
    let mut error = None;
    let mut list = if !force && !cached.is_empty() {
        cached
    } else {
        let key = state.api_key();
        match models::fetch_models(&state.http, &state.v1_base, key.as_deref(), &routing(state)).await {
            Ok((fresh, raw)) if !fresh.is_empty() => {
                *state.models.write().expect("models lock") = fresh.clone();
                let _ = state.store.set_model_cache(raw);
                fresh
            }
            result => {
                // Offline or an empty answer: last good list from disk.
                let disk = state.store.model_cache().map(|v| models::parse_models(&v)).unwrap_or_default();
                if disk.is_empty() {
                    error = Some(match result {
                        Err(e) => e.to_string(),
                        _ if state.api_key().is_some() => "No models are available for this account. With a $0 balance, or with Direct Source turned on in your BlazeAPI dashboard, nothing can be called: add balance or switch Direct Source off, then retry.".to_string(),
                        _ => "BlazeAPI returned no models.".to_string(),
                    });
                }
                disk
            }
        }
    };
    providers::learn_missing(&state.store, &state.http_custom, secrets::load_provider_key).await;
    for p in state.store.providers() {
        list.extend(providers::entries(&p));
    }
    (list, error)
}

#[tauri::command]
pub async fn ai_get_models(state: State<'_, AppState>) -> CmdResult<Vec<models::ModelEntry>> {
    match load_models(&state, false).await {
        (list, Some(e)) if list.is_empty() => Err(e),
        (list, _) => Ok(list),
    }
}

#[tauri::command]
pub async fn ai_refresh_models(state: State<'_, AppState>) -> CmdResult<Vec<models::ModelEntry>> {
    match load_models(&state, true).await {
        (list, Some(e)) if list.is_empty() => Err(e),
        (list, _) => Ok(list),
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompleteReq {
    prefix: String,
    suffix: String,
    filename: Option<String>,
}

fn tail_chars(s: &str, n: usize) -> &str {
    let count = s.chars().count();
    if count <= n { s } else { s.char_indices().nth(count - n).map_or(s, |(i, _)| &s[i..]) }
}

fn head_chars(s: &str, n: usize) -> &str {
    s.char_indices().nth(n).map_or(s, |(i, _)| &s[..i])
}

/// Ghost-text completion for the editor. Quiet by design: when the setting is
/// off, the user is signed out or the model fails, the answer is empty.
#[tauri::command]
pub async fn ai_complete(state: State<'_, AppState>, req: CompleteReq) -> CmdResult<String> {
    if !setting_bool(&state, "aiAutocomplete", true) || req.prefix.trim().is_empty() {
        return Ok(String::new());
    }
    let chosen = setting_str(&state, "aiAutocompleteModel");
    let candidates: Vec<String> = if !chosen.is_empty() {
        vec![chosen]
    } else {
        let list = state.models.read().expect("models lock").clone();
        let c = models::completion_candidates(&list);
        if c.is_empty() { vec![setting_str(&state, "selectedModel")] } else { c }
    };
    let prefix = tail_chars(&req.prefix, 6000);
    let suffix = head_chars(&req.suffix, 2000);
    let name = req.filename.as_deref().map(|f| f.rsplit(['/', '\\']).next().unwrap_or(f));
    for model in candidates {
        let out = match route_for(&state, &model)? {
            Route::Blaze { key: Some(key) } => ai::complete(&state.http, &ai::Target::blaze_routed(&state.v1_base, &key, &routing(&state)), &model, name, prefix, suffix).await,
            Route::Blaze { key: None } => return Ok(String::new()),
            Route::Custom { base, key, model, .. } => ai::complete(&state.http_custom, &ai::Target::custom(&base, key.as_deref()), &model, name, prefix, suffix).await,
        };
        if let Ok(text) = out {
            return Ok(text);
        }
    }
    Ok(String::new())
}

const CORE_TOOLS: &[&str] = &["read_file", "write_file", "list_directory", "search_files", "run_command"];

#[tauri::command]
pub async fn ai_run_tool(state: State<'_, AppState>, name: String, args: String, project_root: String) -> CmdResult<String> {
    if !CORE_TOOLS.contains(&name.as_str()) {
        return Ok(format!("Error: {name} cannot be run here."));
    }
    let root = match state.sandbox.require_root(&project_root) {
        Ok(r) => r,
        Err(_) => return Ok("Error: no project folder is open.".into()),
    };
    Ok(tools::run_tool(&name, &args, &root).await)
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaReq {
    /// "image" (default) or "video".
    #[serde(default)]
    kind: Option<String>,
    prompt: String,
    path: String,
    aspect_ratio: Option<String>,
    /// Video: the project image to animate.
    #[serde(default)]
    image: Option<String>,
    project_root: String,
}

#[tauri::command]
pub async fn ai_generate_media(state: State<'_, AppState>, req: MediaReq) -> CmdResult<String> {
    if req.kind.as_deref() == Some("video") {
        return Ok(generate_video_cmd(&state, &req).await);
    }
    if !setting_bool(&state, "aiMediaGen", false) {
        return Ok("Error: image generation is turned off. The user can enable it in Settings > AI.".into());
    }
    let Some(key) = state.api_key() else { return Ok("Error: not signed in.".into()) };
    let root = match state.sandbox.require_root(&req.project_root) {
        Ok(r) => r,
        Err(_) => return Ok("Error: no project folder is open.".into()),
    };
    let (list, _) = load_models(&state, false).await;
    let wanted = setting_str(&state, "imageModel");
    let supplier = setting_str(&state, "imageSupplier");
    let candidates = models::image_candidates(&list, &wanted);
    if candidates.is_empty() {
        return Ok("Error: no image model is available on your BlazeAPI account.".into());
    }
    // Try the chosen model, then fall back to the next ones when a supplier is
    // out of stock or failing, so one dead model never stops the agent.
    let mut failed: Vec<String> = Vec::new();
    let route = routing(&state);
    for model in candidates.iter().take(4) {
        // A forced supplier only applies to the model it was picked for.
        let forced = if model == &wanted && !supplier.is_empty() { Some(supplier.as_str()) } else { None };
        let job = models::ImageJob { model: Some(model.clone()), supplier: forced, routing: &route, root: &root, rel_path: &req.path, prompt: &req.prompt, aspect: req.aspect_ratio.as_deref() };
        let out = models::generate_image(&state.http, &state.v1_base, &key, job).await;
        let retryable = ["Error: image generation failed", "Error: BlazeAPI returned no image", "Error: could not download", "Error: the image data"].iter().any(|p| out.starts_with(p));
        if !retryable {
            return Ok(if failed.is_empty() { out } else { format!("{out} (Fell back after: {}.)", failed.join("; ")) });
        }
        failed.push(format!("{model} failed: {}", out.trim_start_matches("Error: ")));
    }
    Ok(format!("Error: no image model worked right now ({}). Continue without the image or try again later.", failed.join("; ")))
}

/// Animate a project image with the chosen (or the next working) video model.
async fn generate_video_cmd(state: &AppState, req: &MediaReq) -> String {
    if !setting_bool(state, "aiVideoGen", false) {
        return "Error: video generation is turned off. The user can enable it in Settings > AI.".into();
    }
    let Some(key) = state.api_key() else { return "Error: not signed in.".into() };
    let Ok(root) = state.sandbox.require_root(&req.project_root) else { return "Error: no project folder is open.".into() };
    // No image = text-to-video; with an image = animate it (image-to-video).
    let image = req.image.as_deref().filter(|s| !s.is_empty());
    let (list, _) = load_models(state, false).await;
    let candidates = models::video_candidates(&list, &setting_str(state, "videoModel"), image.is_some());
    if candidates.is_empty() {
        return "Error: no video model is available on your BlazeAPI account.".into();
    }
    let route = routing(state);
    let mut failed: Vec<String> = Vec::new();
    for model in candidates.iter().take(3) {
        let job = models::VideoJob { model, root: &root, image_rel: image, rel_path: &req.path, prompt: &req.prompt, routing: &route };
        let out = models::generate_video(&state.http, &state.v1_base, &key, job).await;
        if !out.starts_with("Error: video generation failed") {
            return if failed.is_empty() { out } else { format!("{out} (Fell back after: {}.)", failed.join("; ")) };
        }
        failed.push(format!("{model}: {}", out.trim_start_matches("Error: ")));
    }
    format!("Error: no video model worked right now ({}). Continue without the video or try again later.", failed.join("; "))
}

/// BlBots: draw an avatar with the user's BlazeAPI image model (one image,
/// billed to their balance; only on an explicit click). Returns a data: URL;
/// the page scales it down before saving it on the bot.
#[tauri::command]
pub async fn bots_avatar_image(state: State<'_, AppState>, prompt: String) -> CmdResult<String> {
    let prompt = prompt.trim();
    if prompt.chars().count() > 500 {
        return Err("Keep the description under 500 characters".into());
    }
    let Some(key) = state.api_key() else { return Err("Sign in to BlazeAPI to generate a picture".into()) };
    let (list, _) = load_models(&state, false).await;
    let candidates = models::image_candidates(&list, &setting_str(&state, "imageModel"));
    if candidates.is_empty() {
        return Err("No image model is available on your BlazeAPI account".into());
    }
    let full = format!(
        "A friendly avatar for an AI assistant bot{}. One simple character, centred, filling most of the square, plain soft background, no text.",
        if prompt.is_empty() { String::new() } else { format!(": {prompt}") }
    );
    let dir = std::env::temp_dir().join(format!("blaze-avatar-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).map_err(err)?;
    let route = routing(&state);
    let mut last = String::from("Error: no image");
    for model in candidates.iter().take(3) {
        let job = models::ImageJob { model: Some(model.clone()), supplier: None, routing: &route, root: &dir, rel_path: "avatar.png", prompt: &full, aspect: Some("1:1") };
        last = models::generate_image(&state.http, &state.v1_base, &key, job).await;
        if !last.starts_with("Error") {
            break;
        }
    }
    let file = std::fs::read_dir(&dir).ok().and_then(|mut d| d.find_map(|e| e.ok().map(|e| e.path())));
    let url = file.and_then(|p| std::fs::read(p).ok()).and_then(|b| models::image_data_url(&b));
    let _ = std::fs::remove_dir_all(&dir);
    url.ok_or_else(|| last.trim_start_matches("Error: ").to_string())
}

/// Suppliers (BlazeAPI codenames) that serve an image model, for Settings.
#[tauri::command]
pub async fn ai_image_suppliers(state: State<'_, AppState>, model: String) -> CmdResult<Vec<models::SupplierEntry>> {
    if model.is_empty() || model.len() > 200 {
        return Ok(Vec::new());
    }
    models::fetch_suppliers(&state.http, &state.api_base, &model).await.map_err(err)
}

// --- chats ----------------------------------------------------------------

#[tauri::command]
pub fn chat_list(state: State<'_, AppState>, project_root: String) -> CmdResult<Vec<chats::ChatSummary>> {
    let root = state.sandbox.require_root(&project_root).map_err(err)?;
    Ok(chats::list(&root))
}

#[tauri::command]
pub fn chat_load(state: State<'_, AppState>, project_root: String, id: String) -> CmdResult<Option<Value>> {
    let root = state.sandbox.require_root(&project_root).map_err(err)?;
    chats::load(&root, &id).map_err(err)
}

#[tauri::command]
pub fn chat_save(state: State<'_, AppState>, project_root: String, chat: Value) -> CmdResult<()> {
    let root = state.sandbox.require_root(&project_root).map_err(err)?;
    chats::save(&root, &chat).map_err(err)
}

#[tauri::command]
pub fn chat_delete(state: State<'_, AppState>, project_root: String, id: String) -> CmdResult<()> {
    let root = state.sandbox.require_root(&project_root).map_err(err)?;
    chats::delete(&root, &id).map_err(err)
}

// --- custom providers --------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderView {
    id: String,
    name: String,
    base_url: String,
    models: Vec<String>,
    /// The endpoint setting: "" (auto), "chat", "responses" or "anthropic".
    wire: String,
    /// The models that can see images.
    vision: Vec<String>,
    has_key: bool,
}

fn view(p: providers::CustomProvider) -> ProviderView {
    let has_key = secrets::load_provider_key(&p.id).is_some();
    let vision = p.models.iter().filter(|m| providers::sees_images(&p, m)).cloned().collect();
    ProviderView { id: p.id, name: p.name, base_url: p.base_url, models: p.models, wire: p.wire, vision, has_key }
}

#[tauri::command]
pub fn providers_list(state: State<'_, AppState>) -> Vec<ProviderView> {
    state.store.providers().into_iter().map(view).collect()
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInput {
    id: Option<String>,
    name: String,
    base_url: String,
    models: Vec<String>,
    #[serde(default)]
    wire: String,
    /// The user's own "sees images" choices, by model.
    #[serde(default)]
    vision: std::collections::BTreeMap<String, bool>,
    /// None leaves the stored key alone; Some("") removes it.
    api_key: Option<String>,
}

#[tauri::command]
pub async fn providers_save(state: State<'_, AppState>, input: ProviderInput) -> CmdResult<ProviderView> {
    let id = input.id.unwrap_or_default();
    let mut caps = state.store.providers().into_iter().find(|p| p.id == id).map(|p| p.caps).unwrap_or_default();
    for (model, sees) in input.vision {
        let c = caps.entry(model).or_default();
        c.vision = Some(sees);
        c.manual = true;
    }
    let mut p = providers::sanitize(providers::CustomProvider { id, name: input.name, base_url: input.base_url, models: input.models, wire: input.wire, caps }).map_err(err)?;
    if let Some(key) = input.api_key {
        let key = key.trim();
        if key.is_empty() {
            secrets::delete_provider_key(&p.id);
        } else if key.len() > 512 || key.chars().any(char::is_control) {
            return Err("That API key does not look right".into());
        } else {
            secrets::save_provider_key(&p.id, key)?;
        }
    }
    // Ask the endpoint what its models can do; saving works without it.
    let key = secrets::load_provider_key(&p.id);
    if let Ok(Ok(catalog)) = tokio::time::timeout(std::time::Duration::from_secs(8), providers::fetch_catalog(&state.http_custom, &p.base_url, key.as_deref())).await {
        providers::apply_catalog(&mut p, &catalog);
    }
    state.store.upsert_provider(p.clone()).map_err(err)?;
    Ok(view(p))
}

#[tauri::command]
pub fn providers_delete(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    secrets::delete_provider_key(&id);
    state.store.remove_provider(&id).map_err(err)
}

/// List the models an endpoint serves, for the "Fetch models" button. Uses
/// the typed key, or the stored one when editing an existing provider.
#[tauri::command]
pub async fn providers_fetch_models(state: State<'_, AppState>, base_url: String, api_key: Option<String>, id: Option<String>) -> CmdResult<Vec<String>> {
    let key = api_key.filter(|k| !k.trim().is_empty()).or_else(|| id.as_deref().and_then(secrets::load_provider_key));
    providers::fetch_models(&state.http_custom, &base_url, key.as_deref()).await.map_err(err)
}

// --- settings / shell / windows ---------------------------------------------

#[tauri::command]
pub fn settings_get(state: State<'_, AppState>) -> Map<String, Value> {
    state.store.settings()
}

#[tauri::command]
pub fn settings_set(state: State<'_, AppState>, patch: Map<String, Value>) -> CmdResult<()> {
    state.store.patch_settings(patch).map_err(err)
}

#[tauri::command]
pub fn open_external(app: AppHandle, url: String) -> CmdResult<()> {
    let parsed = url::Url::parse(&url).map_err(|_| "Not a valid link".to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("Only http(s) links can be opened".into());
    }
    app.opener().open_url(parsed.as_str(), None::<&str>).map_err(err)
}

#[tauri::command]
pub async fn pick_media(app: AppHandle, state: State<'_, AppState>, kind: String) -> CmdResult<Option<String>> {
    use blaze_core::media_store::{self, MediaKind};
    let mk = MediaKind::parse(&kind);
    let (title, name) = match mk {
        MediaKind::Video => ("Choose Video", "Video"),
        MediaKind::Audio => ("Choose Audio", "Audio"),
        MediaKind::Image => ("Choose Image", "Image"),
    };
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().set_title(title).add_filter(name, mk.extensions()).pick_file(move |p| {
        let _ = tx.send(p);
    });
    let Some(picked) = rx.await.ok().flatten() else { return Ok(None) };
    let path = picked.into_path().map_err(err)?;
    // Copy it into the app's media folder: the setting then survives restarts
    // and moving/deleting the original. Large videos take a moment.
    let dir = state.media_dir.clone();
    let stored = tauri::async_runtime::spawn_blocking(move || media_store::import(&dir, &path, mk)).await.map_err(err)?.map_err(err)?;
    // Drop copies nothing points at any more (keep the one just picked).
    let mut keep = media_store::referenced(&state.store.settings());
    keep.push(stored.clone());
    media_store::prune(&state.media_dir, &keep);
    Ok(Some(stored.display().to_string()))
}

// --- Continue on mobile ---------------------------------------------------

#[tauri::command]
pub fn mobile_status(app: AppHandle) -> Value {
    crate::mobile::status(&app)
}

#[tauri::command]
pub async fn mobile_set_password(app: AppHandle, state: State<'_, AppState>, password: String) -> CmdResult<Value> {
    let store_pw = password.clone();
    let hash = tauri::async_runtime::spawn_blocking(move || blaze_core::remote::auth::hash_password(&store_pw)).await.map_err(err)?.map_err(err)?;
    blaze_core::remote::auth::update(&state.store, |c| {
        c.password_hash = Some(hash);
        c.devices.clear();
        Ok(())
    })
    .map_err(err)?;
    crate::mobile::apply(&app).await;
    Ok(crate::mobile::status(&app))
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileConfigReq {
    enabled: bool,
    lan: bool,
    quick_tunnel: bool,
    relay: bool,
    #[serde(default)]
    domain: Option<String>,
    #[serde(default)]
    domain_tunnel: bool,
    #[serde(default)]
    port: Option<u16>,
}

#[tauri::command]
pub async fn mobile_configure(app: AppHandle, state: State<'_, AppState>, cfg: MobileConfigReq) -> CmdResult<Value> {
    let domain = match cfg.domain.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        None => None,
        Some(d) => {
            let u = url::Url::parse(d).map_err(|_| "Enter the domain as https://ide.example.com".to_string())?;
            let local = matches!(u.host_str(), Some("localhost") | Some("127.0.0.1"));
            if u.scheme() != "https" && !local {
                return Err("Your domain must use https://".into());
            }
            Some(format!("{}://{}", u.scheme(), u.host_str().unwrap_or("")) + &u.port().map(|p| format!(":{p}")).unwrap_or_default())
        }
    };
    if let Some(p) = cfg.port {
        if p < 1024 {
            return Err("Use a port of 1024 or higher".into());
        }
    }
    blaze_core::remote::auth::update(&state.store, |c| {
        if cfg.enabled && c.password_hash.is_none() {
            return Err(blaze_core::Error::msg("Choose a password first"));
        }
        c.enabled = cfg.enabled;
        c.lan = cfg.lan;
        c.quick_tunnel = cfg.quick_tunnel;
        c.relay = cfg.relay;
        c.domain = domain.clone();
        c.domain_tunnel = cfg.domain_tunnel && domain.is_some();
        if let Some(p) = cfg.port {
            c.port = p;
        }
        if c.relay && c.relay_slot.is_none() {
            c.relay_slot = Some(blaze_core::remote::relay::new_slot());
        }
        if !c.enabled {
            // Turning it off signs every phone out.
            c.devices.clear();
        }
        Ok(())
    })
    .map_err(err)?;
    crate::mobile::apply(&app).await;
    Ok(crate::mobile::status(&app))
}

#[tauri::command]
pub async fn mobile_set_tunnel_token(app: AppHandle, token: String) -> CmdResult<Value> {
    let t = token.trim();
    if t.is_empty() {
        crate::secrets::delete_secret(crate::mobile::TUNNEL_TOKEN);
    } else {
        if t.len() < 40 || t.len() > 4096 || !t.chars().all(|c| c.is_ascii_alphanumeric() || "-_=+/.".contains(c)) {
            return Err("That does not look like a Cloudflare tunnel token".into());
        }
        crate::secrets::save_secret(crate::mobile::TUNNEL_TOKEN, t)?;
    }
    crate::mobile::apply(&app).await;
    Ok(crate::mobile::status(&app))
}

#[tauri::command]
pub fn mobile_pair_qr(url: String) -> CmdResult<Value> {
    crate::mobile::pair_qr(&url)
}

#[tauri::command]
pub fn mobile_revoke(app: AppHandle, state: State<'_, AppState>, id: String) -> CmdResult<Value> {
    blaze_core::remote::auth::revoke_device(&state.store, &id).map_err(err)?;
    Ok(crate::mobile::status(&app))
}

#[tauri::command]
pub fn cli_status() -> crate::cli_install::CliStatus {
    crate::cli_install::status()
}

#[tauri::command]
pub async fn cli_install() -> CmdResult<String> {
    tauri::async_runtime::spawn_blocking(crate::cli_install::install).await.map_err(err)?
}

#[tauri::command]
pub fn win_new(app: AppHandle) -> CmdResult<()> {
    windows::open(&app, None).map_err(err)
}

#[tauri::command]
pub fn win_open_file(app: AppHandle, state: State<'_, AppState>, path: String, project_root: Option<String>) -> CmdResult<()> {
    let file = state.sandbox.require(&path).map_err(err)?;
    let mut q = vec![("file", file.display().to_string())];
    if let Some(root) = project_root.filter(|r| !r.is_empty()) {
        q.push(("root", state.sandbox.require_root(&root).map_err(err)?.display().to_string()));
    }
    windows::open(&app, Some(q)).map_err(err)
}

// ---- VS Code extensions (Open VSX): declarative parts only, no extension host ----

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketPage {
    items: Vec<extensions::MarketItem>,
    total: u64,
}

#[tauri::command]
pub async fn ext_search(state: State<'_, AppState>, query: String, category: Option<String>, offset: Option<u32>) -> CmdResult<MarketPage> {
    let (items, total) = extensions::search(&state.http, &query, category.as_deref(), offset.unwrap_or(0)).await.map_err(|e| e.to_string())?;
    Ok(MarketPage { items, total })
}

#[tauri::command]
pub async fn ext_details(state: State<'_, AppState>, namespace: String, name: String) -> CmdResult<extensions::Details> {
    extensions::details(&state.http, &namespace, &name).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn ext_install(state: State<'_, AppState>, namespace: String, name: String) -> CmdResult<extensions::Installed> {
    extensions::install(&state.http, &state.ext_dir, &namespace, &name).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub fn ext_uninstall(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    extensions::uninstall(&state.ext_dir, &id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn ext_list(state: State<'_, AppState>) -> Vec<extensions::Installed> {
    extensions::list_installed(&state.ext_dir)
}

#[tauri::command]
pub fn ext_icon_theme(state: State<'_, AppState>, ext_id: String, theme_id: String) -> CmdResult<extensions::IconThemeData> {
    extensions::icon_theme(&state.ext_dir, &ext_id, &theme_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn ext_contributions(state: State<'_, AppState>) -> extensions::Contributions {
    extensions::contributions(&state.ext_dir)
}
