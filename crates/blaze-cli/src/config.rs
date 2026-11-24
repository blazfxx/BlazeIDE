//! Where the CLI keeps things. It shares the IDE's config folder (settings,
//! custom providers, BlBots) and, on macOS/Windows, the IDE's keychain slot,
//! so signing in once covers both.
//!
//! Linux has no Blaze IDE build and often no desktop keyring (servers), so the
//! key is kept in a 0600 file in the config folder there.

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

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod store {
    const SERVICE: &str = "org.blazeapi.ide";

    fn entry(account: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, account).map_err(|e| format!("Could not open the system keychain: {e}"))
    }
    pub fn load(account: &str) -> Option<String> {
        entry(account).ok()?.get_password().ok().filter(|k| !k.is_empty())
    }
    pub fn save(account: &str, key: &str) -> Result<(), String> {
        entry(account)?.set_password(key).map_err(|e| format!("Could not save to the system keychain: {e}"))
    }
    pub fn delete(account: &str) {
        if let Ok(e) = entry(account) {
            let _ = e.delete_credential();
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod store {
    use std::io::Write;

    fn path(account: &str) -> std::path::PathBuf {
        super::config_dir().join("keys").join(account)
    }
    pub fn load(account: &str) -> Option<String> {
        std::fs::read_to_string(path(account)).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    }
    pub fn save(account: &str, key: &str) -> Result<(), String> {
        let p = path(account);
        let dir = p.parent().expect("keys dir");
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
            let tmp = p.with_extension("tmp");
            let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| e.to_string())?;
            f.write_all(key.as_bytes()).map_err(|e| e.to_string())?;
            std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
        }
        #[cfg(not(unix))]
        {
            let mut f = std::fs::File::create(&p).map_err(|e| e.to_string())?;
            f.write_all(key.as_bytes()).map_err(|e| e.to_string())
        }
    }
    pub fn delete(account: &str) {
        let _ = std::fs::remove_file(path(account));
    }
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

pub fn load_provider_key(id: &str) -> Option<String> {
    store::load(&format!("provider-{id}"))
}
