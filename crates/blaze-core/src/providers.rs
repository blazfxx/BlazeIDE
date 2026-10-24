//! Custom providers: any OpenAI-compatible endpoint the user adds (their own
//! gateway, OpenRouter, a local Ollama or LM Studio). Only name, base URL and
//! model ids are stored in state.json; the API key goes to the OS keychain.
//!
//! Model ids reach the UI as `custom:<provider id>/<model>`, so the shell can
//! route a request without the page ever naming a URL.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::{Host, Url};

use crate::error::{Error, Result};
use crate::models::ModelEntry;

const PREFIX: &str = "custom:";
const MAX_MODELS: usize = 500;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CustomProvider {
    pub id: String,
    pub name: String,
    pub base_url: String,
    #[serde(default)]
    pub models: Vec<String>,
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
            context_window: 0,
            kind: "chat".into(),
            vision: false,
            free: false,
            price_in: 0.0,
            price_out: 0.0,
            tags: vec!["custom".into()],
        })
        .collect()
}

/// Ask an OpenAI-compatible endpoint which models it serves (`GET /models`).
pub async fn fetch_models(http: &reqwest::Client, base_url: &str, key: Option<&str>) -> Result<Vec<String>> {
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
    let mut ids: Vec<String> = body
        .get("data")
        .and_then(Value::as_array)
        .map(|rows| rows.iter().filter_map(|m| m.get("id").and_then(Value::as_str).map(str::to_string)).collect())
        .unwrap_or_default();
    ids.sort();
    ids.dedup();
    ids.truncate(MAX_MODELS);
    Ok(ids)
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
        let p = sanitize(CustomProvider { id: String::new(), name: " Mine ".into(), base_url: "https://x.dev/v1/".into(), models: vec!["a".into(), " a ".into(), "".into(), "b".into()] }).unwrap();
        assert_eq!(p.name, "Mine");
        assert_eq!(p.base_url, "https://x.dev/v1");
        assert_eq!(p.models, vec!["a", "b"]);
        assert_eq!(p.id.len(), 12);
        assert!(sanitize(CustomProvider { id: "../x".into(), name: "n".into(), base_url: "https://x.dev".into(), models: vec![] }).is_err());
        assert!(sanitize(CustomProvider { id: String::new(), name: "".into(), base_url: "https://x.dev".into(), models: vec![] }).is_err());
        let e = entries(&p);
        assert_eq!(e[0].id, format!("custom:{}/a", p.id));
        assert_eq!(e[0].provider, "Mine");
    }
}
