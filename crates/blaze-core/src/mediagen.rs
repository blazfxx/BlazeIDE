//! Image / video generation with fallback across models, shared by the IDE
//! shell, the CLI agent and BlBots. A model that is out of stock or failing
//! never stops the agent: the next candidate is tried.

use std::path::Path;

use crate::models::{self, ModelEntry};

pub struct MediaCtx<'a> {
    pub http: &'a reqwest::Client,
    pub v1_base: &'a str,
    pub key: &'a str,
    pub routing: &'a str,
    pub models: &'a [ModelEntry],
    pub root: &'a Path,
}

/// Generate an image at `rel_path`. `wanted` = the user's chosen image model
/// ("" = automatic); `supplier` applies only to that model.
pub async fn image(ctx: &MediaCtx<'_>, wanted: &str, supplier: &str, rel_path: &str, prompt: &str, aspect: Option<&str>) -> String {
    let candidates = models::image_candidates(ctx.models, wanted);
    if candidates.is_empty() {
        return "Error: no image model is available on your BlazeAPI account.".into();
    }
    let mut failed: Vec<String> = Vec::new();
    for model in candidates.iter().take(4) {
        let forced = if model == wanted && !supplier.is_empty() { Some(supplier) } else { None };
        let job = models::ImageJob { model: Some(model.clone()), supplier: forced, routing: ctx.routing, root: ctx.root, rel_path, prompt, aspect };
        let out = models::generate_image(ctx.http, ctx.v1_base, ctx.key, job).await;
        let retryable = ["Error: image generation failed", "Error: BlazeAPI returned no image", "Error: could not download", "Error: the image data"]
            .iter()
            .any(|p| out.starts_with(p));
        if !retryable {
            return if failed.is_empty() { out } else { format!("{out} (Fell back after: {}.)", failed.join("; ")) };
        }
        failed.push(format!("{model} failed: {}", out.trim_start_matches("Error: ")));
    }
    format!("Error: no image model worked right now ({}). Continue without the image or try again later.", failed.join("; "))
}

/// Generate a clip at `rel_path`; with `image` it animates that project image.
pub async fn video(ctx: &MediaCtx<'_>, wanted: &str, image: Option<&str>, rel_path: &str, prompt: &str) -> String {
    let image = image.filter(|s| !s.is_empty());
    let candidates = models::video_candidates(ctx.models, wanted, image.is_some());
    if candidates.is_empty() {
        return "Error: no video model is available on your BlazeAPI account.".into();
    }
    let mut failed: Vec<String> = Vec::new();
    for model in candidates.iter().take(3) {
        let job = models::VideoJob { model, root: ctx.root, image_rel: image, rel_path, prompt, routing: ctx.routing };
        let out = models::generate_video(ctx.http, ctx.v1_base, ctx.key, job).await;
        if !out.starts_with("Error: video generation failed") {
            return if failed.is_empty() { out } else { format!("{out} (Fell back after: {}.)", failed.join("; ")) };
        }
        failed.push(format!("{model}: {}", out.trim_start_matches("Error: ")));
    }
    format!("Error: no video model worked right now ({}). Continue without the video or try again later.", failed.join("; "))
}
