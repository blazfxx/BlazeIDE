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
}

fn session_from(v: &Value) -> SessionPublic {
    SessionPublic {
        user_email: v.get("email").and_then(Value::as_str).unwrap_or("").to_string(),
        username: v.get("username").and_then(Value::as_str).unwrap_or("").to_string(),
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

/// Where a model id is served from. `custom:<id>/<model>` is a provider the
/// user added; everything else is BlazeAPI.
enum Route {
    Blaze { key: Option<String> },
    Custom { base: String, key: Option<String>, model: String },
}

fn route_for(state: &AppState, model: &str) -> Result<Route, String> {
    match providers::split_model_id(model) {
        None => Ok(Route::Blaze { key: state.api_key() }),
        Some((pid, m)) => {
            let p = state.store.providers().into_iter().find(|p| p.id == pid).ok_or("That custom provider was removed. Pick another model.")?;
            Ok(Route::Custom { base: p.base_url, key: secrets::load_provider_key(pid), model: m.to_string() })
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
            ai::stream_chat(&state.http, &ai::Target::blaze(&state.v1_base, key), &req, cancel, |ev| send(with_id(&ev))).await
        }
        Route::Custom { base, key, model } => {
            analytics::track(&app, "ai_request", None);
            // Third-party servers may reject OpenAI's newer parameters, so
            // send the plain request.
            req.model = model.clone();
            req.reasoning_effort = Some("off".into());
            ai::stream_chat(&state.http_custom, &ai::Target::custom(base, key.as_deref()), &req, cancel, |ev| send(with_id(&ev))).await
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
        match models::fetch_models(&state.http, &state.v1_base, key.as_deref()).await {
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
            Route::Blaze { key: Some(key) } => ai::complete(&state.http, &ai::Target::blaze(&state.v1_base, &key), &model, name, prefix, suffix).await,
            Route::Blaze { key: None } => return Ok(String::new()),
            Route::Custom { base, key, model } => ai::complete(&state.http_custom, &ai::Target::custom(&base, key.as_deref()), &model, name, prefix, suffix).await,
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
    prompt: String,
    path: String,
    aspect_ratio: Option<String>,
    project_root: String,
}

#[tauri::command]
pub async fn ai_generate_media(state: State<'_, AppState>, req: MediaReq) -> CmdResult<String> {
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
    let model = if list.iter().any(|m| m.id == wanted && m.kind == "image") { Some(wanted) } else { models::pick_image_model(&list) };
    let job = models::ImageJob { model, root: &root, rel_path: &req.path, prompt: &req.prompt, aspect: req.aspect_ratio.as_deref() };
    Ok(models::generate_image(&state.http, &state.v1_base, &key, job).await)
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
    has_key: bool,
}

fn view(p: providers::CustomProvider) -> ProviderView {
    let has_key = secrets::load_provider_key(&p.id).is_some();
    ProviderView { id: p.id, name: p.name, base_url: p.base_url, models: p.models, has_key }
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
    /// None leaves the stored key alone; Some("") removes it.
    api_key: Option<String>,
}

#[tauri::command]
pub fn providers_save(state: State<'_, AppState>, input: ProviderInput) -> CmdResult<ProviderView> {
    let p = providers::sanitize(providers::CustomProvider { id: input.id.unwrap_or_default(), name: input.name, base_url: input.base_url, models: input.models }).map_err(err)?;
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
    let (title, name, exts): (&str, &str, &[&str]) = match kind.as_str() {
        "video" => ("Choose Video", "Video", &["mp4", "webm", "mov", "m4v", "ogv"]),
        "audio" => ("Choose Audio", "Audio", &["mp3", "ogg", "oga", "wav", "m4a", "aac", "flac"]),
        _ => ("Choose Image", "Image", &["png", "jpg", "jpeg", "webp", "gif", "avif", "bmp"]),
    };
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().set_title(title).add_filter(name, exts).pick_file(move |p| {
        let _ = tx.send(p);
    });
    let Some(picked) = rx.await.ok().flatten() else { return Ok(None) };
    let path = picked.into_path().map_err(err)?;
    let canon = state.sandbox.grant_picked_file(&path).map_err(err)?;
    Ok(Some(canon.display().to_string()))
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
pub fn ext_contributions(state: State<'_, AppState>) -> extensions::Contributions {
    extensions::contributions(&state.ext_dir)
}
