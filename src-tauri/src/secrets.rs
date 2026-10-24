//! API keys live only in the OS credential store: Keychain on macOS,
//! Credential Manager on Windows. They are never written to disk by us, never
//! sent to the webview, and never logged. The BlazeAPI key has its own slot;
//! each custom provider gets another one named after its id.

const SERVICE: &str = "org.blazeapi.ide";
const BLAZE_ACCOUNT: &str = "blazeapi-key";

fn entry(account: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, account).map_err(|e| format!("Could not open the system keychain: {e}"))
}

fn load(account: &str) -> Option<String> {
    entry(account).ok()?.get_password().ok().filter(|k| !k.is_empty())
}

fn save(account: &str, key: &str) -> Result<(), String> {
    entry(account)?.set_password(key).map_err(|e| format!("Could not save to the system keychain: {e}"))
}

fn delete(account: &str) {
    if let Ok(e) = entry(account) {
        let _ = e.delete_credential();
    }
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
