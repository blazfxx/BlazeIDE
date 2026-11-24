//! Continue on mobile: who may use the phone app.
//!
//! - One password per machine, chosen by the user, stored only as an argon2id
//!   hash in the shared store (section `mobile`).
//! - Signing in (password or a one-time pairing code from a QR) gives a
//!   random 32-byte session token in an HttpOnly cookie; only its SHA-256 is
//!   stored. Sessions last 30 days and can be revoked one by one.
//! - Wrong passwords are limited per client (5 per 10 minutes) and overall
//!   (30 per 10 minutes), so even a public address cannot be brute-forced.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::store::Store;

pub const SECTION: &str = "mobile";
pub const SESSION_DAYS: u64 = 30;
pub const MIN_PASSWORD: usize = 10;
const PER_CLIENT_FAILS: usize = 5;
const GLOBAL_FAILS: usize = 30;
const FAIL_WINDOW: Duration = Duration::from_secs(600);
const PAIR_TTL: Duration = Duration::from_secs(300);

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: String,
    /// SHA-256 of the session token (hex).
    pub token_hash: String,
    pub name: String,
    pub created: u64,
    pub last_seen: u64,
}

/// What `Store::section("mobile")` holds.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub password_hash: Option<String>,
    #[serde(default)]
    pub devices: Vec<Device>,
    /// Port the phone server listens on.
    #[serde(default = "default_port")]
    pub port: u16,
    /// Ways in.
    #[serde(default)]
    pub lan: bool,
    #[serde(default)]
    pub quick_tunnel: bool,
    #[serde(default)]
    pub relay: bool,
    /// Custom domain (https://ide.example.com) the user points at this machine.
    #[serde(default)]
    pub domain: Option<String>,
    /// The domain is served through a Cloudflare tunnel whose token is kept
    /// in the keychain (never here).
    #[serde(default)]
    pub domain_tunnel: bool,
    /// Stable id of this machine on the BlazeAPI relay.
    #[serde(default)]
    pub relay_slot: Option<String>,
    /// Name shown on the phone.
    #[serde(default)]
    pub name: Option<String>,
}

pub fn default_port() -> u16 {
    47900
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn load(store: &Store) -> MobileConfig {
    store.section(SECTION).and_then(|v| serde_json::from_value(v).ok()).unwrap_or(MobileConfig { port: default_port(), ..Default::default() })
}

pub fn update<R>(store: &Store, f: impl FnOnce(&mut MobileConfig) -> Result<R>) -> Result<R> {
    store.update_section(SECTION, |v| {
        let mut cfg: MobileConfig = serde_json::from_value(v.clone()).unwrap_or(MobileConfig { port: default_port(), ..Default::default() });
        let out = f(&mut cfg)?;
        *v = serde_json::to_value(&cfg)?;
        Ok(out)
    })
}

pub fn hash_password(pw: &str) -> Result<String> {
    if pw.chars().count() < MIN_PASSWORD {
        return Err(Error::msg(format!("Use at least {MIN_PASSWORD} characters")));
    }
    if pw.len() > 256 {
        return Err(Error::msg("That password is too long"));
    }
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default().hash_password(pw.as_bytes(), &salt).map(|h| h.to_string()).map_err(|e| Error::msg(format!("Could not hash the password: {e}")))
}

pub fn verify_password(hash: &str, pw: &str) -> bool {
    if pw.len() > 256 {
        return false;
    }
    PasswordHash::new(hash).map(|h| Argon2::default().verify_password(pw.as_bytes(), &h).is_ok()).unwrap_or(false)
}

pub fn set_password(store: &Store, pw: &str) -> Result<()> {
    let hash = hash_password(pw)?;
    update(store, |c| {
        c.password_hash = Some(hash);
        // A new password signs every phone out.
        c.devices.clear();
        Ok(())
    })
}

pub fn random_token() -> String {
    use rand::RngCore;
    let mut b = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut b);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

pub fn sha256_hex(s: &str) -> String {
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Constant-time string compare.
pub fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A device name from a User-Agent ("iPhone · Safari").
pub fn device_name(ua: &str) -> String {
    let os = if ua.contains("iPhone") {
        "iPhone"
    } else if ua.contains("iPad") {
        "iPad"
    } else if ua.contains("Android") {
        "Android"
    } else if ua.contains("Mac OS X") {
        "Mac"
    } else if ua.contains("Windows") {
        "Windows"
    } else if ua.contains("Linux") {
        "Linux"
    } else {
        "Device"
    };
    let browser = if ua.contains("Edg/") {
        "Edge"
    } else if ua.contains("CriOS") || ua.contains("Chrome/") {
        "Chrome"
    } else if ua.contains("FxiOS") || ua.contains("Firefox/") {
        "Firefox"
    } else if ua.contains("Safari/") {
        "Safari"
    } else {
        "browser"
    };
    format!("{os} · {browser}")
}

/// Create a session; returns the raw token for the cookie.
pub fn new_session(store: &Store, ua: &str) -> Result<String> {
    let token = random_token();
    let hash = sha256_hex(&token);
    let now = now_secs();
    update(store, |c| {
        c.devices.retain(|d| now.saturating_sub(d.last_seen) < SESSION_DAYS * 86400);
        if c.devices.len() >= 20 {
            c.devices.sort_by_key(|d| d.last_seen);
            c.devices.remove(0);
        }
        c.devices.push(Device { id: format!("d{}", &sha256_hex(&token)[..10]), token_hash: hash, name: device_name(ua), created: now, last_seen: now });
        Ok(())
    })?;
    Ok(token)
}

/// The device for a session token, if valid. Refreshes `last_seen` at most
/// every 10 minutes (avoids a disk write per request).
pub fn check_session(store: &Store, token: &str) -> Option<Device> {
    if token.is_empty() || token.len() > 100 {
        return None;
    }
    let hash = sha256_hex(token);
    let cfg = load(store);
    if !cfg.enabled {
        return None;
    }
    let now = now_secs();
    let dev = cfg.devices.iter().find(|d| same(&d.token_hash, &hash))?.clone();
    if now.saturating_sub(dev.last_seen) > SESSION_DAYS * 86400 {
        return None;
    }
    if now.saturating_sub(dev.last_seen) > 600 {
        let _ = update(store, |c| {
            if let Some(d) = c.devices.iter_mut().find(|d| d.token_hash == hash) {
                d.last_seen = now;
            }
            Ok(())
        });
    }
    Some(dev)
}

pub fn revoke_device(store: &Store, id: &str) -> Result<bool> {
    update(store, |c| {
        let before = c.devices.len();
        c.devices.retain(|d| d.id != id);
        Ok(c.devices.len() != before)
    })
}

/// Failed sign-ins and one-time pairing codes (in memory).
#[derive(Default)]
pub struct Guard {
    fails: Mutex<HashMap<String, Vec<Instant>>>,
    pair_codes: Mutex<Vec<(String, Instant)>>,
}

impl Guard {
    fn prune(list: &mut Vec<Instant>) {
        list.retain(|t| t.elapsed() < FAIL_WINDOW);
    }

    /// Err with a message when this client (or everyone) is locked out.
    pub fn allowed(&self, client: &str) -> std::result::Result<(), String> {
        let mut f = self.fails.lock().expect("guard");
        let mut total = 0;
        for v in f.values_mut() {
            Self::prune(v);
            total += v.len();
        }
        f.retain(|_, v| !v.is_empty());
        if total >= GLOBAL_FAILS {
            return Err("Too many wrong passwords. Sign-in is paused for a few minutes.".into());
        }
        if f.get(client).map_or(0, Vec::len) >= PER_CLIENT_FAILS {
            return Err("Too many wrong passwords from this device. Try again in 10 minutes.".into());
        }
        Ok(())
    }

    pub fn failed(&self, client: &str) {
        self.fails.lock().expect("guard").entry(client.to_string()).or_default().push(Instant::now());
    }

    /// A one-time code for a QR (valid 5 minutes).
    pub fn new_pair_code(&self) -> String {
        let code = random_token();
        let mut p = self.pair_codes.lock().expect("guard");
        p.retain(|(_, t)| t.elapsed() < PAIR_TTL);
        if p.len() >= 10 {
            p.remove(0);
        }
        p.push((code.clone(), Instant::now()));
        code
    }

    /// Use a pairing code (works once).
    pub fn take_pair_code(&self, code: &str) -> bool {
        let mut p = self.pair_codes.lock().expect("guard");
        p.retain(|(_, t)| t.elapsed() < PAIR_TTL);
        match p.iter().position(|(c, _)| same(c, code)) {
            Some(i) => {
                p.remove(i);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_sessions_and_revoke() {
        let d = tempfile::tempdir().unwrap();
        let store = Store::open(d.path());
        assert!(set_password(&store, "short").is_err());
        set_password(&store, "correct horse battery").unwrap();
        let cfg = load(&store);
        let h = cfg.password_hash.clone().unwrap();
        assert!(h.starts_with("$argon2id$"));
        assert!(!h.contains("correct horse"));
        assert!(verify_password(&h, "correct horse battery"));
        assert!(!verify_password(&h, "correct horse batterY"));
        // Sessions only work while mobile access is on.
        let tok = new_session(&store, "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0) Safari/604.1").unwrap();
        assert!(check_session(&store, &tok).is_none());
        update(&store, |c| {
            c.enabled = true;
            Ok(())
        })
        .unwrap();
        let dev = check_session(&store, &tok).unwrap();
        assert_eq!(dev.name, "iPhone · Safari");
        assert!(check_session(&store, "nope").is_none());
        // The token itself is never stored.
        assert!(!serde_json::to_string(&store.section(SECTION)).unwrap().contains(&tok));
        assert!(revoke_device(&store, &dev.id).unwrap());
        assert!(check_session(&store, &tok).is_none());
        // A new password signs everyone out.
        let tok2 = new_session(&store, "x").unwrap();
        assert!(check_session(&store, &tok2).is_some());
        set_password(&store, "another long password").unwrap();
        assert!(check_session(&store, &tok2).is_none());
    }

    #[test]
    fn lockouts_and_pair_codes() {
        let g = Guard::default();
        for _ in 0..PER_CLIENT_FAILS {
            assert!(g.allowed("1.2.3.4").is_ok());
            g.failed("1.2.3.4");
        }
        assert!(g.allowed("1.2.3.4").is_err());
        assert!(g.allowed("5.6.7.8").is_ok());
        for i in 0..GLOBAL_FAILS {
            g.failed(&format!("10.0.0.{i}"));
        }
        assert!(g.allowed("9.9.9.9").is_err(), "global cap");
        let c = g.new_pair_code();
        assert!(!g.take_pair_code("wrong"));
        assert!(g.take_pair_code(&c));
        assert!(!g.take_pair_code(&c), "single use");
    }
}
