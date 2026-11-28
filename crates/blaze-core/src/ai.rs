//! Streaming chat against BlazeAPI's OpenAI-compatible endpoint
//! (`/paid/v1/chat/completions`), including incremental tool calls.

use std::collections::BTreeMap;
use std::time::Duration;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::prompts::system_prompt;
use crate::tools::tools_for_opts;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra: Option<Map<String, Value>>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub role: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// The model's thinking for this turn (`reasoning_content`). Thinking
    /// models (DeepSeek, Kimi, GLM...) reject the next request of a tool loop
    /// unless it is sent back with the assistant's tool calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    /// Images attached to a user message (data: URLs): pasted pictures and
    /// the agent's browser screenshots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiRequest {
    pub request_id: String,
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub mode: String,
    #[serde(default)]
    pub filename: Option<String>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub media_gen: Option<bool>,
    /// Offer generate_video (set by the shell from the aiVideoGen setting).
    #[serde(default)]
    pub video_gen: Option<bool>,
    /// Leave out `reasoning_content` on replayed turns (set after a provider
    /// rejected it).
    #[serde(default)]
    pub no_reasoning_echo: bool,
    /// Extra system instructions appended to the built-in prompt (a bot's
    /// personality and memory, the CLI's environment). Never from the webview.
    #[serde(skip)]
    pub extra_system: Option<String>,
    /// Extra tool schemas (bot tools such as `remember` or `hand_off`).
    #[serde(skip)]
    pub extra_tools: Vec<Value>,
    /// Built-in tools to leave out (e.g. `browser` where there is no preview).
    #[serde(skip)]
    pub exclude_tools: Vec<String>,
}

/// Mirrors the UI's `AIStreamEvent` minus `requestId` (added by the caller).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    Text { text: String },
    Reasoning { text: String },
    ToolDelta {
        index: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(rename = "argsDelta")]
        args_delta: String,
    },
    ToolCall {
        #[serde(rename = "toolCall")]
        tool_call: ToolCall,
    },
    Usage {
        #[serde(rename = "inputTokens")]
        input_tokens: u64,
        #[serde(rename = "outputTokens")]
        output_tokens: u64,
    },
}

const MAX_MESSAGES: usize = 2000;
const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;

pub fn validate(req: &AiRequest) -> Result<()> {
    if req.request_id.is_empty() || req.request_id.len() > 100 {
        return Err(Error::msg("Malformed AI request"));
    }
    if !matches!(req.mode.as_str(), "ask" | "plan" | "agent") {
        return Err(Error::msg("Unknown AI mode"));
    }
    if req.model.trim().is_empty() || req.model.len() > 200 {
        return Err(Error::msg("Pick a model first"));
    }
    if req.messages.is_empty() || req.messages.len() > MAX_MESSAGES {
        return Err(Error::msg("Malformed AI request"));
    }
    if req.messages.iter().any(|m| !matches!(m.role.as_str(), "system" | "user" | "assistant" | "tool")) {
        return Err(Error::msg("Malformed AI request"));
    }
    Ok(())
}

/// Images kept per request: only the newest ones are sent (older ones become
/// a short note), so screenshots do not pile up in every later request.
const MAX_IMAGE_MESSAGES: usize = 3;
const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;

fn wire_message(m: &ChatMessage, echo_reasoning: bool, with_images: bool) -> Value {
    let mut w = json!({ "role": m.role, "content": m.content });
    if m.role == "user" {
        if let Some(imgs) = m.images.as_ref().filter(|v| !v.is_empty()) {
            if with_images {
                let mut parts = vec![json!({ "type": "text", "text": if m.content.is_empty() { "(image)" } else { m.content.as_str() } })];
                for url in imgs.iter().filter(|u| u.starts_with("data:image/") && u.len() <= MAX_IMAGE_BYTES) {
                    parts.push(json!({ "type": "image_url", "image_url": { "url": url } }));
                }
                w["content"] = Value::Array(parts);
            } else {
                w["content"] = json!(format!("{}\n[{} earlier image(s) omitted]", m.content, imgs.len()));
            }
        }
    }
    if echo_reasoning && m.role == "assistant" {
        if let Some(r) = m.reasoning.as_deref().filter(|r| !r.is_empty()) {
            w["reasoning_content"] = json!(r);
        }
    }
    if m.role == "tool" {
        if let Some(id) = &m.tool_call_id {
            w["tool_call_id"] = json!(id);
        }
    }
    if m.role == "assistant" {
        if let Some(calls) = m.tool_calls.as_ref().filter(|c| !c.is_empty()) {
            w["tool_calls"] = Value::Array(
                calls
                    .iter()
                    .map(|tc| {
                        let mut c = json!({ "id": tc.id, "type": "function", "function": { "name": tc.name, "arguments": tc.arguments } });
                        // Provider passthrough that must be echoed back (e.g.
                        // Gemini's thought signature in extra_content).
                        if let Some(extra) = &tc.extra {
                            for (k, v) in extra {
                                c[k] = v.clone();
                            }
                        }
                        c
                    })
                    .collect(),
            );
        }
    }
    w
}

pub fn build_body(req: &AiRequest) -> Value {
    let effort = req.reasoning_effort.clone().unwrap_or_else(|| "medium".into());
    let mut messages: Vec<Value> = Vec::with_capacity(req.messages.len() + 1);
    if !req.messages.iter().any(|m| m.role == "system") {
        let mut sys = system_prompt(&req.mode, req.filename.as_deref(), &effort);
        if let Some(extra) = req.extra_system.as_deref().filter(|e| !e.trim().is_empty()) {
            sys.push_str("\n\n");
            sys.push_str(extra);
        }
        messages.push(json!({ "role": "system", "content": sys }));
    }
    // Thinking is echoed back only for the assistant tool-call turns since the
    // last user message: that is what thinking models require, and older turns
    // would only cost tokens.
    let last_user = req.messages.iter().rposition(|m| m.role == "user" && !m.images.as_ref().is_some_and(|v| !v.is_empty())).unwrap_or(0);
    // Indices of the newest messages that carry images.
    let image_msgs: Vec<usize> = req.messages.iter().enumerate().filter(|(_, m)| m.images.as_ref().is_some_and(|v| !v.is_empty())).map(|(i, _)| i).collect();
    let keep_from = image_msgs.len().saturating_sub(MAX_IMAGE_MESSAGES);
    let keep: std::collections::HashSet<usize> = image_msgs[keep_from..].iter().copied().collect();
    messages.extend(req.messages.iter().enumerate().map(|(i, m)| {
        let echo = !req.no_reasoning_echo && i > last_user && m.tool_calls.as_ref().is_some_and(|c| !c.is_empty());
        wire_message(m, echo, keep.contains(&i))
    }));
    let mut body = json!({
        "model": req.model,
        "messages": messages,
        "stream": true,
        "stream_options": { "include_usage": true },
    });
    let mut tools = tools_for_opts(&req.mode, req.media_gen != Some(false), req.video_gen == Some(true));
    if let Some(t) = tools.as_mut() {
        t.retain(|tool| !req.exclude_tools.iter().any(|x| tool["function"]["name"] == x.as_str()));
        t.extend(req.extra_tools.iter().cloned());
    } else if !req.extra_tools.is_empty() {
        tools = Some(req.extra_tools.clone());
    }
    if let Some(tools) = tools.filter(|t| !t.is_empty()) {
        body["tools"] = Value::Array(tools);
    }
    if effort != "off" {
        body["reasoning_effort"] = json!(if effort == "max" { "high" } else { effort.as_str() });
    }
    body
}

#[derive(Default)]
struct Pending {
    id: String,
    name: String,
    args: String,
    extra: Option<Map<String, Value>>,
}

fn refuses_images(error: &str) -> bool {
    ["Unable to download the file", "Only HTTPS URLs are supported"].iter().any(|k| error.contains(k))
}

fn without_images(messages: &mut [ChatMessage]) {
    for m in messages {
        if let Some(n) = m.images.take().map(|v| v.len()).filter(|n| *n > 0) {
            m.content.push_str(&format!("\n[This provider could not accept {n} attached image(s), so you cannot see them. Say so if the answer depends on them.]"));
        }
    }
}

/// Incremental parser for the OpenAI SSE stream. Feed raw bytes, get events.
#[derive(Default)]
pub struct SseParser {
    buf: Vec<u8>,
    pending: BTreeMap<u32, Pending>,
    /// An error object the gateway sent inside the stream.
    pub error: Option<String>,
}

impl SseParser {
    pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<StreamEvent>) {
        self.buf.extend_from_slice(bytes);
        while let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end();
            if let Some(payload) = line.strip_prefix("data:") {
                let payload = payload.trim();
                if payload != "[DONE]" && !payload.is_empty() {
                    self.handle(payload, out);
                }
            }
        }
    }

    fn handle(&mut self, payload: &str, out: &mut Vec<StreamEvent>) {
        let Ok(v) = serde_json::from_str::<Value>(payload) else { return };
        if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
            if let (Some(i), Some(o)) = (u["prompt_tokens"].as_u64(), u["completion_tokens"].as_u64()) {
                out.push(StreamEvent::Usage { input_tokens: i, output_tokens: o });
            }
        }
        // A mid-stream error object from the gateway: remembered so the caller
        // can report it as an error (and the agent can retry), not as text.
        if let Some(msg) = v.get("error").and_then(|e| e.get("message").and_then(Value::as_str).or_else(|| e.as_str())) {
            self.error = Some(msg.to_string());
            return;
        }
        let Some(delta) = v.get("choices").and_then(|c| c.get(0)).and_then(|c| c.get("delta")) else { return };
        for k in ["reasoning_content", "reasoning"] {
            if let Some(t) = delta.get(k).and_then(Value::as_str).filter(|t| !t.is_empty()) {
                out.push(StreamEvent::Reasoning { text: t.to_string() });
                break;
            }
        }
        if let Some(t) = delta.get("content").and_then(Value::as_str).filter(|t| !t.is_empty()) {
            out.push(StreamEvent::Text { text: t.to_string() });
        }
        let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) else { return };
        for entry in calls {
            let index = entry.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;
            let slot = self.pending.entry(index).or_default();
            let mut id_delta = None;
            if let Some(id) = entry.get("id").and_then(Value::as_str).filter(|s| !s.is_empty() && *s != slot.id) {
                slot.id = id.to_string();
                id_delta = Some(id.to_string());
            }
            let mut name_delta = None;
            let mut args_delta = String::new();
            if let Some(f) = entry.get("function") {
                if let Some(n) = f.get("name").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                    slot.name.push_str(n);
                    name_delta = Some(n.to_string());
                }
                if let Some(a) = f.get("arguments").and_then(Value::as_str) {
                    slot.args.push_str(a);
                    args_delta = a.to_string();
                }
            }
            if let Some(extra) = entry.get("extra_content").and_then(Value::as_object) {
                let merged = slot.extra.get_or_insert_with(Map::new);
                let ec = merged.entry("extra_content").or_insert_with(|| json!({}));
                if let Some(obj) = ec.as_object_mut() {
                    for (k, v) in extra {
                        obj.insert(k.clone(), v.clone());
                    }
                }
            }
            if id_delta.is_some() || name_delta.is_some() || !args_delta.is_empty() {
                out.push(StreamEvent::ToolDelta { index, id: id_delta, name: name_delta, args_delta });
            }
        }
    }

    /// Completed tool calls, in index order. Call once the stream ends.
    pub fn finish(&mut self) -> Vec<StreamEvent> {
        std::mem::take(&mut self.pending)
            .into_values()
            .filter(|p| !p.name.is_empty())
            .map(|p| StreamEvent::ToolCall {
                tool_call: ToolCall {
                    id: if p.id.is_empty() { format!("call_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]) } else { p.id },
                    name: p.name,
                    arguments: if p.args.is_empty() { "{}".into() } else { p.args },
                    extra: p.extra,
                },
            })
            .collect()
    }
}

/// Where a chat request goes: BlazeAPI, or a user-added OpenAI-compatible
/// provider. Only BlazeAPI gets the product-specific headers and messages.
pub struct Target<'a> {
    pub url: String,
    pub key: Option<&'a str>,
    pub branded: bool,
    /// Extra headers (BlazeAPI routing choice).
    pub headers: Vec<(&'static str, String)>,
}

/// BlazeAPI routing headers for a routing choice: "normal", "direct",
/// "cheapest" (Normal routing, cheapest supplier first); anything else = the
/// account's own setting (no header).
pub fn routing_headers(routing: &str) -> Vec<(&'static str, String)> {
    match routing {
        "normal" => vec![("X-Blaze-Routing", "basic".into())],
        "direct" => vec![("X-Blaze-Routing", "trusted".into())],
        "cheapest" => vec![("X-Blaze-Routing", "basic".into()), ("X-Blaze-Provider", "cheapest".into())],
        _ => Vec::new(),
    }
}

impl<'a> Target<'a> {
    pub fn blaze(api_base: &str, key: &'a str) -> Self {
        Target { url: format!("{api_base}/paid/v1/chat/completions"), key: Some(key), branded: true, headers: Vec::new() }
    }
    pub fn blaze_routed(api_base: &str, key: &'a str, routing: &str) -> Self {
        Target { headers: routing_headers(routing), ..Self::blaze(api_base, key) }
    }
    pub fn custom(base_url: &str, key: Option<&'a str>) -> Self {
        Target { url: format!("{}/chat/completions", base_url.trim_end_matches('/')), key, branded: false, headers: Vec::new() }
    }
}

fn friendly_http_error(status: u16, text: &str, branded: bool) -> Error {
    let parsed: Option<Value> = serde_json::from_str(text).ok();
    let msg = parsed.as_ref().and_then(|v| {
        v.get("error").and_then(|e| e.get("message").and_then(Value::as_str).map(str::to_string).or_else(|| e.as_str().map(str::to_string)))
    });
    let message = match (status, msg) {
        (401, _) if branded => "Your BlazeAPI session has expired. Sign in again.".to_string(),
        (401, _) | (403, _) => "The provider rejected the API key. Check it in Settings > Custom providers.".to_string(),
        (402, m) if branded => m.unwrap_or_else(|| "Not enough BlazeAPI balance for this model. Top up in your BlazeAPI dashboard.".into()),
        (429, m) if branded => m.unwrap_or_else(|| "Rate limited by BlazeAPI. Wait a moment and try again.".into()),
        (429, m) => m.unwrap_or_else(|| "Rate limited by the provider. Wait a moment and try again.".into()),
        (_, Some(m)) => m,
        (s, None) if s >= 500 => "This model's provider is having trouble right now. Try again, or switch models.".into(),
        (s, None) => format!("Request failed (HTTP {s})"),
    };
    Error::Http { status, message }
}

/// Run one streaming request. Events go to `emit`; returns when the stream
/// ends, fails, or `cancel` fires (cancel returns Ok).
pub async fn stream_chat(
    http: &reqwest::Client,
    target: &Target<'_>,
    req: &AiRequest,
    cancel: CancellationToken,
    mut emit: impl FnMut(StreamEvent),
) -> Result<()> {
    validate(req)?;
    let mut body = serde_json::to_vec(&build_body(req))?;
    if body.len() > MAX_BODY_BYTES {
        return Err(Error::msg("This conversation is too large to send. Start a new chat."));
    }
    let url = &target.url;
    let mut echo_dropped = req.no_reasoning_echo;
    let mut images_dropped = !req.messages.iter().any(|m| m.images.as_ref().is_some_and(|v| !v.is_empty()));

    // Retry connection failures and 5xx a couple of times, but only before any
    // output has been read, so nothing is ever duplicated.
    let mut resp = None;
    let mut attempt = 0u64;
    while attempt < 3 {
        if cancel.is_cancelled() {
            return Ok(());
        }
        let mut post = http.post(url).header("Content-Type", "application/json").body(body.clone());
        if let Some(k) = target.key {
            post = post.bearer_auth(k);
        }
        if target.branded {
            post = post.header("X-Client", "blaze-ide");
        }
        for (k, v) in &target.headers {
            post = post.header(*k, v.as_str());
        }
        let send = post.send();
        let r = tokio::select! {
            r = send => r,
            _ = cancel.cancelled() => return Ok(()),
        };
        match r {
            Ok(r) if r.status().is_success() => {
                resp = Some(r);
                break;
            }
            Ok(r) if r.status().is_server_error() && attempt < 2 => {
                tokio::time::sleep(Duration::from_millis(400 * (attempt + 1))).await;
            }
            Ok(r) => {
                let status = r.status().as_u16();
                let text = r.text().await.unwrap_or_default();
                // A provider that refuses the echoed thinking: send once more without it.
                if status == 400 && !echo_dropped && text.contains("reasoning") {
                    echo_dropped = true;
                    let mut plain = req.clone();
                    plain.no_reasoning_echo = true;
                    body = serde_json::to_vec(&build_body(&plain))?;
                    attempt += 1;
                    continue;
                }
                // Some gateways cannot pass pictures on. The pictures stay in
                // the history, so without this every later message would fail.
                if status == 400 && !images_dropped && refuses_images(&text) {
                    images_dropped = true;
                    let mut plain = req.clone();
                    plain.no_reasoning_echo = echo_dropped;
                    without_images(&mut plain.messages);
                    body = serde_json::to_vec(&build_body(&plain))?;
                    continue;
                }
                return Err(friendly_http_error(status, &text, target.branded));
            }
            // is_request: the request could not be sent (dropped/stale connection),
            // so nothing reached the server and sending again is safe.
            Err(e) if attempt < 2 && (e.is_connect() || e.is_timeout() || e.is_request()) => {
                tokio::time::sleep(Duration::from_millis(400 * (attempt + 1))).await;
            }
            Err(e) => return Err(e.into()),
        }
        attempt += 1;
    }
    let Some(resp) = resp else {
        return Err(Error::msg(if target.branded {
            "Cannot reach BlazeAPI. Check your connection and try again."
        } else {
            "Cannot reach the provider. Check the URL and your connection."
        }));
    };

    let mut parser = SseParser::default();
    let mut stream = resp.bytes_stream();
    let mut events = Vec::new();
    let mut dropped = false;
    loop {
        let next = tokio::select! {
            n = stream.next() => n,
            _ = cancel.cancelled() => return Ok(()),
        };
        match next {
            None => break,
            Some(Ok(chunk)) => {
                parser.feed(&chunk, &mut events);
                for ev in events.drain(..) {
                    emit(ev);
                }
            }
            Some(Err(_)) => {
                dropped = true;
                break;
            }
        }
    }
    let calls = parser.finish();
    let recovered = !calls.is_empty();
    for ev in calls {
        emit(ev);
    }
    if let Some(msg) = parser.error.take().filter(|_| !recovered) {
        return Err(Error::msg(msg));
    }
    if dropped && !recovered {
        return Err(Error::msg("The response was cut off (the connection dropped during a long generation). Try again, or ask for smaller steps."));
    }
    Ok(())
}


const COMPLETION_SYSTEM: &str = "You are an inline code completion engine inside an editor. The user text contains the code before the cursor, the marker <CURSOR>, then the code after it. Reply with ONLY the text to insert at <CURSOR>: no explanation, no markdown fences, no repetition of the surrounding code. Keep it short (usually the rest of the current line or a few lines). If nothing sensible fits, reply with nothing.";

/// Remove the wrapping a chat model tends to add around a completion.
pub fn clean_completion(raw: &str) -> String {
    let mut t = raw.trim_matches(|c| c == '\r' || c == '\n').to_string();
    if let Some(rest) = t.strip_prefix("```") {
        // ```lang\n...\n```
        let body = rest.split_once('\n').map(|(_, b)| b).unwrap_or("");
        t = body.trim_end().trim_end_matches("```").trim_end_matches('\n').to_string();
    }
    t = t.replace("<CURSOR>", "");
    if t.chars().count() > 1200 {
        t = t.chars().take(1200).collect();
    }
    if t.trim().is_empty() { String::new() } else { t }
}

/// One non-streaming completion for the editor's ghost text. `prefix` and
/// `suffix` are already trimmed by the caller to a window around the cursor.
pub async fn complete(http: &reqwest::Client, target: &Target<'_>, model: &str, filename: Option<&str>, prefix: &str, suffix: &str) -> Result<String> {
    let user = format!("{}{prefix}<CURSOR>{suffix}", filename.map(|f| format!("File: {f}\n\n")).unwrap_or_default());
    let body = json!({
        "model": model,
        "messages": [
            { "role": "system", "content": COMPLETION_SYSTEM },
            { "role": "user", "content": user },
        ],
        "max_tokens": 160,
        "temperature": 0,
        "stream": false,
    });
    let mut post = http.post(&target.url).json(&body).timeout(Duration::from_secs(10));
    if let Some(k) = target.key {
        post = post.bearer_auth(k);
    }
    if target.branded {
        post = post.header("X-Client", "blaze-ide");
    }
    for (k, v) in &target.headers {
        post = post.header(*k, v.as_str());
    }
    let resp = post.send().await.map_err(|e| Error::msg(format!("completion failed: {e}")))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(friendly_http_error(status.as_u16(), &text, target.branded));
    }
    let v: Value = serde_json::from_str(&text).unwrap_or_default();
    Ok(clean_completion(v["choices"][0]["message"]["content"].as_str().unwrap_or("")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(mode: &str) -> AiRequest {
        AiRequest {
            request_id: "r1".into(),
            model: "glm-5.3".into(),
            messages: vec![ChatMessage { role: "user".into(), content: "hi".into(), tool_call_id: None, tool_calls: None, reasoning: None, images: None }],
            mode: mode.into(),
            filename: None,
            reasoning_effort: Some("off".into()),
            media_gen: None,
            video_gen: None,
            no_reasoning_echo: false,
            ..Default::default()
        }
    }

    #[test]
    fn body_shape() {
        let b = build_body(&req("ask"));
        assert_eq!(b["messages"][0]["role"], "system");
        assert!(b.get("tools").is_none());
        assert!(b.get("reasoning_effort").is_none());
        // Extra system text and tools (bots, CLI) and leaving out built-ins.
        let mut r = req("agent");
        r.extra_system = Some("You are Nova.".into());
        r.extra_tools = vec![json!({"type":"function","function":{"name":"remember","parameters":{"type":"object"}}})];
        r.exclude_tools = vec!["browser".into()];
        let b = build_body(&r);
        assert!(b["messages"][0]["content"].as_str().unwrap().ends_with("You are Nova."));
        let names: Vec<&str> = b["tools"].as_array().unwrap().iter().map(|t| t["function"]["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"remember") && !names.contains(&"browser") && names.contains(&"read_file"));
        let mut a = req("ask");
        a.extra_tools = r.extra_tools.clone();
        assert_eq!(build_body(&a)["tools"].as_array().unwrap().len(), 1);
        let mut r = req("agent");
        r.reasoning_effort = Some("max".into());
        let b = build_body(&r);
        assert!(b["tools"].as_array().unwrap().len() >= 7);
        assert_eq!(b["reasoning_effort"], "high");
        assert_eq!(b["stream_options"]["include_usage"], true);
    }

    #[test]
    fn echoes_thinking_only_on_current_tool_turns() {
        let call = ToolCall { id: "c1".into(), name: "read_file".into(), arguments: "{}".into(), extra: None };
        let mut r = req("agent");
        r.messages = vec![
            ChatMessage { role: "user".into(), content: "a".into(), tool_call_id: None, tool_calls: None, reasoning: None, images: None },
            ChatMessage { role: "assistant".into(), content: "".into(), tool_call_id: None, tool_calls: Some(vec![call.clone()]), reasoning: Some("old".into()), images: None },
            ChatMessage { role: "tool".into(), content: "x".into(), tool_call_id: Some("c1".into()), tool_calls: None, reasoning: None, images: None },
            ChatMessage { role: "user".into(), content: "b".into(), tool_call_id: None, tool_calls: None, reasoning: None, images: None },
            ChatMessage { role: "assistant".into(), content: "".into(), tool_call_id: None, tool_calls: Some(vec![call]), reasoning: Some("new".into()), images: None },
        ];
        let b = build_body(&r);
        let msgs = b["messages"].as_array().unwrap();
        assert!(msgs[2].get("reasoning_content").is_none());
        assert_eq!(msgs[5]["reasoning_content"], "new");
        r.no_reasoning_echo = true;
        assert!(build_body(&r)["messages"][5].get("reasoning_content").is_none());
    }

    #[test]
    fn images_become_content_parts_newest_only() {
        let img = |n: &str| ChatMessage { role: "user".into(), content: n.into(), tool_call_id: None, tool_calls: None, reasoning: None, images: Some(vec!["data:image/png;base64,AAAA".into()]) };
        let mut r = req("agent");
        r.messages = vec![img("a"), img("b"), img("c"), img("d")];
        let b = build_body(&r);
        let m = b["messages"].as_array().unwrap();
        assert!(m[1]["content"].as_str().unwrap().contains("earlier image(s) omitted"));
        assert_eq!(m[4]["content"][1]["image_url"]["url"], "data:image/png;base64,AAAA");
        assert_eq!(m[4]["content"][0]["text"], "d");
        let names: Vec<String> = b["tools"].as_array().unwrap().iter().map(|t| t["function"]["name"].as_str().unwrap().to_string()).collect();
        assert!(names.contains(&"browser".to_string()) && !names.contains(&"generate_video".to_string()));
        r.video_gen = Some(true);
        assert!(build_body(&r)["tools"].as_array().unwrap().iter().any(|t| t["function"]["name"] == "generate_video"));
    }

    #[test]
    fn stream_errors_and_reasoning() {
        let mut p = SseParser::default();
        let mut out = Vec::new();
        p.feed(b"data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"hmm\"}}]}\n\ndata: {\"error\":{\"message\":\"upstream busy\"}}\n\n", &mut out);
        assert_eq!(out, vec![StreamEvent::Reasoning { text: "hmm".into() }]);
        assert_eq!(p.error.as_deref(), Some("upstream busy"));
        assert_eq!(routing_headers("direct"), vec![("X-Blaze-Routing", "trusted".to_string())]);
        assert_eq!(routing_headers("cheapest").len(), 2);
        assert!(routing_headers("").is_empty());
    }

    #[test]
    fn cleans_completions() {
        assert_eq!(clean_completion("```ts\nconst a = 1;\n```"), "const a = 1;");
        assert_eq!(clean_completion("\nfoo(<CURSOR>)\n"), "foo()");
        assert_eq!(clean_completion("   \n"), "");
        assert_eq!(clean_completion("return x;"), "return x;");
    }

    #[test]
    fn targets() {
        let t = Target::custom("http://localhost:11434/v1/", Some("k"));
        assert_eq!(t.url, "http://localhost:11434/v1/chat/completions");
        assert!(!t.branded);
        assert_eq!(Target::blaze("https://blazeapi.org", "k").url, "https://blazeapi.org/paid/v1/chat/completions");
    }

    #[test]
    fn validation() {
        assert!(validate(&req("ask")).is_ok());
        assert!(validate(&req("hack")).is_err());
        let mut r = req("ask");
        r.messages[0].role = "root".into();
        assert!(validate(&r).is_err());
    }

    #[test]
    fn parses_text_tools_usage_across_chunks() {
        let stream = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"pa\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"th\\\":\\\"a\\\"}\"}}]}}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5}}\n\n",
            "data: [DONE]\n\n"
        );
        let mut p = SseParser::default();
        let mut out = Vec::new();
        // Feed in awkward 7-byte pieces to exercise line reassembly.
        for chunk in stream.as_bytes().chunks(7) {
            p.feed(chunk, &mut out);
        }
        out.extend(p.finish());
        let text: String = out.iter().filter_map(|e| if let StreamEvent::Text { text } = e { Some(text.as_str()) } else { None }).collect();
        assert_eq!(text, "Hello");
        assert!(out.contains(&StreamEvent::Usage { input_tokens: 10, output_tokens: 5 }));
        let call = out.iter().find_map(|e| if let StreamEvent::ToolCall { tool_call } = e { Some(tool_call.clone()) } else { None }).unwrap();
        assert_eq!((call.id.as_str(), call.name.as_str(), call.arguments.as_str()), ("c1", "read_file", "{\"path\":\"a\"}"));
        let ser = serde_json::to_value(&out[2]).unwrap();
        assert_eq!(ser["type"], "tool_delta");
        assert_eq!(ser["argsDelta"], "{\"pa");
    }

    #[test]
    fn http_errors_are_friendly() {
        assert!(friendly_http_error(401, "", true).to_string().contains("Sign in again"));
        assert_eq!(friendly_http_error(400, "{\"error\":{\"message\":\"bad model\"}}", true).to_string(), "bad model");
        assert!(friendly_http_error(502, "<html>", true).to_string().contains("provider"));
        assert_eq!(friendly_http_error(402, "", true).status(), Some(402));
        assert!(friendly_http_error(401, "", false).to_string().contains("rejected the API key"));
    }

    #[tokio::test]
    async fn streams_from_a_local_server() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 65536];
            let n = s.read(&mut buf).await.unwrap();
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            assert!(req.contains("authorization: Bearer test-key") || req.contains("Authorization: Bearer test-key"));
            let body = "data: {\"choices\":[{\"delta\":{\"content\":\"pong\"}}]}\n\ndata: [DONE]\n\n";
            let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
            s.write_all(resp.as_bytes()).await.unwrap();
        });
        let http = reqwest::Client::new();
        let mut got = Vec::new();
        stream_chat(&http, &Target::blaze(&format!("http://127.0.0.1:{port}"), "test-key"), &req("ask"), CancellationToken::new(), |e| got.push(e)).await.unwrap();
        assert_eq!(got, vec![StreamEvent::Text { text: "pong".into() }]);
    }

    #[test]
    fn providers_that_refuse_pictures_get_the_text_only() {
        assert!(refuses_images(r#"{"error":{"message":"anthropic_: {\"message\":\"Only HTTPS URLs are supported.\"}"}}"#));
        assert!(refuses_images("Unable to download the file. Please verify the URL and try again."));
        assert!(!refuses_images("context length exceeded"));
        let mut m = vec![ChatMessage { role: "user".into(), content: "look".into(), images: Some(vec!["data:image/png;base64,AA".into()]), ..Default::default() }];
        without_images(&mut m);
        assert!(m[0].images.is_none() && m[0].content.starts_with("look\n[This provider could not accept 1"));
    }
}
