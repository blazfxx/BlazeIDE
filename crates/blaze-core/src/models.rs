//! The model picker's list, straight from the signed-in account's
//! `/paid/v1/models` (so it only shows what this account can call).

use std::path::Path;
use std::time::Duration;

use base64::Engine;
use serde::Serialize;
use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::paths::resolve_in;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelEntry {
    pub id: String,
    pub display_name: String,
    pub provider: String,
    pub context_window: u64,
    pub kind: String,
    pub vision: bool,
    /// Free tier: callable at no cost.
    pub free: bool,
    pub price_in: f64,
    pub price_out: f64,
    pub tags: Vec<String>,
}

const VENDORS: &[(&str, &str)] = &[
    ("claude", "Anthropic"),
    ("gpt", "OpenAI"),
    ("o1", "OpenAI"),
    ("o3", "OpenAI"),
    ("o4", "OpenAI"),
    ("codex", "OpenAI"),
    ("gemini", "Google"),
    ("gemma", "Google"),
    ("nano-banana", "Google"),
    ("deepseek", "DeepSeek"),
    ("glm", "Z.ai"),
    ("kimi", "Moonshot AI"),
    ("qwen", "Alibaba"),
    ("grok", "xAI"),
    ("llama", "Meta"),
    ("mistral", "Mistral"),
    ("devstral", "Mistral"),
    ("minimax", "MiniMax"),
    ("venice", "Venice"),
    ("seedream", "ByteDance"),
    ("flux", "Black Forest Labs"),
];

fn bare(id: &str) -> &str {
    id.rsplit('/').next().unwrap_or(id)
}

pub fn vendor_for(id: &str) -> String {
    let b = bare(id).to_lowercase();
    VENDORS.iter().find(|(p, _)| b.starts_with(p)).map(|(_, v)| v.to_string()).unwrap_or_else(|| "Other".into())
}

/// "claude-sonnet-5" -> "Claude Sonnet 5", "free/glm-5.3" -> "GLM 5.3 (free)".
pub fn display_name(id: &str) -> String {
    let b = bare(id);
    let words: Vec<String> = b
        .split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let lower = w.to_lowercase();
            if ["gpt", "glm", "gpt", "ai", "tts", "oss", "vl"].contains(&lower.as_str()) {
                lower.to_uppercase()
            } else if lower.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) && lower.chars().skip(1).any(|c| c.is_ascii_digit()) && lower.len() <= 5 {
                // k2.6, v4, o3 -> K2.6, V4, O3
                lower.to_uppercase()
            } else {
                let mut c = lower.chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
            }
        })
        .collect();
    let mut name = words.join(" ");
    if id.starts_with("free/") {
        name.push_str(" (free)");
    }
    name
}

fn has_tag(tags: &str, t: &str) -> bool {
    tags.split(',').any(|x| x.trim() == t)
}

pub fn parse_models(body: &Value) -> Vec<ModelEntry> {
    let Some(rows) = body.get("data").and_then(Value::as_array) else { return Vec::new() };
    let mut out: Vec<ModelEntry> = rows
        .iter()
        .filter_map(|m| {
            let id = m.get("id")?.as_str()?.to_string();
            let kind = match m.get("type").and_then(Value::as_str).unwrap_or("chat") {
                "chat" => "chat",
                "image" => "image",
                "video" => "video",
                _ => return None,
            };
            if m.get("available_for_account").and_then(Value::as_bool) == Some(false) {
                return None;
            }
            let tags_raw = m.get("tags").and_then(Value::as_str).unwrap_or("");
            // Retired plan-only rows are not callable on pay-as-you-go keys.
            if has_tag(tags_raw, "tokv2") || has_tag(tags_raw, "credits") {
                return None;
            }
            let num = |k: &str| m.get(k).and_then(Value::as_f64).unwrap_or(0.0);
            let flat = num("price_in_per_m") == 0.0 && num("price_out_per_m") == 0.0;
            let free = has_tag(tags_raw, "free") || m.get("is_free").and_then(Value::as_bool) == Some(true) || id.starts_with("free/");
            Some(ModelEntry {
                display_name: display_name(&id),
                provider: vendor_for(&id),
                context_window: m.get("context_window").and_then(Value::as_u64).unwrap_or(0),
                kind: kind.into(),
                vision: m.get("vision").and_then(Value::as_bool).unwrap_or(false),
                free,
                price_in: if free { 0.0 } else if flat { num("min_charge_usd") } else { num("price_in_per_m") },
                price_out: if free { 0.0 } else { num("price_out_per_m") },
                tags: tags_raw.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect(),
                id,
            })
        })
        .collect();
    out.sort_by(|a, b| a.provider.cmp(&b.provider).then_with(|| a.display_name.cmp(&b.display_name)));
    out.dedup_by(|a, b| a.id == b.id);
    out
}

/// The model list. Works signed out too (the catalogue is public), so the
/// picker is never empty; with a key it reflects what that account can call.
pub async fn fetch_models(http: &reqwest::Client, api_base: &str, api_key: Option<&str>) -> Result<(Vec<ModelEntry>, Value)> {
    let mut req = http.get(format!("{api_base}/paid/v1/models")).timeout(Duration::from_secs(15));
    if let Some(k) = api_key {
        req = req.bearer_auth(k);
    }
    let resp = req.send().await?;
    if !resp.status().is_success() {
        return Err(Error::Http { status: resp.status().as_u16(), message: "Could not load models from BlazeAPI".into() });
    }
    let body: Value = resp.json().await?;
    Ok((parse_models(&body), body))
}

#[derive(serde::Deserialize, Default)]
pub struct Usage {
    #[serde(default)]
    pub balance_usd: f64,
}

pub async fn fetch_balance(http: &reqwest::Client, api_base: &str, api_key: &str) -> Result<f64> {
    let resp = http.get(format!("{api_base}/paid/v1/usage")).bearer_auth(api_key).timeout(Duration::from_secs(10)).send().await?;
    if !resp.status().is_success() {
        return Err(Error::Http { status: resp.status().as_u16(), message: "Could not load your balance".into() });
    }
    Ok(resp.json::<Usage>().await.unwrap_or_default().balance_usd)
}

const PREFERRED_COMPLETION_MODELS: &[&str] = &["glm-5.3", "free/glm-5.3", "free/deepseek-v4-flash", "deepseek-v4-flash"];

/// Models to try, in order, for editor autocomplete when the user has not
/// chosen one: a very cheap fast model first (a suggestion costs a fraction
/// of a cent), then free ones if that fails (e.g. no balance).
pub fn completion_candidates(models: &[ModelEntry]) -> Vec<String> {
    PREFERRED_COMPLETION_MODELS
        .iter()
        .filter(|p| models.iter().any(|m| m.id == **p && m.kind == "chat"))
        .map(|s| s.to_string())
        .collect()
}

const PREFERRED_IMAGE_MODELS: &[&str] = &["gpt-image-2", "gpt-image-2.5", "gemini-2.5-flash-image", "nano-banana-pro"];

pub fn pick_image_model(models: &[ModelEntry]) -> Option<String> {
    PREFERRED_IMAGE_MODELS
        .iter()
        .find(|p| models.iter().any(|m| m.id == **p))
        .map(|s| s.to_string())
        .or_else(|| models.iter().find(|m| m.kind == "image").map(|m| m.id.clone()))
}

fn size_for(model: &str, aspect: Option<&str>) -> Option<&'static str> {
    if !model.starts_with("gpt-image") {
        return None;
    }
    Some(match aspect.unwrap_or("1:1") {
        "16:9" | "4:3" => "1536x1024",
        "9:16" | "3:4" => "1024x1536",
        _ => "1024x1024",
    })
}

/// What to generate and where to save it (inside `root`).
pub struct ImageJob<'a> {
    pub model: Option<String>,
    pub root: &'a Path,
    pub rel_path: &'a str,
    pub prompt: &'a str,
    pub aspect: Option<&'a str>,
}

/// Generate an image and save it inside the project. Returns the tool-result
/// text for the model (never Err, so the agent can carry on).
pub async fn generate_image(http: &reqwest::Client, api_base: &str, api_key: &str, job: ImageJob<'_>) -> String {
    let ImageJob { model, root, rel_path, prompt, aspect } = job;
    if prompt.trim().is_empty() {
        return "Error: a prompt is required.".into();
    }
    let dest = match resolve_in(root, rel_path) {
        Ok(p) => p,
        Err(e) => return format!("Error: {e}"),
    };
    let Some(model) = model else { return "Error: no image model is available on your BlazeAPI account.".into() };
    let mut body = json!({ "model": model, "prompt": prompt, "n": 1 });
    if let Some(size) = size_for(&model, aspect) {
        body["size"] = json!(size);
    }
    let resp = match http
        .post(format!("{api_base}/paid/v1/images/generations"))
        .bearer_auth(api_key)
        .json(&body)
        .timeout(Duration::from_secs(180))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => return format!("Error: {}", Error::from(e)),
    };
    let status = resp.status();
    let v: Value = resp.json().await.unwrap_or_default();
    if !status.is_success() {
        let msg = v.get("error").and_then(|e| e.get("message").and_then(Value::as_str).or_else(|| e.as_str())).unwrap_or("generation failed");
        return format!("Error: image generation failed ({msg}).");
    }
    let item = &v["data"][0];
    let bytes = if let Some(b64) = item.get("b64_json").and_then(Value::as_str) {
        match base64::engine::general_purpose::STANDARD.decode(b64) {
            Ok(b) => b,
            Err(_) => return "Error: the image data was invalid.".into(),
        }
    } else if let Some(url) = item.get("url").and_then(Value::as_str).filter(|u| u.starts_with("https://")) {
        match http.get(url).timeout(Duration::from_secs(60)).send().await {
            Ok(r) if r.status().is_success() => match r.bytes().await {
                Ok(b) => b.to_vec(),
                Err(_) => return "Error: could not download the image.".into(),
            },
            _ => return "Error: could not download the image.".into(),
        }
    } else {
        return "Error: BlazeAPI returned no image.".into();
    };
    if let Some(parent) = dest.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return format!("Error: {e}");
        }
    }
    if let Err(e) = std::fs::write(&dest, &bytes) {
        return format!("Error: {e}");
    }
    format!("Saved image to {rel_path} ({} KB) using {model}. Reference it from your HTML/CSS at that path.", bytes.len() / 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_vendors() {
        assert_eq!(display_name("claude-sonnet-5"), "Claude Sonnet 5");
        assert_eq!(display_name("free/glm-5.3"), "GLM 5.3 (free)");
        assert_eq!(display_name("kimi-k2.6"), "Kimi K2.6");
        assert_eq!(display_name("gpt-5.5"), "GPT 5.5");
        assert_eq!(vendor_for("free/deepseek-v4-flash"), "DeepSeek");
        assert_eq!(vendor_for("something-new"), "Other");
    }

    #[test]
    fn parses_and_filters() {
        let body = json!({"data":[
            {"id":"glm-5.3","type":"chat","context_window":1000000,"price_in_per_m":0.15,"price_out_per_m":0.5,"tags":"payg","available_for_account":true},
            {"id":"gpt-image-2","type":"image","min_charge_usd":0.005,"tags":"payg,flat"},
            {"id":"free/glm-5.3","type":"chat","tags":"free","is_free":true,"price_in_per_m":0,"price_out_per_m":0},
            {"id":"veo-3","type":"video","min_charge_usd":0.4,"tags":"payg,flat"},
            {"id":"tts-1","type":"audio"},
            {"id":"tokv2/x","type":"chat","tags":"tokv2"},
            {"id":"locked","type":"chat","available_for_account":false}
        ]});
        let m = parse_models(&body);
        assert_eq!(m.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), vec!["gpt-image-2", "veo-3", "glm-5.3", "free/glm-5.3"]);
        assert!(m[3].free && m[3].price_in == 0.0);
        assert_eq!(m[1].kind, "video");
        assert!(!m[2].free);
        assert_eq!(m[0].price_in, 0.005);
        assert_eq!(completion_candidates(&m), vec!["glm-5.3", "free/glm-5.3"]);
        assert_eq!(pick_image_model(&m).as_deref(), Some("gpt-image-2"));
        assert_eq!(size_for("gpt-image-2", Some("16:9")), Some("1536x1024"));
        assert_eq!(size_for("nano-banana-pro", Some("16:9")), None);
    }
}
