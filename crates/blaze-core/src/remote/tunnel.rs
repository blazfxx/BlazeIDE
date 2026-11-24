//! Cloudflare tunnels for Continue on mobile:
//! - quick tunnel: a free random https://*.trycloudflare.com address, no
//!   account, new address each start;
//! - named tunnel: the user's own domain through their Cloudflare tunnel
//!   (they paste the tunnel token; the hostname points at
//!   http://localhost:<port> in their Cloudflare dashboard).
//!
//! `cloudflared` is downloaded once from Cloudflare's GitHub release, a
//! pinned version checked against its SHA-256, into the app's config folder.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use crate::error::{Error, Result};

pub const CLOUDFLARED_VERSION: &str = "2026.11.0";

struct Asset {
    name: &'static str,
    sha256: &'static str,
    tgz: bool,
}

fn asset() -> Option<Asset> {
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    Some(match (os, arch) {
        ("macos", "aarch64") => Asset { name: "cloudflared-darwin-arm64.tgz", sha256: "8c2f90277ef06a6080bb40b964d33f09f1188102f8798f2e344174b6cf41accf", tgz: true },
        ("macos", "x86_64") => Asset { name: "cloudflared-darwin-amd64.tgz", sha256: "8a59d51120cebbe902f0a8d6e1b7a7fd4f9a5bb4dcb3e0f05872483e75e7db6b", tgz: true },
        ("windows", "x86_64") => Asset { name: "cloudflared-windows-amd64.exe", sha256: "d5c1d82031da6253dcdcff64c944cd0fe063016edeb2e4fd088075fc9bd52d0a", tgz: false },
        ("linux", "x86_64") => Asset { name: "cloudflared-linux-amd64", sha256: "51f7c68b57ed7269164075d7e789efce5b59355982f06392deeb037d982a51fd", tgz: false },
        ("linux", "aarch64") => Asset { name: "cloudflared-linux-arm64", sha256: "abe54a006acf9faf2d19ee9d953cb0d0804e92f59c5382a162e57b84d086f56f", tgz: false },
        _ => return None,
    })
}

fn bin_path(dir: &Path) -> PathBuf {
    dir.join("bin").join(format!("cloudflared-{CLOUDFLARED_VERSION}{}", if cfg!(windows) { ".exe" } else { "" }))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Path of a verified cloudflared, downloading it the first time.
pub async fn ensure_cloudflared(http: &reqwest::Client, config_dir: &Path) -> Result<PathBuf> {
    let a = asset().ok_or_else(|| Error::msg("Tunnels are not available for this computer's platform"))?;
    let bin = bin_path(config_dir);
    if bin.is_file() {
        return Ok(bin);
    }
    let url = format!("https://github.com/cloudflare/cloudflared/releases/download/{CLOUDFLARED_VERSION}/{}", a.name);
    let resp = http.get(&url).timeout(Duration::from_secs(300)).send().await?;
    if !resp.status().is_success() {
        return Err(Error::msg(format!("Could not download cloudflared (HTTP {})", resp.status().as_u16())));
    }
    let bytes = resp.bytes().await?;
    if bytes.len() > 120 * 1024 * 1024 {
        return Err(Error::msg("cloudflared download is unexpectedly large"));
    }
    if hex(&Sha256::digest(&bytes)) != a.sha256 {
        return Err(Error::msg("The cloudflared download did not match its checksum; not using it"));
    }
    let data = if a.tgz {
        let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(&bytes[..]));
        let mut found = None;
        for e in ar.entries()? {
            let mut e = e?;
            if e.path()?.file_name().is_some_and(|n| n == "cloudflared") {
                let mut v = Vec::new();
                std::io::Read::read_to_end(&mut e, &mut v)?;
                found = Some(v);
                break;
            }
        }
        found.ok_or_else(|| Error::msg("cloudflared archive had no binary"))?
    } else {
        bytes.to_vec()
    };
    std::fs::create_dir_all(bin.parent().expect("bin dir"))?;
    let tmp = bin.with_extension("download");
    std::fs::write(&tmp, &data)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&tmp, &bin)?;
    Ok(bin)
}

/// Find the quick tunnel address in a cloudflared log line.
pub fn find_quick_url(line: &str) -> Option<String> {
    let i = line.find("https://")?;
    let rest = &line[i..];
    let end = rest.find(|c: char| c.is_whitespace() || c == '|' || c == '"').unwrap_or(rest.len());
    let url = &rest[..end];
    let host = url.trim_start_matches("https://").trim_end_matches('/');
    (host.ends_with(".trycloudflare.com") && host != "api.trycloudflare.com" && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')).then(|| format!("https://{host}/"))
}

pub struct Tunnel {
    pub child: Child,
    pub url: Option<String>,
}

fn base(bin: &Path) -> Command {
    let mut c = Command::new(bin);
    c.arg("tunnel").arg("--no-autoupdate");
    c.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(windows)]
    {
        c.creation_flags(0x0800_0000);
    }
    c
}

/// Start a quick tunnel to 127.0.0.1:`port` and wait for its address.
pub async fn quick(bin: &Path, port: u16) -> Result<Tunnel> {
    let mut cmd = base(bin);
    cmd.arg("--url").arg(format!("http://127.0.0.1:{port}"));
    let mut child = cmd.spawn().map_err(|e| Error::msg(format!("Could not start cloudflared: {e}")))?;
    let stderr = child.stderr.take().ok_or_else(|| Error::msg("cloudflared has no log"))?;
    let mut lines = BufReader::new(stderr).lines();
    let found = tokio::time::timeout(Duration::from_secs(60), async {
        while let Ok(Some(l)) = lines.next_line().await {
            if let Some(u) = find_quick_url(&l) {
                return Some(u);
            }
        }
        None
    })
    .await
    .ok()
    .flatten();
    let Some(url) = found else {
        let _ = child.kill().await;
        return Err(Error::msg("The quick tunnel did not come up (Cloudflare may be rate limiting quick tunnels). Try again in a minute."));
    };
    // Keep draining the log so cloudflared never blocks on a full pipe.
    tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
    Ok(Tunnel { child, url: Some(url) })
}

/// Run the user's named tunnel. The token goes in through the environment,
/// not the command line (other users could read that in a process list).
pub async fn named(bin: &Path, token: &str) -> Result<Tunnel> {
    let mut cmd = base(bin);
    cmd.arg("run").env("TUNNEL_TOKEN", token);
    let mut child = cmd.spawn().map_err(|e| Error::msg(format!("Could not start cloudflared: {e}")))?;
    if let Some(stderr) = child.stderr.take() {
        let mut lines = BufReader::new(stderr).lines();
        tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
    }
    Ok(Tunnel { child, url: None })
}

/// Check that `url` reaches THIS machine's phone server (its /healthz answers
/// with our id).
pub async fn verify(http: &reqwest::Client, url: &str, server_id: &str) -> Result<()> {
    let u = format!("{}/healthz", url.trim_end_matches('/'));
    let r = http.get(&u).timeout(Duration::from_secs(15)).send().await.map_err(|e| Error::msg(format!("{url} did not answer: {e}")))?;
    let v: serde_json::Value = r.json().await.map_err(|_| Error::msg(format!("{url} did not answer like Blaze")))?;
    if v.get("id").and_then(|i| i.as_str()) == Some(server_id) {
        Ok(())
    } else {
        Err(Error::msg(format!("{url} answers, but not from this computer")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_quick_url() {
        let l = "2026-11-24T10:00:00Z INF |  https://quiet-river-1234.trycloudflare.com                       |";
        assert_eq!(find_quick_url(l).as_deref(), Some("https://quiet-river-1234.trycloudflare.com/"));
        assert_eq!(find_quick_url("INF Requesting new quick Tunnel on trycloudflare.com..."), None);
        assert_eq!(find_quick_url("see https://api.trycloudflare.com/tunnel"), None);
        assert_eq!(find_quick_url("https://evil.com/?x=.trycloudflare.com"), None);
        assert!(asset().is_some() || cfg!(not(any(target_os = "linux", target_os = "macos", target_os = "windows"))));
    }
}
