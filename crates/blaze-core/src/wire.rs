//! Other wire formats for custom providers. Blaze builds every request in the
//! OpenAI chat-completions shape; for a provider that only speaks (or only
//! allows tools with) another endpoint, that body is translated here and the
//! provider's stream is turned back into the same [`StreamEvent`]s.
//!
//!   chat       POST <base>/chat/completions   (OpenAI)
//!   responses  POST <base>/responses          (OpenAI Responses API)
//!   anthropic  POST <base>/messages           (Anthropic Messages)

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::ai::{finish_calls, Pending, StreamEvent};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wire {
    Chat,
    Responses,
    Anthropic,
}

impl Wire {
    /// "auto" and "" are not a wire: they mean "find out" (None).
    pub fn parse(s: &str) -> Option<Wire> {
        match s.trim().to_ascii_lowercase().as_str() {
            "chat" | "chat_completions" | "chat-completions" | "openai" => Some(Wire::Chat),
            "responses" | "openai-responses" => Some(Wire::Responses),
            "anthropic" | "messages" => Some(Wire::Anthropic),
            _ => None,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Wire::Chat => "chat",
            Wire::Responses => "responses",
            Wire::Anthropic => "anthropic",
        }
    }

    pub fn url(self, base: &str) -> String {
        let base = base.trim_end_matches('/');
        match self {
            Wire::Chat => format!("{base}/chat/completions"),
            Wire::Responses => format!("{base}/responses"),
            // Anthropic bases are often given without /v1 (ANTHROPIC_BASE_URL style).
            Wire::Anthropic if base.ends_with("/v1") || base.contains("/v1/") => format!("{base}/messages"),
            Wire::Anthropic => format!("{base}/v1/messages"),
        }
    }
}

/// Is `s` something a provider setting may hold?
pub fn valid_setting(s: &str) -> bool {
    matches!(s.trim().to_ascii_lowercase().as_str(), "" | "auto" | "chat" | "responses" | "anthropic")
}

fn text_of(content: &Value) -> String {
    match content {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

fn image_url(part: &Value) -> Option<&str> {
    part.get("image_url").and_then(|u| u.get("url").and_then(Value::as_str).or_else(|| u.as_str()))
}

/// `data:image/png;base64,AAA` -> ("image/png", "AAA")
fn split_data_url(u: &str) -> Option<(&str, &str)> {
    let rest = u.strip_prefix("data:")?;
    let (head, data) = rest.split_once(',')?;
    Some((head.strip_suffix(";base64")?, data))
}

fn arguments_object(args: &str) -> Value {
    serde_json::from_str::<Value>(if args.trim().is_empty() { "{}" } else { args }).ok().filter(Value::is_object).unwrap_or_else(|| json!({}))
}

fn chat_tools(chat: &Value) -> impl Iterator<Item = &Value> {
    chat.get("tools").and_then(Value::as_array).into_iter().flatten().filter_map(|t| t.get("function"))
}

// ------------------------------------------------------------------ Responses

pub fn to_responses(chat: &Value) -> Value {
    let mut instructions: Vec<String> = Vec::new();
    let mut input: Vec<Value> = Vec::new();
    for m in chat.get("messages").and_then(Value::as_array).into_iter().flatten() {
        let content = m.get("content").unwrap_or(&Value::Null);
        match m.get("role").and_then(Value::as_str).unwrap_or("") {
            "system" | "developer" => {
                let t = text_of(content);
                if !t.trim().is_empty() {
                    instructions.push(t);
                }
            }
            "user" => {
                let parts: Vec<Value> = match content {
                    Value::Array(ps) => ps
                        .iter()
                        .filter_map(|p| match p.get("type").and_then(Value::as_str) {
                            Some("text") => Some(json!({ "type": "input_text", "text": p.get("text").and_then(Value::as_str).unwrap_or("") })),
                            Some("image_url") => image_url(p).map(|u| json!({ "type": "input_image", "image_url": u })),
                            _ => None,
                        })
                        .collect(),
                    other => vec![json!({ "type": "input_text", "text": text_of(other) })],
                };
                if !parts.is_empty() {
                    input.push(json!({ "role": "user", "content": parts }));
                }
            }
            "assistant" => {
                let t = text_of(content);
                if !t.trim().is_empty() {
                    input.push(json!({ "role": "assistant", "content": [{ "type": "output_text", "text": t }] }));
                }
                for tc in m.get("tool_calls").and_then(Value::as_array).into_iter().flatten() {
                    input.push(json!({
                        "type": "function_call",
                        "call_id": tc.get("id").and_then(Value::as_str).unwrap_or(""),
                        "name": tc.pointer("/function/name").and_then(Value::as_str).unwrap_or(""),
                        "arguments": tc.pointer("/function/arguments").and_then(Value::as_str).unwrap_or("{}"),
                    }));
                }
            }
            "tool" => input.push(json!({ "type": "function_call_output", "call_id": m.get("tool_call_id").and_then(Value::as_str).unwrap_or(""), "output": text_of(content) })),
            _ => {}
        }
    }
    let mut body = json!({ "model": chat["model"], "input": input, "stream": true, "store": false });
    if !instructions.is_empty() {
        body["instructions"] = json!(instructions.join("\n\n"));
    }
    let tools: Vec<Value> = chat_tools(chat).map(|f| json!({ "type": "function", "name": f["name"], "description": f.get("description").cloned().unwrap_or(json!("")), "parameters": f.get("parameters").cloned().unwrap_or(json!({ "type": "object", "properties": {} })) })).collect();
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
    }
    if let Some(e) = chat.get("reasoning_effort").and_then(Value::as_str) {
        body["reasoning"] = json!({ "effort": e });
    }
    if let Some(n) = chat.get("max_tokens").and_then(Value::as_u64) {
        body["max_output_tokens"] = json!(n.max(16));
    }
    body
}

// ------------------------------------------------------------------ Anthropic

fn push_blocks(messages: &mut Vec<Value>, role: &str, blocks: Vec<Value>) {
    if blocks.is_empty() {
        return;
    }
    if let Some(last) = messages.last_mut().filter(|l| l["role"] == role) {
        if let Some(arr) = last["content"].as_array_mut() {
            arr.extend(blocks);
            return;
        }
    }
    messages.push(json!({ "role": role, "content": blocks }));
}

const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

pub fn to_anthropic(chat: &Value) -> Value {
    let mut system: Vec<String> = Vec::new();
    let mut messages: Vec<Value> = Vec::new();
    for m in chat.get("messages").and_then(Value::as_array).into_iter().flatten() {
        let content = m.get("content").unwrap_or(&Value::Null);
        match m.get("role").and_then(Value::as_str).unwrap_or("") {
            "system" | "developer" => {
                let t = text_of(content);
                if !t.trim().is_empty() {
                    system.push(t);
                }
            }
            "user" => {
                let blocks: Vec<Value> = match content {
                    Value::Array(ps) => ps
                        .iter()
                        .filter_map(|p| match p.get("type").and_then(Value::as_str) {
                            Some("text") => p.get("text").and_then(Value::as_str).filter(|t| !t.is_empty()).map(|t| json!({ "type": "text", "text": t })),
                            Some("image_url") => image_url(p).and_then(|u| match split_data_url(u) {
                                Some((mt, data)) => Some(json!({ "type": "image", "source": { "type": "base64", "media_type": mt, "data": data } })),
                                None if u.starts_with("http") => Some(json!({ "type": "image", "source": { "type": "url", "url": u } })),
                                None => None,
                            }),
                            _ => None,
                        })
                        .collect(),
                    other => {
                        let t = text_of(other);
                        if t.is_empty() { Vec::new() } else { vec![json!({ "type": "text", "text": t })] }
                    }
                };
                push_blocks(&mut messages, "user", blocks);
            }
            "assistant" => {
                let mut blocks = Vec::new();
                let t = text_of(content);
                if !t.trim().is_empty() {
                    blocks.push(json!({ "type": "text", "text": t }));
                }
                for tc in m.get("tool_calls").and_then(Value::as_array).into_iter().flatten() {
                    blocks.push(json!({
                        "type": "tool_use",
                        "id": tc.get("id").and_then(Value::as_str).unwrap_or(""),
                        "name": tc.pointer("/function/name").and_then(Value::as_str).unwrap_or(""),
                        "input": arguments_object(tc.pointer("/function/arguments").and_then(Value::as_str).unwrap_or("{}")),
                    }));
                }
                push_blocks(&mut messages, "assistant", blocks);
            }
            "tool" => {
                let block = json!({ "type": "tool_result", "tool_use_id": m.get("tool_call_id").and_then(Value::as_str).unwrap_or(""), "content": text_of(content) });
                // Results must come first in their user message.
                match messages.last_mut().filter(|l| l["role"] == "user") {
                    Some(last) => {
                        let arr = last["content"].as_array_mut().expect("blocks");
                        let at = arr.iter().take_while(|b| b["type"] == "tool_result").count();
                        arr.insert(at, block);
                    }
                    None => messages.push(json!({ "role": "user", "content": [block] })),
                }
            }
            _ => {}
        }
    }
    if messages.first().is_none_or(|m| m["role"] != "user") {
        messages.insert(0, json!({ "role": "user", "content": [{ "type": "text", "text": "." }] }));
    }
    let mut body = json!({ "model": chat["model"], "messages": messages, "stream": true, "max_tokens": chat.get("max_tokens").and_then(Value::as_u64).unwrap_or(16000) });
    if !system.is_empty() {
        body["system"] = json!(system.join("\n\n"));
    }
    let tools: Vec<Value> = chat_tools(chat).map(|f| json!({ "name": f["name"], "description": f.get("description").cloned().unwrap_or(json!("")), "input_schema": f.get("parameters").cloned().unwrap_or(json!({ "type": "object", "properties": {} })) })).collect();
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
    }
    if let Some(e) = chat.get("reasoning_effort").and_then(Value::as_str).filter(|e| EFFORTS.contains(e)) {
        body["output_config"] = json!({ "effort": e });
    }
    body
}

// -------------------------------------------------------------------- parsing

/// A stream parser: bytes in, events out.
pub trait StreamParser: Send {
    fn feed(&mut self, bytes: &[u8], out: &mut Vec<StreamEvent>);
    /// Completed tool calls; call once the stream ends.
    fn finish(&mut self) -> Vec<StreamEvent>;
    /// An error the provider sent inside the stream.
    fn take_error(&mut self) -> Option<String>;
}

/// Splits SSE into the payloads of its `data:` lines.
#[derive(Default)]
struct Lines {
    buf: Vec<u8>,
}

impl Lines {
    fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            if let Some(p) = line.trim_end().strip_prefix("data:") {
                let p = p.trim();
                if !p.is_empty() && p != "[DONE]" {
                    out.push(p.to_string());
                }
            }
        }
        out
    }
}

fn slot_delta(slot: &mut Pending, index: u32, id: Option<&str>, name: Option<&str>, args: &str, out: &mut Vec<StreamEvent>) {
    let mut id_delta = None;
    if let Some(i) = id.filter(|i| !i.is_empty() && *i != slot.id) {
        slot.id = i.to_string();
        id_delta = Some(i.to_string());
    }
    let mut name_delta = None;
    if let Some(n) = name.filter(|n| !n.is_empty() && slot.name.is_empty()) {
        slot.name = n.to_string();
        name_delta = Some(n.to_string());
    }
    slot.args.push_str(args);
    if id_delta.is_some() || name_delta.is_some() || !args.is_empty() {
        out.push(StreamEvent::ToolDelta { index, id: id_delta, name: name_delta, args_delta: args.to_string() });
    }
}

fn message_of(v: &Value) -> Option<String> {
    v.pointer("/error/message").and_then(Value::as_str).or_else(|| v.get("message").and_then(Value::as_str)).or_else(|| v.pointer("/response/error/message").and_then(Value::as_str)).map(str::to_string)
}

#[derive(Default)]
pub struct ResponsesParser {
    lines: Lines,
    pending: BTreeMap<u32, Pending>,
    error: Option<String>,
}

impl ResponsesParser {
    fn handle(&mut self, v: &Value, out: &mut Vec<StreamEvent>) {
        let index = v.get("output_index").and_then(Value::as_u64).unwrap_or(0) as u32;
        match v.get("type").and_then(Value::as_str).unwrap_or("") {
            "response.output_text.delta" => {
                if let Some(t) = v.get("delta").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                    out.push(StreamEvent::Text { text: t.to_string() });
                }
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                if let Some(t) = v.get("delta").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                    out.push(StreamEvent::Reasoning { text: t.to_string() });
                }
            }
            "response.output_item.added" => {
                let item = &v["item"];
                if item["type"] == "function_call" {
                    let slot = self.pending.entry(index).or_default();
                    slot_delta(slot, index, item.get("call_id").and_then(Value::as_str), item.get("name").and_then(Value::as_str), "", out);
                }
            }
            "response.function_call_arguments.delta" => {
                let slot = self.pending.entry(index).or_default();
                slot_delta(slot, index, None, None, v.get("delta").and_then(Value::as_str).unwrap_or(""), out);
            }
            "response.function_call_arguments.done" => {
                let slot = self.pending.entry(index).or_default();
                if slot.args.is_empty() {
                    slot.args = v.get("arguments").and_then(Value::as_str).unwrap_or("").to_string();
                }
            }
            "response.output_item.done" => {
                let item = &v["item"];
                if item["type"] == "function_call" {
                    let slot = self.pending.entry(index).or_default();
                    if slot.name.is_empty() {
                        slot.name = item.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                    }
                    if slot.id.is_empty() {
                        slot.id = item.get("call_id").and_then(Value::as_str).unwrap_or("").to_string();
                    }
                    if slot.args.is_empty() {
                        slot.args = item.get("arguments").and_then(Value::as_str).unwrap_or("").to_string();
                    }
                }
            }
            "response.completed" | "response.incomplete" => {
                if let (Some(i), Some(o)) = (v.pointer("/response/usage/input_tokens").and_then(Value::as_u64), v.pointer("/response/usage/output_tokens").and_then(Value::as_u64)) {
                    out.push(StreamEvent::Usage { input_tokens: i, output_tokens: o });
                }
            }
            "response.failed" | "error" => {
                self.error = Some(message_of(v).unwrap_or_else(|| "The provider reported an error".into()));
            }
            _ => {}
        }
    }
}

impl StreamParser for ResponsesParser {
    fn feed(&mut self, bytes: &[u8], out: &mut Vec<StreamEvent>) {
        for p in self.lines.push(bytes) {
            if let Ok(v) = serde_json::from_str::<Value>(&p) {
                self.handle(&v, out);
            }
        }
    }
    fn finish(&mut self) -> Vec<StreamEvent> {
        finish_calls(std::mem::take(&mut self.pending))
    }
    fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }
}

#[derive(Default)]
pub struct AnthropicParser {
    lines: Lines,
    pending: BTreeMap<u32, Pending>,
    error: Option<String>,
    input: u64,
}

impl AnthropicParser {
    fn handle(&mut self, v: &Value, out: &mut Vec<StreamEvent>) {
        let index = v.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;
        match v.get("type").and_then(Value::as_str).unwrap_or("") {
            "message_start" => {
                let u = &v["message"]["usage"];
                let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
                self.input = n("input_tokens") + n("cache_read_input_tokens") + n("cache_creation_input_tokens");
            }
            "content_block_start" => {
                let b = &v["content_block"];
                if b["type"] == "tool_use" {
                    let slot = self.pending.entry(index).or_default();
                    slot_delta(slot, index, b.get("id").and_then(Value::as_str), b.get("name").and_then(Value::as_str), "", out);
                }
            }
            "content_block_delta" => {
                let d = &v["delta"];
                match d.get("type").and_then(Value::as_str).unwrap_or("") {
                    "text_delta" => {
                        if let Some(t) = d.get("text").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                            out.push(StreamEvent::Text { text: t.to_string() });
                        }
                    }
                    "thinking_delta" => {
                        if let Some(t) = d.get("thinking").and_then(Value::as_str).filter(|t| !t.is_empty()) {
                            out.push(StreamEvent::Reasoning { text: t.to_string() });
                        }
                    }
                    "input_json_delta" => {
                        let slot = self.pending.entry(index).or_default();
                        slot_delta(slot, index, None, None, d.get("partial_json").and_then(Value::as_str).unwrap_or(""), out);
                    }
                    _ => {}
                }
            }
            "message_delta" => {
                if let Some(o) = v.pointer("/usage/output_tokens").and_then(Value::as_u64) {
                    let i = v.pointer("/usage/input_tokens").and_then(Value::as_u64).filter(|i| *i > 0).unwrap_or(self.input);
                    out.push(StreamEvent::Usage { input_tokens: i.max(self.input), output_tokens: o });
                }
            }
            "error" => self.error = Some(message_of(v).unwrap_or_else(|| "The provider reported an error".into())),
            _ => {}
        }
    }
}

impl StreamParser for AnthropicParser {
    fn feed(&mut self, bytes: &[u8], out: &mut Vec<StreamEvent>) {
        for p in self.lines.push(bytes) {
            if let Ok(v) = serde_json::from_str::<Value>(&p) {
                self.handle(&v, out);
            }
        }
    }
    fn finish(&mut self) -> Vec<StreamEvent> {
        finish_calls(std::mem::take(&mut self.pending))
    }
    fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }
}

pub fn parser_for(w: Wire) -> Box<dyn StreamParser> {
    match w {
        Wire::Chat => Box::<crate::ai::SseParser>::default(),
        Wire::Responses => Box::<ResponsesParser>::default(),
        Wire::Anthropic => Box::<AnthropicParser>::default(),
    }
}

/// The headers a wire needs beyond the bearer token.
pub fn extra_headers(w: Wire, key: Option<&str>) -> Vec<(&'static str, String)> {
    match w {
        Wire::Anthropic => {
            let mut h = vec![("anthropic-version", "2023-06-01".to_string())];
            if let Some(k) = key {
                h.push(("x-api-key", k.to_string()));
            }
            h
        }
        _ => Vec::new(),
    }
}

#[allow(dead_code)]
fn _unused(_: Map<String, Value>) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat() -> Value {
        json!({
            "model": "gpt-6.2-astra", "stream": true, "max_tokens": 2000, "reasoning_effort": "high",
            "messages": [
                { "role": "system", "content": "Be brief." },
                { "role": "user", "content": [{ "type": "text", "text": "look" }, { "type": "image_url", "image_url": { "url": "data:image/png;base64,AAAA" } }] },
                { "role": "assistant", "content": "Opening it.", "tool_calls": [{ "id": "c1", "type": "function", "function": { "name": "read_file", "arguments": "{\"path\":\"a\"}" } }] },
                { "role": "tool", "tool_call_id": "c1", "content": "file text" },
                { "role": "user", "content": "thanks" }
            ],
            "tools": [{ "type": "function", "function": { "name": "read_file", "description": "Read", "parameters": { "type": "object", "properties": { "path": { "type": "string" } } } } }]
        })
    }

    #[test]
    fn urls() {
        assert_eq!(Wire::Responses.url("https://x.dev/v1/"), "https://x.dev/v1/responses");
        assert_eq!(Wire::Anthropic.url("https://api.silvrai.net/"), "https://api.silvrai.net/v1/messages");
        assert_eq!(Wire::Anthropic.url("https://api.silvrai.net/v1"), "https://api.silvrai.net/v1/messages");
        assert_eq!(Wire::parse("Responses"), Some(Wire::Responses));
        assert_eq!(Wire::parse("auto"), None);
        assert!(valid_setting("") && valid_setting("Auto") && !valid_setting("grpc"));
    }

    #[test]
    fn responses_request() {
        let b = to_responses(&chat());
        assert_eq!(b["instructions"], "Be brief.");
        assert_eq!(b["reasoning"]["effort"], "high");
        assert_eq!(b["max_output_tokens"], 2000);
        assert_eq!(b["tools"][0]["name"], "read_file");
        assert!(b["tools"][0].get("function").is_none());
        let input = b["input"].as_array().unwrap();
        assert_eq!(input[0]["content"][1], json!({ "type": "input_image", "image_url": "data:image/png;base64,AAAA" }));
        assert_eq!(input[1]["content"][0]["type"], "output_text");
        assert_eq!(input[2], json!({ "type": "function_call", "call_id": "c1", "name": "read_file", "arguments": "{\"path\":\"a\"}" }));
        assert_eq!(input[3], json!({ "type": "function_call_output", "call_id": "c1", "output": "file text" }));
        assert_eq!(input[4]["role"], "user");
    }

    #[test]
    fn anthropic_request() {
        let b = to_anthropic(&chat());
        assert_eq!(b["system"], "Be brief.");
        assert_eq!(b["output_config"]["effort"], "high");
        assert_eq!(b["tools"][0]["input_schema"]["type"], "object");
        let m = b["messages"].as_array().unwrap();
        assert_eq!(m[0]["content"][1], json!({ "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "AAAA" } }));
        assert_eq!(m[1]["content"][1], json!({ "type": "tool_use", "id": "c1", "name": "read_file", "input": { "path": "a" } }));
        // The tool result and the next user text share one user message, result first.
        assert_eq!(m.len(), 3);
        assert_eq!(m[2]["content"][0]["type"], "tool_result");
        assert_eq!(m[2]["content"][1], json!({ "type": "text", "text": "thanks" }));
    }

    fn run(mut p: Box<dyn StreamParser>, chunks: &[&str]) -> (Vec<StreamEvent>, Option<String>) {
        let mut out = Vec::new();
        for c in chunks {
            p.feed(c.as_bytes(), &mut out);
        }
        out.extend(p.finish());
        (out, p.take_error())
    }

    #[test]
    fn responses_stream() {
        let (ev, err) = run(
            Box::<ResponsesParser>::default(),
            &[
                "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"Hel\"}\n\n",
                "data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"hm\"}\n\ndata: {\"type\":\"response.output_item.added\",\"output_index\":1,\"item\":{\"type\":\"function_call\",\"call_id\":\"c9\",\"name\":\"read_file\"}}\n\n",
                "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":1,\"delta\":\"{\\\"path\\\":\"}\n\ndata: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":1,\"delta\":\"\\\"a\\\"}\"}\n\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":12,\"output_tokens\":5}}}\n\ndata: [DONE]\n\n",
            ],
        );
        assert!(err.is_none());
        assert!(ev.contains(&StreamEvent::Text { text: "Hel".into() }) && ev.contains(&StreamEvent::Reasoning { text: "hm".into() }));
        assert!(ev.contains(&StreamEvent::Usage { input_tokens: 12, output_tokens: 5 }));
        assert!(matches!(ev.last(), Some(StreamEvent::ToolCall { tool_call }) if tool_call.id == "c9" && tool_call.name == "read_file" && tool_call.arguments == "{\"path\":\"a\"}"));
        let (_, err) = run(Box::<ResponsesParser>::default(), &["data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"boom\"}}}\n\n"]);
        assert_eq!(err.as_deref(), Some("boom"));
    }

    #[test]
    fn anthropic_stream() {
        let (ev, err) = run(
            Box::<AnthropicParser>::default(),
            &[
                "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":10,\"cache_read_input_tokens\":4}}}\n\n",
                "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\"}}\n\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\n\n",
                "data: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"tu1\",\"name\":\"read_file\"}}\n\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"b\\\"}\"}}\n\n",
                "data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":7}}\n\ndata: {\"type\":\"message_stop\"}\n\n",
            ],
        );
        assert!(err.is_none());
        assert!(ev.contains(&StreamEvent::Text { text: "Hi".into() }));
        assert!(ev.contains(&StreamEvent::Usage { input_tokens: 14, output_tokens: 7 }));
        assert!(matches!(ev.last(), Some(StreamEvent::ToolCall { tool_call }) if tool_call.id == "tu1" && tool_call.arguments == "{\"path\":\"b\"}"));
        let (_, err) = run(Box::<AnthropicParser>::default(), &["data: {\"type\":\"error\",\"error\":{\"message\":\"overloaded\"}}\n\n"]);
        assert_eq!(err.as_deref(), Some("overloaded"));
    }
}
