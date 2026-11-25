//! Updating the `blaze` CLI itself.
//!
//! `blaze update` (or `/update`) installs the newest build now. Setting
//! `cliUpdates`: "auto" (default: a new build is downloaded in the background
//! and used the next time blaze starts), "notify" (just say so) or "off".
//! Checked at most every 6 hours.
//!
//! The version comes from https://blazeapi.org/ide/latest.json (the same feed
//! as the IDE); the archive is checked against SHA256SUMS before it replaces
//! this executable. A blaze that came inside Blaze IDE is left alone: it is
//! updated together with the app.

use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

pub const FEED: &str = "https://blazeapi.org/ide";
const CHECK_EVERY: u64 = 6 * 3600;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    Auto,
    Notify,
    Off,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "" | "auto" => Some(Mode::Auto),
            "notify" | "manual" => Some(Mode::Notify),
            "off" => Some(Mode::Off),
            _ => None,
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Mode::Auto => "auto",
            Mode::Notify => "notify",
            Mode::Off => "off",
        }
    }
}

fn base() -> String {
    std::env::var("BLAZE_UPDATE_BASE").ok().filter(|v| !v.trim().is_empty()).unwrap_or_else(|| FEED.to_string()).trim_end_matches('/').to_string()
}

fn asset() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        Some("blaze-cli-macos-universal.tar.gz")
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some("blaze-cli-windows-x64.zip")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("blaze-cli-linux-x64.tar.gz")
    } else {
        None
    }
}

/// `0.1.31` > `0.1.30`; anything unparsable is never "newer".
pub fn newer(remote: &str, local: &str) -> bool {
    let p = |v: &str| -> Option<Vec<u64>> { v.trim().trim_start_matches('v').split(['.', '-', '+']).take(3).map(|x| x.parse().ok()).collect() };
    match (p(remote), p(local)) {
        (Some(r), Some(l)) => r > l,
        _ => false,
    }
}

/// Why this executable cannot update itself, if it can't.
pub fn managed_by() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let s = exe.to_string_lossy();
    if s.contains(".app/Contents/") {
        return Some("This blaze comes with Blaze IDE and updates together with the app.".into());
    }
    if cfg!(windows) && exe.parent().map(|d| d.join("Blaze IDE.exe").exists() || d.join("blaze-ide.exe").exists()).unwrap_or(false) {
        return Some("This blaze comes with Blaze IDE and updates together with the app.".into());
    }
    if s.contains("/Cellar/") || s.contains("/nix/store/") || s.starts_with("/usr/bin/") {
        return Some("This blaze was installed by a package manager: update it there.".into());
    }
    None
}

pub async fn latest(http: &blaze_core::reqwest::Client) -> Result<String, String> {
    let v: serde_json::Value = http
        .get(format!("{}/latest.json?t={}", base(), now()))
        .send()
        .await
        .map_err(|e| format!("Could not reach blazeapi.org: {e}"))?
        .error_for_status()
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    v.get("version").and_then(|x| x.as_str()).map(str::to_string).ok_or_else(|| "The update feed has no version".into())
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Download, verify and put `version` in place of this executable.
pub async fn install(http: &blaze_core::reqwest::Client, version: &str) -> Result<PathBuf, String> {
    if let Some(why) = managed_by() {
        return Err(why);
    }
    let asset = asset().ok_or("There is no blaze build for this platform yet.")?;
    let q = format!("?v={version}");
    let get = |url: String| async move {
        let r = http.get(&url).send().await.map_err(|e| format!("Download failed: {e}"))?.error_for_status().map_err(|e| format!("Download failed: {e}"))?;
        r.bytes().await.map_err(|e| format!("Download failed: {e}"))
    };
    let sums = get(format!("{}/download/SHA256SUMS{q}", base())).await?;
    let want = String::from_utf8_lossy(&sums)
        .lines()
        .find_map(|l| {
            let mut it = l.split_whitespace();
            let (h, n) = (it.next()?, it.next()?);
            (n.trim_start_matches('*') == asset).then(|| h.to_ascii_lowercase())
        })
        .ok_or("The checksum list does not name this build")?;
    let bytes = get(format!("{}/download/{asset}{q}", base())).await?;
    let got = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect::<String>();
    if got != want {
        return Err("The download did not match its checksum, so it was not installed.".into());
    }
    let bin = extract(asset, &bytes)?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    replace(&exe, &bin)?;
    Ok(exe)
}

fn extract(asset: &str, bytes: &[u8]) -> Result<Vec<u8>, String> {
    let want = if asset.ends_with(".zip") { "blaze.exe" } else { "blaze" };
    if asset.ends_with(".zip") {
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
        let mut f = z.by_name(want).map_err(|_| "The archive has no blaze.exe")?;
        let mut out = Vec::new();
        f.read_to_end(&mut out).map_err(|e| e.to_string())?;
        return Ok(out);
    }
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    for e in ar.entries().map_err(|e| e.to_string())? {
        let mut e = e.map_err(|e| e.to_string())?;
        let path = e.path().map_err(|e| e.to_string())?.to_path_buf();
        if path.file_name().map(|n| n == want).unwrap_or(false) && e.header().entry_type().is_file() {
            let mut out = Vec::new();
            e.read_to_end(&mut out).map_err(|e| e.to_string())?;
            return Ok(out);
        }
    }
    Err("The archive has no blaze binary".into())
}

/// Swap the file at `exe` for `bin`. The running process keeps its old copy
/// (unix: the old inode; Windows: the renamed `.old` file).
pub fn replace(exe: &Path, bin: &[u8]) -> Result<(), String> {
    let dir = exe.parent().ok_or("No folder")?;
    let name = exe.file_name().ok_or("No file name")?.to_string_lossy().into_owned();
    let tmp = dir.join(format!(".{name}.new"));
    std::fs::write(&tmp, bin).map_err(|e| format!("Cannot write next to {}: {e}", exe.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, exe).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("Cannot replace {}: {e}", exe.display())
        })?;
    }
    #[cfg(windows)]
    {
        let old = dir.join(format!("{name}.old"));
        let _ = std::fs::remove_file(&old);
        std::fs::rename(exe, &old).map_err(|e| format!("Cannot move the running blaze aside: {e}"))?;
        if let Err(e) = std::fs::rename(&tmp, exe) {
            let _ = std::fs::rename(&old, exe);
            return Err(format!("Cannot replace {}: {e}", exe.display()));
        }
    }
    Ok(())
}

/// Windows leaves `blaze.exe.old` behind after an update; remove it next time.
pub fn cleanup() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(exe.with_file_name(format!("{}.old", exe.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())));
    }
}

/// What a background check found.
pub enum Found {
    None,
    Available(String),
    Installed(String),
    Failed(String),
}

/// Background check for the TUI: respects the mode and the 6 h spacing.
/// `last` is the unix time of the previous check (setting `cliUpdateChecked`).
pub async fn background(http: &blaze_core::reqwest::Client, mode: Mode, last: u64) -> (Found, Option<u64>) {
    if mode == Mode::Off || now().saturating_sub(last) < CHECK_EVERY || std::env::var("BLAZE_NO_UPDATE").is_ok() {
        return (Found::None, None);
    }
    let stamp = Some(now());
    let Ok(v) = latest(http).await else { return (Found::None, stamp) };
    if !newer(&v, crate::VERSION) {
        return (Found::None, stamp);
    }
    if mode == Mode::Notify || managed_by().is_some() {
        return (Found::Available(v), stamp);
    }
    match install(http, &v).await {
        Ok(_) => (Found::Installed(v), stamp),
        Err(e) => (Found::Failed(format!("Update to {v} failed: {e}")), stamp),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(newer("0.1.32", "0.1.31"));
        assert!(newer("0.2.0", "0.1.99"));
        assert!(!newer("0.1.31", "0.1.31"));
        assert!(!newer("0.1.9", "0.1.31"));
        assert!(!newer("garbage", "0.1.0"));
        assert_eq!(Mode::parse("manual"), Some(Mode::Notify));
        assert_eq!(Mode::parse(""), Some(Mode::Auto));
    }

    #[test]
    fn extracts_and_replaces() {
        let mut gz = Vec::new();
        {
            let enc = flate2::write::GzEncoder::new(&mut gz, flate2::Compression::fast());
            let mut b = tar::Builder::new(enc);
            let data = b"#!/bin/sh\necho new\n";
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o755);
            h.set_cksum();
            b.append_data(&mut h, "blaze", &data[..]).unwrap();
            b.into_inner().unwrap().finish().unwrap();
        }
        let bin = extract("blaze-cli-linux-x64.tar.gz", &gz).unwrap();
        assert_eq!(bin, b"#!/bin/sh\necho new\n");
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("blaze");
        std::fs::write(&exe, b"old").unwrap();
        replace(&exe, &bin).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), bin);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&exe).unwrap().permissions().mode() & 0o777, 0o755);
        }
    }
}
