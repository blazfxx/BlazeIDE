//! Secrets in private files: `<config>/org.blazeapi.ide/keys/<name>`, folder
//! 0700 and files 0600, readable only by the user's own account.
//!
//! Used on Linux (servers often have no keyring) and on macOS. On macOS the
//! Keychain ties an item to the exact code signature of the app that saved
//! it; Blaze builds are not Developer ID signed, so every new IDE or CLI build
//! looked like a stranger and macOS asked for the login password again. Keys
//! that are still in the Keychain are copied here once (`take_once`).

use std::io::Write;
use std::path::PathBuf;

pub fn dir() -> PathBuf {
    let base = std::env::var("BLAZE_CONFIG_DIR")
        .ok()
        .filter(|d| !d.trim().is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("org.blazeapi.ide"));
    base.join("keys")
}

fn valid(name: &str) -> bool {
    !name.is_empty() && name.len() <= 80 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.') && !name.starts_with('.')
}

pub fn load(name: &str) -> Option<String> {
    if !valid(name) {
        return None;
    }
    std::fs::read_to_string(dir().join(name)).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

pub fn save(name: &str, value: &str) -> Result<(), String> {
    if !valid(name) {
        return Err("Invalid secret name".into());
    }
    let d = dir();
    std::fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    let p = d.join(name);
    let tmp = d.join(format!(".{name}.tmp"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700));
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| e.to_string())?;
        f.write_all(value.as_bytes()).map_err(|e| e.to_string())?;
    }
    #[cfg(not(unix))]
    {
        let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        f.write_all(value.as_bytes()).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, &p).map_err(|e| e.to_string())?;
    mark_taken(name);
    Ok(())
}

pub fn delete(name: &str) {
    if valid(name) {
        let _ = std::fs::remove_file(dir().join(name));
        mark_taken(name);
    }
}

fn marker(name: &str) -> PathBuf {
    dir().join(format!(".{name}.imported"))
}

fn mark_taken(name: &str) {
    let d = dir();
    if std::fs::create_dir_all(&d).is_ok() {
        let _ = std::fs::write(marker(name), b"");
    }
}

/// Load `name`; if it is not here yet, ask `old` (the Keychain) ONCE, ever,
/// and keep what it returns. After that the old store is never asked again,
/// so macOS shows its password prompt at most one more time per key.
pub fn take_once(name: &str, old: impl FnOnce() -> Option<String>) -> Option<String> {
    if let Some(v) = load(name) {
        return Some(v);
    }
    if !valid(name) || marker(name).exists() {
        return None;
    }
    mark_taken(name);
    let v = old().filter(|v| !v.trim().is_empty())?;
    let _ = save(name, v.trim());
    Some(v.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_and_one_time_import() {
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("BLAZE_CONFIG_DIR", tmp.path());
        assert!(load("blazeapi-key").is_none());
        save("blazeapi-key", "sk-1").unwrap();
        assert_eq!(load("blazeapi-key").as_deref(), Some("sk-1"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(dir().join("blazeapi-key")).unwrap().permissions().mode() & 0o777, 0o600);
        }
        delete("blazeapi-key");
        assert!(load("blazeapi-key").is_none());
        assert!(take_once("blazeapi-key", || panic!("a deleted key is never re-imported")).is_none());
        // First load of a key that only exists in the old store: imported once.
        let mut asked = 0;
        assert_eq!(take_once("provider-abc", || { asked += 1; Some("sk-old".into()) }).as_deref(), Some("sk-old"));
        assert_eq!(take_once("provider-abc", || { asked += 1; None }).as_deref(), Some("sk-old"));
        assert_eq!(asked, 1);
        // Missing everywhere: the old store is asked once, then never again.
        assert!(take_once("provider-x", || None).is_none());
        assert!(take_once("provider-x", || panic!("asked twice")).is_none());
        assert!(save("../evil", "x").is_err());
        std::env::remove_var("BLAZE_CONFIG_DIR");
    }
}
