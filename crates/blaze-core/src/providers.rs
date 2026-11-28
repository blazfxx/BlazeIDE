//! Custom providers: any OpenAI-compatible endpoint the user adds (their own
//! gateway, OpenRouter, a local Ollama or LM Studio). Only name, base URL and
//! model ids are stored in state.json; the API key goes to the OS keychain.
//!
//! Model ids reach the UI as `custom:<provider id>/<model>`, so the shell can
//! route a request without the page ever naming a URL.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::{Host, Url};

use crate::error::{Error, Result};
use crate::models::ModelEntry;

const PREFIX: &str = "custom:";
const MAX_MODELS: usize = 500;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CustomProvider {
    pub id: String,
    pub name: String,
    pub base_url: String,
    #[serde(default)]
    pub models: Vec<String>,
    /// Which endpoint the provider speaks: "auto" / "" (find out), "chat",
    /// "responses" or "anthropic" (see `wire`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub wire: String,
    /// What each model can do, as the endpoint (or the user) said.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub caps: BTreeMap<String, ModelCaps>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelCaps {
    /// Whether it can see images (None = nobody said).
    pub vision: Option<bool>,
    /// Context window in tokens (0 = unknown).
    pub context: u64,
    /// The user set `vision`: never overwrite it.
    pub manual: bool,
}

const MAX_CONTEXT: u64 = 20_000_000;

/// What one row of a `/models` answer says. Endpoints differ: OpenRouter-style
/// (also Surplus), Mistral, LM Studio, LiteLLM and plain flags.
fn caps_of(row: &Value) -> ModelCaps {
    let strings = |v: Option<&Value>| -> Option<Vec<String>> { v.and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_ascii_lowercase)).collect()) };
    let flag = |paths: &[&str]| paths.iter().find_map(|p| row.pointer(p).and_then(Value::as_bool));
    let has = |l: &[String], words: &[&str]| l.iter().any(|x| words.contains(&x.as_str()));
    let inputs = ["/architecture/input_modalities", "/input_modalities", "/modalities/input", "/modalities"].iter().find_map(|p| strings(row.pointer(p)));
    // "text+image->text"
    let arrow = ["/architecture/modality", "/modality"].iter().find_map(|p| row.pointer(p).and_then(Value::as_str)).and_then(|m| m.split_once("->")).map(|(i, _)| i.to_ascii_lowercase().contains("image"));
    let features: Vec<String> = ["/supported_features", "/capabilities", "/features", "/tags"].iter().filter_map(|p| strings(row.pointer(p))).flatten().collect();
    let flagged = flag(&["/capabilities/vision", "/supports_vision", "/vision", "/abilities/vision", "/supports_image_in", "/supports_image_input", "/model_info/supports_vision", "/capabilities/image_input"]);
    // Any "yes" wins; "no" needs the endpoint to have listed the inputs.
    let yes = inputs.as_ref().is_some_and(|l| has(l, &["image", "images", "vision"])) || has(&features, &["vision", "image_input", "image-input"]) || arrow == Some(true) || flagged == Some(true) || row.get("type").and_then(Value::as_str) == Some("vlm");
    let vision = if yes {
        Some(true)
    } else if inputs.is_some() || arrow.is_some() {
        Some(false)
    } else {
        flagged
    };
    let context = ["/context_length", "/context_window", "/max_context_length", "/max_model_len", "/max_input_tokens", "/top_provider/context_length", "/max_context_window_tokens", "/model_info/max_input_tokens"]
        .iter()
        .find_map(|p| row.pointer(p).and_then(Value::as_u64))
        .unwrap_or(0)
        .min(MAX_CONTEXT);
    ModelCaps { vision, context, manual: false }
}

/// Model families known to see images, for endpoints whose list says nothing.
/// Only ever used to answer "yes"; an unknown model still gets its images.
pub fn known_vision(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    let m = m.rsplit('/').next().unwrap_or(&m).split(':').next().unwrap_or("").to_string();
    if ["embed", "whisper", "tts", "rerank", "transcribe", "moderation"].iter().any(|k| m.contains(k)) {
        return false;
    }
    let starts = ["claude-3", "claude-haiku", "claude-sonnet", "claude-opus", "claude-fable", "gpt-4o", "gpt-4.1", "gpt-4-turbo", "gpt-5", "gpt-6", "o3", "o4-", "gemini", "grok-4", "llama-4", "llama4", "pixtral", "llava", "glm-5.3-flash", "gemma3", "gemma-3", "mistral-medium-3", "mistral-small-3.", "kimi-k2.5"];
    let part = |k: &str| m.split(['-', '_', '.']).any(|p| p == k);
    // GLM's vision models end in "v": glm-4v, glm-4.5v, glm-4.6v-flash.
    let glm_v = m.starts_with("glm-") && m.split('-').any(|p| p.len() > 1 && p.ends_with('v') && p[..p.len() - 1].chars().all(|c| c.is_ascii_digit() || c == '.'));
    let reasoning_only = m.starts_with("o3-mini");
    !reasoning_only && (starts.iter().any(|s| m.starts_with(s)) || part("vl") || part("vision") || m.contains("-vl-") || m.contains("vision") || glm_v || m.contains("qwen2.5vl") || m.contains("minicpm-v"))
}

/// Can this provider's model see images? What was said, else what is known.
pub fn sees_images(p: &CustomProvider, model: &str) -> bool {
    p.caps.get(model).and_then(|c| c.vision).unwrap_or_else(|| known_vision(model))
}

/// Take in what a catalogue says about the provider's models. A user's own
/// choice stays. Every listed model gets an entry, so it is asked only once.
pub fn apply_catalog(p: &mut CustomProvider, catalog: &[(String, ModelCaps)]) {
    let found: HashMap<&str, &ModelCaps> = catalog.iter().map(|(id, c)| (id.as_str(), c)).collect();
    for m in &p.models {
        // "model:route" shares the abilities of "model".
        let said = found.get(m.as_str()).or_else(|| m.split_once(':').and_then(|(base, _)| found.get(base))).map(|c| (*c).clone()).unwrap_or_default();
        let slot = p.caps.entry(m.clone()).or_default();
        if !slot.manual {
            slot.vision = said.vision;
        }
        slot.context = said.context;
    }
}

fn is_loopback(host: &Host<&str>) -> bool {
    match host {
        Host::Domain(d) => d.eq_ignore_ascii_case("localhost"),
        Host::Ipv4(ip) => ip.is_loopback(),
        Host::Ipv6(ip) => ip.is_loopback(),
    }
}

/// https only, except plain http to this machine (Ollama, LM Studio). No
/// credentials, query or fragment in the URL. Returns it without a trailing
/// slash.
pub fn validate_base_url(raw: &str) -> Result<String> {
    let u = Url::parse(raw.trim()).map_err(|_| Error::msg("That is not a valid URL"))?;
    let host = u.host().ok_or_else(|| Error::msg("The URL needs a host"))?;
    match u.scheme() {
        "https" => {}
        "http" if is_loopback(&host) => {}
        "http" => return Err(Error::msg("Use https:// (plain http is only allowed for localhost)")),
        _ => return Err(Error::msg("The URL must start with https://")),
    }
    if !u.username().is_empty() || u.password().is_some() || u.query().is_some() || u.fragment().is_some() {
        return Err(Error::msg("Put the API key in the key field, not in the URL"));
    }
    Ok(u.as_str().trim_end_matches('/').to_string())
}

/// Clean a provider before it is stored. The id is generated here when empty.
pub fn sanitize(mut p: CustomProvider) -> Result<CustomProvider> {
    p.name = p.name.trim().to_string();
    if p.name.is_empty() || p.name.chars().count() > 40 {
        return Err(Error::msg("Give the provider a name (40 characters at most)"));
    }
    p.base_url = validate_base_url(&p.base_url)?;
    if !crate::wire::valid_setting(&p.wire) {
        return Err(Error::msg("Endpoint must be auto, chat, responses or anthropic"));
    }
    p.wire = p.wire.trim().to_ascii_lowercase();
    if p.wire == "auto" {
        p.wire.clear();
    }
    if p.id.is_empty() {
        p.id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
    }
    if !p.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') || p.id.len() > 40 {
        return Err(Error::msg("Invalid provider id"));
    }
    let mut seen = std::collections::HashSet::new();
    p.models = p
        .models
        .into_iter()
        .map(|m| m.trim().to_string())
        .filter(|m| !m.is_empty() && m.len() <= 200 && !m.chars().any(char::is_control) && seen.insert(m.clone()))
        .take(MAX_MODELS)
        .collect();
    let listed: HashSet<&String> = p.models.iter().collect();
    p.caps.retain(|m, _| listed.contains(m));
    for c in p.caps.values_mut() {
        c.context = c.context.min(MAX_CONTEXT);
    }
    Ok(p)
}

pub fn model_id(provider_id: &str, model: &str) -> String {
    format!("{PREFIX}{provider_id}/{model}")
}

/// `custom:<provider>/<model>` -> (provider id, model). The model may itself
/// contain slashes (`meta-llama/llama-3`).
pub fn split_model_id(id: &str) -> Option<(&str, &str)> {
    id.strip_prefix(PREFIX)?.split_once('/').filter(|(p, m)| !p.is_empty() && !m.is_empty())
}

pub fn entries(p: &CustomProvider) -> Vec<ModelEntry> {
    p.models
        .iter()
        .map(|m| ModelEntry {
            id: model_id(&p.id, m),
            display_name: m.rsplit('/').next().unwrap_or(m).to_string(),
            provider: p.name.clone(),
            context_window: p.caps.get(m).map_or(0, |c| c.context),
            kind: "chat".into(),
            vision: sees_images(p, m),
            free: false,
            price_in: 0.0,
            price_out: 0.0,
            // "text-only" = someone SAID it cannot see; unknown is not that.
            tags: if p.caps.get(m).and_then(|c| c.vision) == Some(false) { vec!["custom".into(), "text-only".into()] } else { vec!["custom".into()] },
        })
        .collect()
}

/// Ask an OpenAI-compatible endpoint which models it serves (`GET /models`).
pub async fn fetch_models(http: &reqwest::Client, base_url: &str, key: Option<&str>) -> Result<Vec<String>> {
    let mut ids: Vec<String> = fetch_catalog(http, base_url, key).await?.into_iter().map(|(id, _)| id).collect();
    ids.truncate(MAX_MODELS);
    Ok(ids)
}

/// Learn the abilities of providers saved without them (added by an older
/// version, or while the endpoint could not be reached). Each is tried once
/// per run. True when something was learnt.
pub async fn learn_missing(store: &crate::store::Store, http: &reqwest::Client, key_of: impl Fn(&str) -> Option<String>) -> bool {
    static TRIED: Mutex<Option<HashSet<String>>> = Mutex::new(None);
    let todo: Vec<CustomProvider> = store
        .providers()
        .into_iter()
        .filter(|p| p.models.iter().any(|m| !p.caps.contains_key(m)))
        .filter(|p| TRIED.lock().expect("tried").get_or_insert_with(HashSet::new).insert(format!("{}|{}|{}", p.id, p.base_url, p.models.len())))
        .collect();
    let answers = futures_util::future::join_all(todo.into_iter().map(|p| {
        let key = key_of(&p.id);
        async move { (p.id.clone(), tokio::time::timeout(Duration::from_secs(6), fetch_catalog(http, &p.base_url, key.as_deref())).await) }
    }))
    .await;
    let mut learnt = false;
    for (id, answer) in answers {
        let Ok(Ok(catalog)) = answer else { continue };
        // Read again: it may have been edited while we asked.
        if let Some(mut p) = store.providers().into_iter().find(|p| p.id == id) {
            apply_catalog(&mut p, &catalog);
            learnt |= store.upsert_provider(p).is_ok();
        }
    }
    learnt
}

/// Like [`fetch_models`], with what the endpoint says about each one.
pub async fn fetch_catalog(http: &reqwest::Client, base_url: &str, key: Option<&str>) -> Result<Vec<(String, ModelCaps)>> {
    let base = validate_base_url(base_url)?;
    let mut req = http.get(format!("{base}/models")).timeout(Duration::from_secs(15));
    if let Some(k) = key.filter(|k| !k.is_empty()) {
        req = req.bearer_auth(k);
    }
    let resp = req.send().await.map_err(|_| Error::msg("Could not reach that endpoint"))?;
    if !resp.status().is_success() {
        return Err(Error::Http { status: resp.status().as_u16(), message: format!("The endpoint answered HTTP {}", resp.status().as_u16()) });
    }
    let body: Value = resp.json().await.map_err(|_| Error::msg("The endpoint did not return a model list"))?;
    let mut rows: Vec<(String, ModelCaps)> = body
        .get("data")
        .and_then(Value::as_array)
        .map(|rows| rows.iter().filter_map(|m| Some((m.get("id").and_then(Value::as_str)?.to_string(), caps_of(m)))).collect())
        .unwrap_or_default();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows.dedup_by(|a, b| a.0 == b.0);
    rows.truncate(5000);
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(validate_base_url("https://openrouter.ai/api/v1/").unwrap(), "https://openrouter.ai/api/v1");
        assert_eq!(validate_base_url("http://localhost:11434/v1").unwrap(), "http://localhost:11434/v1");
        assert!(validate_base_url("http://127.0.0.1:1234/v1").is_ok());
        assert!(validate_base_url("http://example.com/v1").is_err());
        assert!(validate_base_url("ftp://example.com").is_err());
        assert!(validate_base_url("https://user:pw@example.com/v1").is_err());
        assert!(validate_base_url("https://example.com/v1?key=abc").is_err());
        assert!(validate_base_url("not a url").is_err());
    }

    #[test]
    fn ids_round_trip() {
        let id = model_id("abc123", "meta-llama/llama-3");
        assert_eq!(split_model_id(&id), Some(("abc123", "meta-llama/llama-3")));
        assert_eq!(split_model_id("claude-sonnet-5"), None);
        assert_eq!(split_model_id("custom:abc/"), None);
    }

    #[test]
    fn sanitizes() {
        let p = sanitize(CustomProvider { id: String::new(), name: " Mine ".into(), base_url: "https://x.dev/v1/".into(), models: vec!["a".into(), " a ".into(), "".into(), "b".into()], ..Default::default() }).unwrap();
        assert_eq!(p.name, "Mine");
        assert_eq!(p.base_url, "https://x.dev/v1");
        assert_eq!(p.models, vec!["a", "b"]);
        assert_eq!(p.id.len(), 12);
        assert!(sanitize(CustomProvider { id: "../x".into(), name: "n".into(), base_url: "https://x.dev".into(), models: vec![], ..Default::default() }).is_err());
        assert!(sanitize(CustomProvider { id: String::new(), name: "".into(), base_url: "https://x.dev".into(), models: vec![], ..Default::default() }).is_err());
        let e = entries(&p);
        assert_eq!(e[0].id, format!("custom:{}/a", p.id));
        assert_eq!(e[0].provider, "Mine");
    }

    #[test]
    fn reads_what_models_can_do() {
        let say = |v: serde_json::Value| caps_of(&v);
        let glm = say(serde_json::json!({ "id": "glm-5.3-flash", "context_length": 1048576, "architecture": { "modality": "text+image+video->text", "input_modalities": ["text", "image", "video"] }, "supported_features": ["streaming", "vision", "tools"] }));
        assert_eq!((glm.vision, glm.context), (Some(true), 1048576));
        assert_eq!(say(serde_json::json!({ "architecture": { "input_modalities": ["text"] } })).vision, Some(false));
        assert_eq!(say(serde_json::json!({ "architecture": { "modality": "text->text" } })).vision, Some(false));
        assert_eq!(say(serde_json::json!({ "capabilities": { "vision": true }, "max_context_length": 131072 })).context, 131072);
        assert_eq!(say(serde_json::json!({ "capabilities": ["completion", "vision"] })).vision, Some(true));
        assert_eq!(say(serde_json::json!({ "abilities": { "vision": true } })).vision, Some(true));
        assert_eq!(say(serde_json::json!({ "id": "gpt-6", "object": "model" })), ModelCaps::default());
        for yes in ["glm-5.3-flash", "z-ai/glm-4.6v", "glm-4.5v:surplus", "claude-opus-5-5", "openai/gpt-6.1-sol", "gemini-4-carbon", "qwen3-vl-72b", "llama-3.2-11b-vision", "pixtral-large"] {
            assert!(known_vision(yes), "{yes}");
        }
        for no in ["glm-5.3", "deepseek-v4.1", "text-embedding-3", "o3-mini", "llama-3.3-70b", "qwen3-coder"] {
            assert!(!known_vision(no), "{no}");
        }
        let mut p = CustomProvider { id: "p1".into(), name: "S".into(), base_url: "https://x.dev/v1".into(), models: vec!["glm-5.3-flash".into(), "glm-5.3-flash:cheap".into(), "mystery".into(), "claude-opus-5-5".into(), "mine".into()], ..Default::default() };
        p.caps.insert("mine".into(), ModelCaps { vision: Some(true), context: 0, manual: true });
        assert!(entries(&p)[0].vision && !entries(&p)[2].vision);
        apply_catalog(&mut p, &[("glm-5.3-flash".into(), glm.clone()), ("mine".into(), ModelCaps { vision: Some(false), context: 9, manual: false }), ("claude-opus-5-5".into(), ModelCaps { vision: Some(false), ..Default::default() })]);
        let e = entries(&p);
        assert!(e[0].vision && e[0].context_window == 1048576 && e[1].vision && e[1].context_window == 1048576);
        assert!(!e[2].vision && !e[2].tags.contains(&"text-only".to_string()), "unknown is not text-only");
        assert!(!e[3].vision && e[3].tags.contains(&"text-only".to_string()), "the endpoint's answer wins over the name");
        assert!(e[4].vision && p.caps["mine"].manual, "the user's choice stays");
        let old: CustomProvider = serde_json::from_str(r#"{"id":"a","name":"n","baseUrl":"https://x.dev","models":["glm-5.3-flash"]}"#).unwrap();
        assert!(old.caps.is_empty() && old.wire.is_empty() && entries(&old)[0].vision);
    }

    #[test]
    fn the_endpoint_setting_is_checked() {
        let mk = |w: &str| sanitize(CustomProvider { name: "n".into(), base_url: "https://x.dev".into(), wire: w.into(), ..Default::default() });
        assert_eq!(mk(" Responses ").unwrap().wire, "responses");
        assert_eq!(mk("auto").unwrap().wire, "");
        assert!(mk("grpc").is_err());
    }
}
