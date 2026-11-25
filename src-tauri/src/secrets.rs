//! API keys: Credential Manager on Windows; on macOS a private file in the
//! app's config folder (0600, shared with the blaze CLI), because the Keychain
//! asked for the login password after every update of these non-Developer-ID
//! builds. Old Keychain items are imported once. Keys are never sent to the
//! webview and never logged. The BlazeAPI key has its own slot; each custom
//! provider gets another one named after its id.

const SERVICE: &str = "org.blazeapi.ide";
const BLAZE_ACCOUNT: &str = "blazeapi-key";

fn entry(account: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, account).map_err(|e| format!("Could not open the system keychain: {e}"))
}

#[cfg(not(target_os = "macos"))]
fn load(account: &str) -> Option<String> {
    entry(account).ok()?.get_password().ok().filter(|k| !k.is_empty())
}

#[cfg(not(target_os = "macos"))]
fn save(account: &str, key: &str) -> Result<(), String> {
    entry(account)?.set_password(key).map_err(|e| format!("Could not save to the system keychain: {e}"))
}

#[cfg(not(target_os = "macos"))]
fn delete(account: &str) {
    if let Ok(e) = entry(account) {
        let _ = e.delete_credential();
    }
}

#[cfg(target_os = "macos")]
fn load(account: &str) -> Option<String> {
    blaze_core::secret_file::take_once(account, || entry(account).ok()?.get_password().ok())
}

#[cfg(target_os = "macos")]
fn save(account: &str, key: &str) -> Result<(), String> {
    blaze_core::secret_file::save(account, key)
}

#[cfg(target_os = "macos")]
fn delete(account: &str) {
    blaze_core::secret_file::delete(account)
}

pub fn load_key() -> Option<String> {
    load(BLAZE_ACCOUNT)
}

pub fn save_key(key: &str) -> Result<(), String> {
    save(BLAZE_ACCOUNT, key)
}

pub fn delete_key() {
    delete(BLAZE_ACCOUNT);
}

fn provider_account(id: &str) -> String {
    format!("provider-{id}")
}

pub fn load_provider_key(id: &str) -> Option<String> {
    load(&provider_account(id))
}

pub fn save_provider_key(id: &str, key: &str) -> Result<(), String> {
    save(&provider_account(id), key)
}

pub fn delete_provider_key(id: &str) {
    delete(&provider_account(id));
}

/// Other secrets by name (e.g. the Cloudflare tunnel token for Continue on mobile).
pub fn load_secret(name: &str) -> Option<String> {
    load(name)
}

pub fn save_secret(name: &str, value: &str) -> Result<(), String> {
    save(name, value)
}

pub fn delete_secret(name: &str) {
    delete(name);
}
