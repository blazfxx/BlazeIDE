//! Where the CLI keeps things. It shares the IDE's config folder (settings,
//! custom providers, BlBots) and its secrets, so signing in once covers both:
//! Credential Manager on Windows, private 0600 files in the config folder on
//! macOS and Linux.

use std::path::PathBuf;

pub const APP_ID: &str = "org.blazeapi.ide";

/// Same folder Tauri's `app_config_dir()` gives the IDE.
pub fn config_dir() -> PathBuf {
    if let Ok(d) = std::env::var("BLAZE_CONFIG_DIR") {
        if !d.trim().is_empty() {
            return PathBuf::from(d);
        }
    }
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join(APP_ID)
}

/// Windows: Credential Manager (shared with Blaze IDE, never prompts).
#[cfg(target_os = "windows")]
mod store {
    const SERVICE: &str = "org.blazeapi.ide";

    fn entry(account: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, account).map_err(|e| format!("Could not open the credential store: {e}"))
    }
    pub fn load(account: &str) -> Option<String> {
        entry(account).ok()?.get_password().ok().filter(|k| !k.is_empty())
    }
    pub fn save(account: &str, key: &str) -> Result<(), String> {
        entry(account)?.set_password(key).map_err(|e| format!("Could not save to the credential store: {e}"))
    }
    pub fn delete(account: &str) {
        if let Ok(e) = entry(account) {
            let _ = e.delete_credential();
        }
    }
}

/// macOS: a private file shared with Blaze IDE (see blaze_core::secret_file
/// for why not the Keychain); old Keychain items are imported once.
#[cfg(target_os = "macos")]
mod store {
    use blaze_core::secret_file as f;
    pub fn load(account: &str) -> Option<String> {
        f::take_once(account, || keyring::Entry::new("org.blazeapi.ide", account).ok()?.get_password().ok())
    }
    pub fn save(account: &str, key: &str) -> Result<(), String> {
        f::save(account, key)
    }
    pub fn delete(account: &str) {
        f::delete(account)
    }
}

/// Linux: a private file (servers usually have no keyring).
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod store {
    pub use blaze_core::secret_file::{delete, load, save};
}

const BLAZE_ACCOUNT: &str = "blazeapi-key";

/// `BLAZE_API_KEY` wins (CI, servers), then the stored key.
pub fn load_key() -> Option<String> {
    if let Ok(k) = std::env::var("BLAZE_API_KEY") {
        if !k.trim().is_empty() {
            return Some(k.trim().to_string());
        }
    }
    store::load(BLAZE_ACCOUNT)
}

pub fn save_key(key: &str) -> Result<(), String> {
    store::save(BLAZE_ACCOUNT, key)
}

pub fn delete_key() {
    store::delete(BLAZE_ACCOUNT)
}

/// Other secrets (e.g. the Cloudflare tunnel token for a custom domain).
pub fn load_secret(name: &str) -> Option<String> {
    store::load(name)
}

pub fn save_secret(name: &str, value: &str) -> Result<(), String> {
    store::save(name, value)
}

pub fn delete_secret(name: &str) {
    store::delete(name)
}

pub fn load_provider_key(id: &str) -> Option<String> {
    store::load(&format!("provider-{id}"))
}
