//! "Sign in with BlazeAPI": OAuth 2.0 authorization code + PKCE (S256) for a
//! native app (RFC 8252). The browser does the sign-in on blazeapi.org; a
//! one-shot listener on 127.0.0.1 catches the redirect. The app then trades the
//! code for a short-lived OAuth token, uses it once to mint its own revocable
//! API key, and keeps only that key (in the OS keychain).

use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};

pub const CLIENT_ID: &str = "blaze-ide";
pub const SCOPES: &str = "identity provider";
/// Registered redirect ports, tried in order (exact-match redirect URIs).
pub const LOOPBACK_PORTS: [u16; 3] = [47823, 47824, 47825];
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(600);

#[derive(Debug, Clone)]
pub struct SignedIn {
    pub api_key: String,
    pub email: String,
    pub username: String,
}

pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

fn random_b64(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

impl Pkce {
    pub fn new() -> Self {
        // 48 random bytes -> 64 base64url chars (RFC 7636 allows 43-128).
        let verifier = random_b64(48);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Pkce { verifier, challenge }
    }
}

impl Default for Pkce {
    fn default() -> Self {
        Self::new()
    }
}

pub fn authorize_url(site: &str, redirect_uri: &str, state: &str, challenge: &str) -> String {
    let mut u = url::Url::parse(&format!("{site}/oauth/consent")).expect("valid site url");
    u.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", SCOPES)
        .append_pair("state", state)
        .append_pair("code_challenge", challenge)
        .append_pair("code_challenge_method", "S256");
    u.to_string()
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

const DONE_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Blaze IDE</title><body style=\"font:16px system-ui;background:#0a0a0a;color:#eee;display:grid;place-items:center;height:100vh;margin:0\"><div style=\"text-align:center\"><h2>You're signed in</h2><p>You can close this tab and return to Blaze IDE.</p></div>";
const FAIL_PAGE: &str = "<!doctype html><meta charset=utf-8><title>Blaze IDE</title><body style=\"font:16px system-ui;background:#0a0a0a;color:#eee;display:grid;place-items:center;height:100vh;margin:0\"><div style=\"text-align:center\"><h2>Sign-in was not completed</h2><p>Return to Blaze IDE and try again.</p></div>";

pub struct Loopback {
    listener: TcpListener,
    pub redirect_uri: String,
}

impl Loopback {
    /// Bind the first free registered port on 127.0.0.1 only (never 0.0.0.0).
    pub async fn bind() -> Result<Self> {
        Self::bind_ports(&LOOPBACK_PORTS).await
    }

    pub async fn bind_ports(ports: &[u16]) -> Result<Self> {
        for port in ports {
            if let Ok(listener) = TcpListener::bind(("127.0.0.1", *port)).await {
                let port = listener.local_addr()?.port();
                return Ok(Loopback { listener, redirect_uri: format!("http://127.0.0.1:{port}/callback") });
            }
        }
        Err(Error::msg("Could not start the sign-in listener: the ports Blaze IDE uses are busy. Close other Blaze IDE windows and try again."))
    }

    /// Wait for the browser to hit /callback with our state. Requests for
    /// other paths (favicon, probes) are answered 404 and ignored.
    pub async fn wait_for_code(self, expected_state: &str, cancel: CancellationToken) -> Result<String> {
        let accept = async {
            loop {
                let (mut sock, _) = self.listener.accept().await?;
                let mut buf = vec![0u8; 8192];
                let n = tokio::time::timeout(Duration::from_secs(5), sock.read(&mut buf)).await.unwrap_or(Ok(0)).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let target = req.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("").to_string();
                let Ok(url) = url::Url::parse(&format!("http://127.0.0.1{target}")) else { continue };
                if url.path() != "/callback" {
                    let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                    continue;
                }
                let q: std::collections::HashMap<String, String> = url.query_pairs().into_owned().collect();
                let state_ok = q.get("state").is_some_and(|s| constant_time_eq(s, expected_state));
                let page = if state_ok && q.contains_key("code") { DONE_PAGE } else { FAIL_PAGE };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n{}",
                    page.len(),
                    page
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                if !state_ok {
                    // A request with the wrong state is not ours (or is forged);
                    // keep waiting for the real one.
                    continue;
                }
                if let Some(err) = q.get("error") {
                    return Err(if err == "access_denied" { Error::msg("Sign-in was cancelled in the browser.") } else { Error::msg(format!("Sign-in failed: {err}")) });
                }
                return q.get("code").cloned().ok_or_else(|| Error::msg("Sign-in failed: no code returned"));
            }
        };
        tokio::select! {
            r = accept => r,
            _ = cancel.cancelled() => Err(Error::msg("Sign-in cancelled.")),
            _ = tokio::time::sleep(SIGN_IN_TIMEOUT) => Err(Error::msg("Sign-in timed out. Try again.")),
        }
    }
}

#[derive(Deserialize)]
struct TokenResp {
    access_token: String,
}
#[derive(Deserialize)]
struct UserInfo {
    #[serde(default)]
    email: String,
    #[serde(default)]
    username: String,
}
#[derive(Deserialize)]
struct KeyResp {
    api_key: String,
}

async fn error_from(resp: reqwest::Response) -> Error {
    let status = resp.status().as_u16();
    let body: serde_json::Value = resp.json().await.unwrap_or_default();
    let msg = body.get("error").and_then(|e| e.as_str().map(str::to_string).or_else(|| e.get("message").and_then(|m| m.as_str()).map(str::to_string)));
    Error::Http { status, message: msg.unwrap_or_else(|| format!("BlazeAPI returned HTTP {status}")) }
}

/// Exchange the code, read the account, mint the IDE's API key.
pub async fn finish(http: &reqwest::Client, site: &str, code: &str, redirect_uri: &str, verifier: &str) -> Result<SignedIn> {
    let resp = http
        .post(format!("{site}/api/oauth/token"))
        .json(&serde_json::json!({
            "grant_type": "authorization_code",
            "client_id": CLIENT_ID,
            "code": code,
            "redirect_uri": redirect_uri,
            "code_verifier": verifier,
        }))
        .send()
        .await?;
    if !resp.status().is_success() {
        return Err(error_from(resp).await);
    }
    let token = resp.json::<TokenResp>().await?.access_token;

    let resp = http.get(format!("{site}/api/oauth/userinfo")).bearer_auth(&token).send().await?;
    if !resp.status().is_success() {
        return Err(error_from(resp).await);
    }
    let user: UserInfo = resp.json().await?;

    let resp = http.post(format!("{site}/api/oauth/provider-key")).bearer_auth(&token).send().await?;
    if !resp.status().is_success() {
        return Err(error_from(resp).await);
    }
    let key = resp.json::<KeyResp>().await?.api_key;
    Ok(SignedIn { api_key: key, email: user.email, username: user.username })
}

/// Full browser sign-in. `open_browser` receives the consent URL.
pub async fn sign_in(http: &reqwest::Client, site: &str, open_browser: impl FnOnce(&str) -> Result<()>, cancel: CancellationToken) -> Result<SignedIn> {
    let pkce = Pkce::new();
    let state = random_b64(32);
    let lb = Loopback::bind().await?;
    let redirect = lb.redirect_uri.clone();
    open_browser(&authorize_url(site, &redirect, &state, &pkce.challenge))?;
    let code = lb.wait_for_code(&state, cancel).await?;
    finish(http, site, &code, &redirect, &pkce.verifier).await
}

/// Best-effort server-side revocation of the IDE's key on sign-out.
pub async fn revoke_key(http: &reqwest::Client, site: &str, api_key: &str) {
    let _ = http.post(format!("{site}/api/oauth/provider-key/revoke")).bearer_auth(api_key).timeout(Duration::from_secs(5)).send().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_shape() {
        let p = Pkce::new();
        assert!(p.verifier.len() >= 43);
        assert_eq!(URL_SAFE_NO_PAD.encode(Sha256::digest(p.verifier.as_bytes())), p.challenge);
        assert_ne!(Pkce::new().verifier, p.verifier);
    }

    #[test]
    fn url_is_well_formed() {
        let u = authorize_url("https://blazeapi.org", "http://127.0.0.1:47823/callback", "st", "ch");
        let parsed = url::Url::parse(&u).unwrap();
        assert_eq!(parsed.path(), "/oauth/consent");
        let q: std::collections::HashMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(q["client_id"], CLIENT_ID);
        assert_eq!(q["scope"], "identity provider");
        assert_eq!(q["code_challenge_method"], "S256");
        assert_eq!(q["redirect_uri"], "http://127.0.0.1:47823/callback");
    }

    async fn hit(port: u16, path: &str) -> String {
        let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
        s.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes()).await.unwrap();
        let mut out = String::new();
        let _ = s.read_to_string(&mut out).await;
        out
    }

    #[tokio::test]
    async fn loopback_ignores_wrong_state_and_returns_code() {
        let lb = Loopback::bind_ports(&[0]).await.unwrap();
        let port: u16 = lb.redirect_uri.split(':').nth(2).unwrap().trim_end_matches("/callback").parse().unwrap();
        let wait = tokio::spawn(lb.wait_for_code("good-state", CancellationToken::new()));
        assert!(hit(port, "/favicon.ico").await.starts_with("HTTP/1.1 404"));
        assert!(hit(port, "/callback?code=evil&state=bad").await.contains("not completed"));
        assert!(hit(port, "/callback?code=the-code&state=good-state").await.contains("signed in"));
        assert_eq!(wait.await.unwrap().unwrap(), "the-code");
    }

    #[tokio::test]
    async fn loopback_denied_and_cancel() {
        let lb = Loopback::bind_ports(&[0]).await.unwrap();
        let port: u16 = lb.redirect_uri.split(':').nth(2).unwrap().trim_end_matches("/callback").parse().unwrap();
        let wait = tokio::spawn(lb.wait_for_code("s", CancellationToken::new()));
        hit(port, "/callback?error=access_denied&state=s").await;
        assert!(wait.await.unwrap().unwrap_err().to_string().contains("cancelled"));

        let lb = Loopback::bind_ports(&[0]).await.unwrap();
        let c = CancellationToken::new();
        let wait = tokio::spawn(lb.wait_for_code("s", c.clone()));
        c.cancel();
        assert!(wait.await.unwrap().is_err());
    }
}
