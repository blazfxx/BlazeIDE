//! The web tools for the IDE chat (bots, the CLI and phone turns run theirs
//! inside the core agent).

use blaze_core::ai::ToolCall;
use blaze_core::web::{tool, Policy};
use serde_json::{json, Value};
use tauri::State;

use crate::AppState;

/// The IDE chat has one browser.
pub const SESSION: &str = "ide";

pub fn enabled(state: &AppState) -> bool {
    state.store.settings().get("webAccess").and_then(Value::as_bool).unwrap_or(true)
}

#[tauri::command]
pub async fn web_tool(state: State<'_, AppState>, name: String, arguments: String) -> Result<Value, String> {
    if !enabled(&state) {
        return Ok(json!({ "text": "Error: web access is turned off in Settings.", "images": [] }));
    }
    if !tool::NAMES.contains(&name.as_str()) {
        return Err("Not a web tool".into());
    }
    let out = tool::run(&Policy::default(), SESSION, &name, &arguments).await;
    Ok(json!({ "text": out.text, "images": out.images }))
}

/// Why "Auto" asks about this call (None = it may run).
#[tauri::command]
pub fn approval_risk(name: String, arguments: String) -> Option<String> {
    let call = ToolCall { name, arguments, ..Default::default() };
    if call.name == "web_interact" {
        return tool::needs_approval(blaze_core::agent::Approval::Auto, SESSION, &call).then(|| "it could send, buy, delete or sign something on a website".to_string());
    }
    blaze_core::agent::risk_of(&call).map(str::to_string)
}
