//! Anonymous usage counts, strictly opt-in (Settings > Privacy, or the
//! onboarding question). Never sent unless `analytics` is true in settings.
//!
//! What is sent: a random install id, the app version, the platform and the
//! event name (app_open, sign_in, ai_request, update_installed), plus the
//! BlazeAPI model id for ai_request. Never file names, file contents,
//! prompts, paths or the names of custom providers.

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::AppState;

fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "mac"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    }
}

pub fn enabled(state: &AppState) -> bool {
    state.store.settings().get("analytics").and_then(Value::as_bool) == Some(true)
}

/// Fire and forget; a failure is ignored.
pub fn track(app: &AppHandle, event: &'static str, model: Option<&str>) {
    let state = app.state::<AppState>();
    if !enabled(&state) {
        return;
    }
    let mut body = json!({
        "install": state.store.install_id(),
        "v": app.package_info().version.to_string(),
        "platform": platform(),
        "event": event,
    });
    if let Some(m) = model.filter(|m| !m.is_empty() && m.len() <= 100) {
        body["model"] = json!(m);
    }
    let http = state.http.clone();
    let url = format!("{}/api/ide/events", state.api_base);
    tauri::async_runtime::spawn(async move {
        let _ = http.post(url).json(&body).timeout(std::time::Duration::from_secs(5)).send().await;
    });
}
