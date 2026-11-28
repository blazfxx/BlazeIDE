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

/// Tag on a model that is listed but cannot be called until the account has balance.
pub const NEEDS_BALANCE: &str = "needs-balance";

fn has_tag(tags: &str, t: &str) -> bool {
    tags.split(',').any(|x| x.trim() == t)
}

pub fn parse_models(body: &Value) -> Vec<ModelEntry> {
    let Some(rows) = body.get("data").and_then(Value::as_array) else { return Vec::new() };
    let mut out: Vec<ModelEntry> = rows
        .iter()
        .filter_map(|m| {
            let mut id = m.get("id")?.as_str()?.to_string();
            let listed_name = id.clone();
            let kind = match m.get("type").and_then(Value::as_str).unwrap_or("chat") {
                "chat" => "chat",
                "image" => "image",
                "video" => "video",
                _ => return None,
            };
            // Down for everyone: hide. Only waiting for balance: keep, marked, so
            // a user with no balance can still see what exists.
            let mut needs_balance = false;
            if m.get("available_for_account").and_then(Value::as_bool) == Some(false) {
                match m.get("unavailable_reason").and_then(Value::as_str) {
                    Some("insufficient_payg_balance") => needs_balance = true,
                    _ => return None,
                }
            }
            // Video rows are listed under a display name ("Seedance 2.0 (image-to-video)");
            // the callable id is in model_id ("payg/seedance-2-0-image-to-video").
            if kind == "video" {
                if let Some(mid) = m.get("model_id").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                    id = mid.trim_start_matches("payg/").to_string();
                }
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
                display_name: if kind == "video" && listed_name != id { listed_name } else { display_name(&id) },
                provider: vendor_for(&id),
                context_window: m.get("context_window").and_then(Value::as_u64).unwrap_or(0),
                kind: kind.into(),
                vision: m.get("vision").and_then(Value::as_bool).unwrap_or(false),
                free,
                price_in: if free { 0.0 } else if flat { num("min_charge_usd") } else { num("price_in_per_m") },
                price_out: if free { 0.0 } else { num("price_out_per_m") },
                tags: tags_raw.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).chain(needs_balance.then(|| NEEDS_BALANCE.to_string())).collect(),
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
pub async fn fetch_models(http: &reqwest::Client, api_base: &str, api_key: Option<&str>, routing: &str) -> Result<(Vec<ModelEntry>, Value)> {
    // Normal and Direct list the same models under slightly different ids
    // (gpt-6-1-sol vs gpt-6.1-sol), so ask for the list the requests will use.
    let section = match routing {
        // Direct was retired; "trusted" is Normal routing with a provider header.
        "normal" | "cheapest" | "fastest" | "trusted" | "direct" => "?section=normal",
        _ => "",
    };
    let mut req = http.get(format!("{api_base}/paid/v1/models{section}")).timeout(Duration::from_secs(15));
    if let Some(k) = api_key {
        req = req.bearer_auth(k);
    }
    for (k, v) in crate::ai::routing_headers(routing) {
        req = req.header(k, v);
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
    let callable: Vec<ModelEntry> = models.iter().filter(|m| !m.tags.iter().any(|t| t == NEEDS_BALANCE)).cloned().collect();
    let models = &callable[..];
    PREFERRED_COMPLETION_MODELS
        .iter()
        .filter(|p| models.iter().any(|m| m.id == **p && m.kind == "chat"))
        .map(|s| s.to_string())
        .collect()
}

/// Automatic image model order (2026-11-14): models whose supplier was
/// actually delivering first. gpt-image-2 / gpt-image-2.5 come last: their
/// supplier was out of stock when this list was made.
const PREFERRED_IMAGE_MODELS: &[&str] = &["seedream-4.5", "gemini-2.5-flash-image", "nano-banana-pro", "flux.2-flex", "gpt-5-image-mini", "gpt-image-2.5", "gpt-image-2"];

/// Edit-only models need an input image; the agent only does text-to-image.
fn is_edit_model(id: &str) -> bool {
    id.contains("edit")
}

/// Image models to try, in order: the chosen one first (if the account has
/// it), then the automatic order, then any other text-to-image model.
/// A picture file as a data: URL (type from its first bytes; PNG, JPEG,
/// WebP or GIF only).
pub fn image_data_url(bytes: &[u8]) -> Option<String> {
    use base64::Engine;
    let mime = if bytes.starts_with(b"\x89PNG") {
        "image/png"
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "image/webp"
    } else if bytes.starts_with(b"GIF8") {
        "image/gif"
    } else {
        return None;
    };
    Some(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

pub fn image_candidates(models: &[ModelEntry], wanted: &str) -> Vec<String> {
    let callable: Vec<ModelEntry> = models.iter().filter(|m| !m.tags.iter().any(|t| t == NEEDS_BALANCE)).cloned().collect();
    let models = &callable[..];
    let usable = |id: &str| models.iter().any(|m| m.id == id && m.kind == "image") && !is_edit_model(id);
    let mut out: Vec<String> = Vec::new();
    if !wanted.is_empty() && usable(wanted) {
        out.push(wanted.to_string());
    }
    for p in PREFERRED_IMAGE_MODELS {
        if usable(p) && !out.iter().any(|x| x == p) {
            out.push(p.to_string());
        }
    }
    for m in models.iter().filter(|m| m.kind == "image" && !is_edit_model(&m.id)) {
        if !out.contains(&m.id) {
            out.push(m.id.clone());
        }
    }
    out
}

pub fn pick_image_model(models: &[ModelEntry]) -> Option<String> {
    image_candidates(models, "").into_iter().next()
}

/// One BlazeAPI supplier (codename) that serves a model.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SupplierEntry {
    pub id: String,
    pub status: String,
    pub price: f64,
}

/// Suppliers for a model (public `/api/v1/providers?model=`, Normal routing).
pub async fn fetch_suppliers(http: &reqwest::Client, site: &str, model: &str) -> Result<Vec<SupplierEntry>> {
    let resp = http.get(format!("{site}/api/v1/providers")).query(&[("model", model)]).timeout(Duration::from_secs(10)).send().await?;
    if !resp.status().is_success() {
        return Err(Error::Http { status: resp.status().as_u16(), message: "Could not load the suppliers for this model".into() });
    }
    let v: Value = resp.json().await?;
    Ok(v.get("data").and_then(Value::as_array).map(|rows| {
        rows.iter().filter_map(|r| Some(SupplierEntry {
            id: r.get("id")?.as_str()?.to_string(),
            status: r.get("status").and_then(Value::as_str).unwrap_or("").to_string(),
            price: r.get("base_fee_usd").and_then(Value::as_f64).unwrap_or(0.0),
        })).collect()
    }).unwrap_or_default())
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
    /// BlazeAPI supplier codename to force; None = automatic.
    pub supplier: Option<&'a str>,
    /// Routing choice ("normal", "direct", "cheapest", "" = account).
    pub routing: &'a str,
    pub root: &'a Path,
    pub rel_path: &'a str,
    pub prompt: &'a str,
    pub aspect: Option<&'a str>,
}

/// Generate an image and save it inside the project. Returns the tool-result
/// text for the model (never Err, so the agent can carry on).
pub async fn generate_image(http: &reqwest::Client, api_base: &str, api_key: &str, job: ImageJob<'_>) -> String {
    let ImageJob { model, supplier, routing, root, rel_path, prompt, aspect } = job;
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
    let mut post = http.post(format!("{api_base}/paid/v1/images/generations")).bearer_auth(api_key).json(&body).timeout(Duration::from_secs(180));
    if let Some(s) = supplier.filter(|s| !s.is_empty()) {
        // Forcing a supplier is a Normal-routing feature.
        post = post.header("X-Blaze-Routing", "basic").header("X-Blaze-Provider", s);
    } else {
        for (k, v) in crate::ai::routing_headers(routing) {
            post = post.header(k, v);
        }
    }
    let resp = match post.send().await {
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

/// Automatic video model order, fast + good value first. Image-to-video when the
/// agent passes a still, text-to-video when it does not.
const PREFERRED_IMAGE_VIDEO: &[&str] = &["seedance-2-0-fast-image-to-video", "wan-2-6-flash-image-to-video", "kling-o3-standard-image-to-video", "seedance-2-0-image-to-video", "kling-v3-turbo-standard-image-to-video", "wan-2-7-image-to-video", "veo3-1-fast-image-to-video", "kling-2-6-pro-image-to-video"];
const PREFERRED_TEXT_VIDEO: &[&str] = &["kling-o3-standard-text-to-video", "ltx-2-v2-3-fast-text-to-video", "wan-2-5-preview-text-to-video", "kling-v3-standard-text-to-video", "seedance-1-5-pro-text-to-video", "wan-2-7-text-to-video", "veo3-1-fast-text-to-video", "kling-2-6-pro-text-to-video"];

/// Does this video model take a starting image (image-to-video) rather than text only?
pub fn video_takes_image(id: &str) -> bool {
    !id.contains("text-to-video")
}

pub fn video_candidates(models: &[ModelEntry], wanted: &str, with_image: bool) -> Vec<String> {
    let callable: Vec<ModelEntry> = models.iter().filter(|m| !m.tags.iter().any(|t| t == NEEDS_BALANCE)).cloned().collect();
    let models = &callable[..];
    let fits = |id: &str| if with_image { id.contains("image-to-video") } else { id.contains("text-to-video") };
    let has = |id: &str| fits(id) && models.iter().any(|m| m.id == id && m.kind == "video");
    let preferred = if with_image { PREFERRED_IMAGE_VIDEO } else { PREFERRED_TEXT_VIDEO };
    let mut out: Vec<String> = Vec::new();
    if !wanted.is_empty() && has(wanted) {
        out.push(wanted.to_string());
    }
    for p in preferred {
        if has(p) && !out.iter().any(|x| x == p) {
            out.push(p.to_string());
        }
    }
    for m in models.iter().filter(|m| m.kind == "video" && fits(&m.id)) {
        if !out.contains(&m.id) {
            out.push(m.id.clone());
        }
    }
    out
}

/// What to animate and where to save the clip (inside `root`).
pub struct VideoJob<'a> {
    pub model: &'a str,
    pub root: &'a Path,
    /// None = text-to-video.
    pub image_rel: Option<&'a str>,
    pub rel_path: &'a str,
    pub prompt: &'a str,
    pub routing: &'a str,
}

const MAX_VIDEO_SOURCE: u64 = 12 * 1024 * 1024;

/// Animate a project image into a clip with a BlazeAPI video model (POST
/// /paid/v1/video/generations, answer = base64 mp4). Returns the tool-result
/// text; "Error: video generation failed" means another model may work.
pub async fn generate_video(http: &reqwest::Client, api_base: &str, api_key: &str, job: VideoJob<'_>) -> String {
    let VideoJob { model, root, image_rel, rel_path, prompt, routing } = job;
    let dest = match resolve_in(root, rel_path) {
        Ok(p) => p,
        Err(e) => return format!("Error: {e}"),
    };
    let mut body = json!({ "model": model, "prompt": if prompt.trim().is_empty() { "subtle cinematic motion" } else { prompt } });
    if let Some(image_rel) = image_rel {
        let src = match resolve_in(root, image_rel) {
            Ok(p) => p,
            Err(e) => return format!("Error: {e}"),
        };
        let meta = match std::fs::metadata(&src) {
            Ok(m) if m.is_file() => m,
            _ => return format!("Error: the image {image_rel} does not exist. Create it with generate_image first, or leave `image` out for text-to-video."),
        };
        if meta.len() > MAX_VIDEO_SOURCE {
            return "Error: the source image is larger than 12 MB; use a smaller image.".into();
        }
        let mime = match src.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).as_deref() {
            Some("png") => "image/png",
            Some("jpg" | "jpeg") => "image/jpeg",
            Some("webp") => "image/webp",
            _ => return "Error: the source must be a .png, .jpg or .webp image.".into(),
        };
        let bytes = match std::fs::read(&src) {
            Ok(b) => b,
            Err(e) => return format!("Error: {e}"),
        };
        body["image_url"] = json!(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(&bytes)));
    } else if prompt.trim().is_empty() {
        return "Error: text-to-video needs a prompt describing the clip.".into();
    }
    let mut post = http.post(format!("{api_base}/paid/v1/video/generations")).bearer_auth(api_key).json(&body).timeout(Duration::from_secs(660));
    for (k, v) in crate::ai::routing_headers(routing) {
        post = post.header(k, v);
    }
    let resp = match post.send().await {
        Ok(r) => r,
        Err(e) => return format!("Error: video generation failed ({}).", Error::from(e)),
    };
    let status = resp.status();
    let v: Value = resp.json().await.unwrap_or_default();
    let item = &v["data"][0];
    let Some(b64) = item.get("b64_json").and_then(Value::as_str).filter(|_| status.is_success()) else {
        let msg = v.get("error").and_then(|e| e.get("message").and_then(Value::as_str).or_else(|| e.as_str())).unwrap_or("no video returned");
        return format!("Error: video generation failed ({msg}).");
    };
    let Ok(clip) = base64::engine::general_purpose::STANDARD.decode(b64) else { return "Error: video generation failed (invalid video data).".into() };
    if let Some(parent) = dest.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            return format!("Error: {e}");
        }
    }
    if let Err(e) = std::fs::write(&dest, &clip) {
        return format!("Error: {e}");
    }
    let secs = item.get("seconds").and_then(Value::as_f64).map(|s| format!(", {s} s")).unwrap_or_default();
    format!("Saved video to {rel_path} ({} KB{secs}) using {model}. Use it with <video src=\"{rel_path}\" autoplay muted loop playsinline>.", clip.len() / 1024)
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
            {"id":"locked","type":"chat","available_for_account":false},
            {"id":"claude-sonnet-5-5","type":"chat","available_for_account":false,"unavailable_reason":"insufficient_payg_balance","tags":"payg"},
            {"id":"down","type":"chat","available_for_account":false,"unavailable_reason":"model_currently_down"}
        ]});
        let m = parse_models(&body);
        assert_eq!(m.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), vec!["claude-sonnet-5-5", "gpt-image-2", "veo-3", "glm-5.3", "free/glm-5.3"]);
        assert!(m.iter().find(|x| x.id == "claude-sonnet-5-5").unwrap().tags.contains(&NEEDS_BALANCE.to_string()), "kept, marked");
        let vids = parse_models(&json!({"data":[{"id":"Seedance 2.0 (image-to-video)","model_id":"payg/seedance-2-0-image-to-video","type":"video","min_charge_usd":0.29,"tags":"payg,flat"}]}));
        assert_eq!((vids[0].id.as_str(), vids[0].display_name.as_str()), ("seedance-2-0-image-to-video", "Seedance 2.0 (image-to-video)"));
        assert_eq!(video_candidates(&vids, "", true), vec!["seedance-2-0-image-to-video"]);
        assert!(video_candidates(&vids, "", false).is_empty());
        assert!(video_takes_image("kling-o3-standard-reference-to-video") && !video_takes_image("kling-o3-standard-text-to-video"));
        let by = |id: &str| m.iter().find(|x| x.id == id).unwrap();
        assert!(by("free/glm-5.3").free && by("free/glm-5.3").price_in == 0.0);
        assert_eq!(by("veo-3").kind, "video");
        assert!(!by("glm-5.3").free);
        assert_eq!(by("gpt-image-2").price_in, 0.005);
        assert_eq!(completion_candidates(&m), vec!["glm-5.3", "free/glm-5.3"]);
        assert_eq!(pick_image_model(&m).as_deref(), Some("gpt-image-2"));
        let imgs = vec![
            ModelEntry { id: "gpt-image-2".into(), display_name: String::new(), provider: String::new(), context_window: 0, kind: "image".into(), vision: false, free: false, price_in: 0.0, price_out: 0.0, tags: vec![] },
            ModelEntry { id: "seedream-4.5".into(), display_name: String::new(), provider: String::new(), context_window: 0, kind: "image".into(), vision: false, free: false, price_in: 0.0, price_out: 0.0, tags: vec![] },
            ModelEntry { id: "qwen-image-2-edit".into(), display_name: String::new(), provider: String::new(), context_window: 0, kind: "image".into(), vision: false, free: false, price_in: 0.0, price_out: 0.0, tags: vec![] },
        ];
        assert_eq!(image_candidates(&imgs, ""), vec!["seedream-4.5", "gpt-image-2"]);
        assert_eq!(image_candidates(&imgs, "gpt-image-2"), vec!["gpt-image-2", "seedream-4.5"]);
        assert_eq!(image_candidates(&imgs, "gone"), vec!["seedream-4.5", "gpt-image-2"]);
        assert_eq!(size_for("gpt-image-2", Some("16:9")), Some("1536x1024"));
        assert_eq!(size_for("nano-banana-pro", Some("16:9")), None);
    }
}
