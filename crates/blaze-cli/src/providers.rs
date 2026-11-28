//! `blaze providers ...`: bring your own OpenAI-compatible provider.
//!
//! Requests to a provider you add go straight from this computer to its base
//! URL with your key; they never pass through BlazeAPI and are not billed to
//! your BlazeAPI balance. Shared with Blaze IDE (Settings > Custom providers).

use blaze_core::providers::{self, CustomProvider};
use clap::Subcommand;

use crate::config;
use crate::session::Session;

#[derive(Subcommand)]
pub enum ProvidersCmd {
    /// List your providers and their models.
    List,
    /// Add a provider: blaze providers add "My Ollama" http://localhost:11434/v1
    Add {
        name: String,
        /// OpenAI-compatible base URL (https://..., or http://localhost...).
        base_url: String,
        /// Read the API key from stdin (or set it later with `blaze providers key`).
        #[arg(long)]
        key: bool,
        /// Model ids to offer, comma separated (default: ask the provider's /models).
        #[arg(long = "models", value_delimiter = ',')]
        models: Vec<String>,
        /// Which endpoint it speaks: auto (default), chat, responses or anthropic.
        #[arg(long, default_value = "auto")]
        wire: String,
    },
    /// Choose which endpoint a provider speaks: auto, chat, responses or anthropic.
    Wire { provider: String, wire: String },
    /// Set or replace a provider's API key (read from stdin).
    Key { provider: String },
    /// Refresh a provider's model list from its /models endpoint.
    Models { provider: String },
    /// Remove a provider (and its key).
    Rm { provider: String },
}

fn find(s: &Session, key: &str) -> Option<CustomProvider> {
    s.store.providers().into_iter().find(|p| p.id == key || p.name.eq_ignore_ascii_case(key))
}

fn read_key() -> Result<String, String> {
    eprint!("API key (input is not shown when typed; paste and press Enter): ");
    let k = crate::serve::read_secret_line()?;
    let k = k.trim().to_string();
    if k.is_empty() { Err("No key given".into()) } else { Ok(k) }
}

pub async fn run(s: &mut Session, cmd: ProvidersCmd) -> Result<(), String> {
    match cmd {
        ProvidersCmd::List => {
            let list = s.store.providers();
            if list.is_empty() {
                println!("No providers yet. Add one:\n  blaze providers add \"My Ollama\" http://localhost:11434/v1\n  blaze providers add OpenRouter https://openrouter.ai/api/v1 --key");
            }
            for p in list {
                let has_key = config::load_provider_key(&p.id).is_some();
                println!("{}  {}  ({} models, {}{})", p.name, p.base_url, p.models.len(), if p.wire.is_empty() { "endpoint auto" } else { p.wire.as_str() }, if has_key { ", key saved" } else { ", no key" });
                for m in p.models.iter().take(12) {
                    println!("    {}", providers::model_id(&p.id, m));
                }
            }
            println!("\nUse one with:  blaze -m custom:<provider id>/<model>   (or /model in the chat)");
            println!("Requests to these go straight to the provider, not through BlazeAPI.");
            Ok(())
        }
        ProvidersCmd::Wire { provider, wire } => {
            let mut p = find(s, &provider).ok_or("No such provider (see blaze providers list)")?;
            p.wire = wire;
            let p = providers::sanitize(p).map_err(|e| e.to_string())?;
            s.store.upsert_provider(p.clone()).map_err(|e| e.to_string())?;
            println!("{} now uses {}.", p.name, if p.wire.is_empty() { "automatic endpoint detection" } else { p.wire.as_str() });
            Ok(())
        }
        ProvidersCmd::Add { name, base_url, key, models, wire } => {
            let key = if key { Some(read_key()?) } else { None };
            let mut p = providers::sanitize(CustomProvider { name, base_url, models, wire, ..Default::default() }).map_err(|e| e.to_string())?;
            if p.models.is_empty() {
                match providers::fetch_models(&s.http_custom, &p.base_url, key.as_deref()).await {
                    Ok(m) if !m.is_empty() => p.models = m,
                    Ok(_) => eprintln!("The provider listed no models; add them with --models a,b."),
                    Err(e) => eprintln!("Could not list its models ({e}); add them with --models a,b."),
                }
                p = providers::sanitize(p).map_err(|e| e.to_string())?;
            }
            if let Ok(catalog) = providers::fetch_catalog(&s.http_custom, &p.base_url, key.as_deref()).await {
                providers::apply_catalog(&mut p, &catalog);
            }
            if let Some(k) = &key {
                config::save_secret(&format!("provider-{}", p.id), k)?;
            }
            s.store.upsert_provider(p.clone()).map_err(|e| e.to_string())?;
            println!("Added {} ({} models). It talks to {} directly; nothing goes through BlazeAPI.", p.name, p.models.len(), p.base_url);
            if let Some(m) = p.models.first() {
                println!("Try it:  blaze -m {}", providers::model_id(&p.id, m));
            }
            Ok(())
        }
        ProvidersCmd::Key { provider } => {
            let p = find(s, &provider).ok_or("No such provider (see blaze providers list)")?;
            config::save_secret(&format!("provider-{}", p.id), &read_key()?)?;
            println!("Saved the key for {}.", p.name);
            Ok(())
        }
        ProvidersCmd::Models { provider } => {
            let mut p = find(s, &provider).ok_or("No such provider (see blaze providers list)")?;
            let key = config::load_provider_key(&p.id);
            let catalog = providers::fetch_catalog(&s.http_custom, &p.base_url, key.as_deref()).await.map_err(|e| e.to_string())?;
            p.models = catalog.iter().map(|(id, _)| id.clone()).take(500).collect();
            let mut p = providers::sanitize(p).map_err(|e| e.to_string())?;
            providers::apply_catalog(&mut p, &catalog);
            s.store.upsert_provider(p.clone()).map_err(|e| e.to_string())?;
            println!("{} now offers {} models.", p.name, p.models.len());
            Ok(())
        }
        ProvidersCmd::Rm { provider } => {
            let p = find(s, &provider).ok_or("No such provider (see blaze providers list)")?;
            s.store.remove_provider(&p.id).map_err(|e| e.to_string())?;
            config::delete_secret(&format!("provider-{}", p.id));
            println!("Removed {}.", p.name);
            Ok(())
        }
    }
}
