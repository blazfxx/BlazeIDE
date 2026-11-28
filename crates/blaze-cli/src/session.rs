//! Everything a CLI run needs: the shared store, the key, models, and the
//! chat being worked on (saved in the IDE's format so either can resume it).

use std::path::Path;
use std::sync::Arc;

use blaze_core::agent::{AgentConfig, Approval, Backend};
pub use blaze_core::chatfile::{ChatFile as Chat, DEFAULT_MODEL};
use blaze_core::models::{self, ModelEntry};
use blaze_core::store::Store;
use serde_json::{Map, Value};

use crate::config;

pub struct Session {
    pub store: Store,
    pub http: blaze_core::reqwest::Client,
    pub http_custom: blaze_core::reqwest::Client,
    pub api_base: String,
    pub v1_base: String,
    pub key: Option<String>,
    pub models: Vec<ModelEntry>,
}


impl Session {
    pub fn open() -> Self {
        let dir = config::config_dir();
        Session {
            store: Store::open(&dir),
            http: blaze_core::http_client(),
            http_custom: blaze_core::custom_http_client(),
            api_base: blaze_core::api_base(),
            v1_base: blaze_core::v1_base(),
            key: config::load_key(),
            models: Vec::new(),
        }
    }

    pub fn setting_str(&self, key: &str) -> String {
        self.store.settings().get(key).and_then(Value::as_str).unwrap_or("").to_string()
    }

    pub fn setting_bool(&self, key: &str, default: bool) -> bool {
        self.store.settings().get(key).and_then(Value::as_bool).unwrap_or(default)
    }

    pub fn set_setting(&self, key: &str, v: Value) {
        let mut p = Map::new();
        p.insert(key.to_string(), v);
        let _ = self.store.patch_settings(p);
    }

    pub fn account_name(&self) -> Option<String> {
        let a = self.store.account()?;
        a.get("email").and_then(Value::as_str).filter(|s| !s.is_empty()).or_else(|| a.get("username").and_then(Value::as_str).filter(|s| !s.is_empty())).map(str::to_string)
    }

    pub fn is_admin(&self) -> bool {
        self.store.account().and_then(|a| a.get("username").and_then(Value::as_str).map(|u| u.eq_ignore_ascii_case("blazfoxx"))).unwrap_or(false)
    }

    /// Same rule as the IDE: "cheapest" only for the owner account.
    pub fn routing(&self) -> String {
        match self.setting_str("aiRouting").as_str() {
            "normal" => "normal".into(),
            "direct" => "direct".into(),
            "cheapest" if self.is_admin() => "cheapest".into(),
            "cheapest" => "normal".into(),
            _ => String::new(),
        }
    }

    /// The account's models (+ custom providers). Falls back to the cached
    /// list when offline.
    pub async fn load_models(&mut self, force: bool) -> Result<(), String> {
        if !force && !self.models.is_empty() {
            return Ok(());
        }
        let fetched = models::fetch_models(&self.http, &self.v1_base, self.key.as_deref(), &self.routing()).await;
        let mut err = None;
        let mut list = match fetched {
            Ok((fresh, raw)) if !fresh.is_empty() => {
                let _ = self.store.set_model_cache(raw);
                fresh
            }
            other => {
                let disk = self.store.model_cache().map(|v| models::parse_models(&v)).unwrap_or_default();
                if disk.is_empty() {
                    err = Some(match other {
                        Err(e) => e.to_string(),
                        Ok(_) => "BlazeAPI returned no models for this account (zero balance?).".into(),
                    });
                }
                disk
            }
        };
        // Your own providers work even when BlazeAPI cannot be reached.
        for p in self.store.providers() {
            list.extend(blaze_core::providers::entries(&p));
        }
        self.models = list;
        match err {
            Some(e) if self.models.is_empty() => Err(e),
            _ => Ok(()),
        }
    }

    pub fn backend(&self) -> Backend {
        Backend {
            http: self.http.clone(),
            http_custom: self.http_custom.clone(),
            v1_base: self.v1_base.clone(),
            key: self.key.clone(),
            routing: self.routing(),
            providers: self.store.providers(),
            provider_key: Arc::new(config::load_provider_key),
            models: self.models.clone(),
        }
    }

    pub fn default_model(&self) -> String {
        let m = self.setting_str("cliModel");
        if !m.is_empty() {
            return m;
        }
        let m = self.setting_str("selectedModel");
        if m.is_empty() { DEFAULT_MODEL.into() } else { m }
    }

    pub fn default_approval(&self) -> Approval {
        Approval::parse(&self.setting_str("cliApproval")).unwrap_or(Approval::Edits)
    }

    pub fn agent_config(&self, root: &Path, model: &str, mode: &str, approval: Approval) -> AgentConfig {
        let mut cfg = AgentConfig::new(root.to_path_buf(), model, mode);
        cfg.approval = approval;
        let effort = self.setting_str("reasoningEffort");
        cfg.reasoning_effort = Some(if effort.is_empty() { "medium".into() } else { effort });
        cfg.image_gen = self.setting_bool("aiMediaGen", false);
        cfg.video_gen = self.setting_bool("aiVideoGen", false);
        cfg.image_model = self.setting_str("imageModel");
        cfg.video_model = self.setting_str("videoModel");
        cfg.extra_system = Some(
            "You are running in the `blaze` command-line app (no editor window, no preview browser). \
             Keep answers compact for a terminal: short paragraphs, plain markdown."
                .into(),
        );
        blaze_core::skills::tool::for_chat(&mut cfg, &config::config_dir());
        if self.setting_bool("webAccess", true) {
            blaze_core::web::tool::attach(&mut cfg, "cli");
        }
        cfg
    }

    pub fn find_model(&self, id: &str) -> Option<&ModelEntry> {
        self.models.iter().find(|m| m.id == id)
    }
}

/// Short price text for a model ("$3.00 / $15.00 per 1M", "free").
pub fn price_text(m: &ModelEntry) -> String {
    if m.free {
        return "free".into();
    }
    if m.price_in <= 0.0 && m.price_out <= 0.0 {
        return String::new();
    }
    format!("${:.2} / ${:.2} per 1M", m.price_in, m.price_out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prices() {
        let mut m = ModelEntry { id: "x".into(), display_name: "X".into(), provider: String::new(), context_window: 0, kind: "chat".into(), vision: false, free: true, price_in: 0.0, price_out: 0.0, tags: vec![] };
        assert_eq!(price_text(&m), "free");
        m.free = false;
        m.price_in = 3.0;
        m.price_out = 15.0;
        assert_eq!(price_text(&m), "$3.00 / $15.00 per 1M");
    }
}
