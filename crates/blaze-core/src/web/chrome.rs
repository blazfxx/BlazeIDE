//! The agent's browser: the Chrome, Edge, Brave or Chromium already on this
//! computer, run without a window and driven over the DevTools protocol.
//! It gets an empty throw-away profile (none of the user's logins, history or
//! extensions) and can only reach public websites (`proxy`).

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use super::guard::Policy;
use super::proxy::{self, Proxy};

const IDLE: Duration = Duration::from_secs(600);
const MAX_BROWSERS: usize = 3;
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// A Chromium-family browser installed on this computer.
pub fn find() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("BLAZE_BROWSER").map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let mut c: Vec<PathBuf> = Vec::new();
    if cfg!(windows) {
        let roots: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"].iter().filter_map(std::env::var_os).map(PathBuf::from).collect();
        for exe in ["Google/Chrome/Application/chrome.exe", "Microsoft/Edge/Application/msedge.exe", "BraveSoftware/Brave-Browser/Application/brave.exe", "Chromium/Application/chrome.exe"] {
            c.extend(roots.iter().map(|r| r.join(exe)));
        }
    } else if cfg!(target_os = "macos") {
        let mut roots = vec![PathBuf::from("/Applications")];
        roots.extend(dirs::home_dir().map(|h| h.join("Applications")));
        for app in ["Google Chrome.app/Contents/MacOS/Google Chrome", "Microsoft Edge.app/Contents/MacOS/Microsoft Edge", "Brave Browser.app/Contents/MacOS/Brave Browser", "Chromium.app/Contents/MacOS/Chromium"] {
            c.extend(roots.iter().map(|r| r.join(app)));
        }
    } else {
        let path: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
        for exe in ["google-chrome-stable", "google-chrome", "chromium", "chromium-browser", "microsoft-edge", "brave-browser"] {
            c.extend(path.iter().map(|d| d.join(exe)));
        }
    }
    c.into_iter().find(|p| p.is_file())
}

pub const NO_BROWSER: &str = "No Chrome, Edge, Brave or Chromium was found on this computer. web_search and web_fetch still work; install Google Chrome to use the browser.";

fn running_as_root() -> bool {
    cfg!(target_os = "linux") && std::fs::read_to_string("/proc/self/status").is_ok_and(|s| s.lines().any(|l| l.starts_with("Uid:\t0\t")))
}

pub struct Browser {
    ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    child: Child,
    profile: PathBuf,
    _proxy: Proxy,
    next: u64,
    /// The tab being used (a DevTools session id) and its target id.
    session: String,
    target: String,
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
        let dir = self.profile.clone();
        // The browser needs a moment to let go of its files (Windows).
        std::thread::spawn(move || {
            for _ in 0..20 {
                if std::fs::remove_dir_all(&dir).is_ok() || !dir.exists() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        });
    }
}

impl Browser {
    pub async fn launch(policy: Policy) -> Result<Browser, String> {
        let exe = find().ok_or(NO_BROWSER)?;
        let proxy = proxy::start(policy).await?;
        let profile = std::env::temp_dir().join(format!("blaze-browser-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&profile).map_err(|e| e.to_string())?;
        let mut cmd = Command::new(&exe);
        cmd.args([
            "--headless=new",
            "--remote-debugging-port=0",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-background-networking",
            "--disable-component-update",
            "--disable-default-apps",
            "--disable-extensions",
            "--disable-sync",
            "--disable-quic",
            "--mute-audio",
            "--hide-scrollbars",
            "--window-size=1280,900",
            // Nothing may go around the proxy: not localhost, not WebRTC.
            "--proxy-bypass-list=<-loopback>",
            "--force-webrtc-ip-handling-policy=disable_non_proxied_udp",
        ]);
        cmd.arg(format!("--proxy-server=http://127.0.0.1:{}", proxy.port));
        cmd.arg(format!("--user-data-dir={}", profile.display()));
        // Chrome refuses to sandbox itself for the root user (servers).
        if running_as_root() {
            cmd.arg("--no-sandbox");
        }
        cmd.arg("about:blank").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true);
        #[cfg(windows)]
        {
            cmd.creation_flags(0x0800_0000);
        }
        let mut child = cmd.spawn().map_err(|e| format!("Could not start the browser: {e}"))?;
        let port_file = profile.join("DevToolsActivePort");
        let started = Instant::now();
        let endpoint = loop {
            if let Ok(s) = std::fs::read_to_string(&port_file) {
                let mut l = s.lines();
                if let (Some(port), Some(path)) = (l.next().and_then(|p| p.parse::<u16>().ok()), l.next()) {
                    break format!("ws://127.0.0.1:{port}{path}");
                }
            }
            if let Ok(Some(code)) = child.try_wait() {
                let _ = std::fs::remove_dir_all(&profile);
                return Err(format!("The browser closed right away ({code})"));
            }
            if started.elapsed() > Duration::from_secs(25) {
                let _ = child.start_kill();
                let _ = std::fs::remove_dir_all(&profile);
                return Err("The browser did not start in time".into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        let (ws, _) = tokio_tungstenite::connect_async(endpoint.as_str()).await.map_err(|e| format!("Could not talk to the browser: {e}"))?;
        let mut b = Browser { ws, child, profile, _proxy: proxy, next: 0, session: String::new(), target: String::new() };
        b.raw("Browser.setDownloadBehavior", json!({ "behavior": "deny" }), false).await?;
        let created = b.raw("Target.createTarget", json!({ "url": "about:blank" }), false).await?;
        let target = created["targetId"].as_str().ok_or("The browser gave no tab")?.to_string();
        b.attach(&target).await?;
        Ok(b)
    }

    async fn send(&mut self, v: Value) -> Result<(), String> {
        self.ws.send(Message::Text(v.to_string().into())).await.map_err(|_| "The browser closed".to_string())
    }

    async fn raw(&mut self, method: &str, params: Value, in_tab: bool) -> Result<Value, String> {
        self.next += 1;
        let id = self.next;
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if in_tab {
            msg["sessionId"] = Value::String(self.session.clone());
        }
        self.send(msg).await?;
        let wait = async {
            loop {
                let m = self.ws.next().await.ok_or("The browser closed")?.map_err(|_| "The browser closed")?;
                let Message::Text(t) = m else { continue };
                let Ok(v) = serde_json::from_str::<Value>(&t) else { continue };
                // An alert() or confirm() would freeze the page: dismiss it.
                if v["method"] == "Page.javascriptDialogOpening" {
                    self.next += 1;
                    let mut close = json!({ "id": self.next, "method": "Page.handleJavaScriptDialog", "params": { "accept": false } });
                    close["sessionId"] = v["sessionId"].clone();
                    self.send(close).await?;
                    continue;
                }
                if v["id"].as_u64() == Some(id) {
                    return match v.get("error") {
                        Some(e) => Err(format!("Browser: {}", e["message"].as_str().unwrap_or("error"))),
                        None => Ok(v["result"].clone()),
                    };
                }
            }
        };
        tokio::time::timeout(CALL_TIMEOUT, wait).await.map_err(|_| "The browser did not answer".to_string())?
    }

    async fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.raw(method, params, true).await
    }

    async fn attach(&mut self, target: &str) -> Result<(), String> {
        let a = self.raw("Target.attachToTarget", json!({ "targetId": target, "flatten": true }), false).await?;
        self.session = a["sessionId"].as_str().ok_or("Could not use the tab")?.to_string();
        self.target = target.to_string();
        self.call("Page.enable", json!({})).await?;
        let ua = self.raw("Browser.getVersion", json!({}), false).await?;
        if let Some(ua) = ua["userAgent"].as_str() {
            // Many sites turn away anything that says "Headless".
            self.call("Emulation.setUserAgentOverride", json!({ "userAgent": ua.replace("HeadlessChrome", "Chrome") })).await?;
        }
        self.call("Emulation.setDeviceMetricsOverride", json!({ "width": 1280, "height": 900, "deviceScaleFactor": 1, "mobile": false })).await?;
        Ok(())
    }

    /// Run script in the page and get its value.
    pub async fn eval(&mut self, expression: &str) -> Result<Value, String> {
        let r = self.call("Runtime.evaluate", json!({ "expression": expression, "returnByValue": true, "awaitPromise": true })).await?;
        if r.get("exceptionDetails").is_some() {
            return Err("The page could not be read".into());
        }
        Ok(r["result"]["value"].clone())
    }

    /// Wait until the page has loaded (or has had a fair chance to).
    async fn settle(&mut self) {
        let start = Instant::now();
        tokio::time::sleep(Duration::from_millis(350)).await;
        while start.elapsed() < Duration::from_secs(15) {
            match self.eval("document.readyState").await {
                Ok(v) if v == "complete" => break,
                // Slow ads can keep a usable page "loading" for ever.
                Ok(v) if v == "interactive" && start.elapsed() > Duration::from_secs(6) => break,
                _ => tokio::time::sleep(Duration::from_millis(250)).await,
            }
        }
        // Give scripts that draw the page a moment.
        tokio::time::sleep(Duration::from_millis(600)).await;
    }

    /// A click can open a new tab: carry on in it and close the old one.
    async fn follow_new_tab(&mut self) -> Result<(), String> {
        let t = self.raw("Target.getTargets", json!({}), false).await?;
        let newer = t["targetInfos"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|i| i["type"] == "page" && i["openerId"].as_str() == Some(self.target.as_str()))
            .and_then(|i| i["targetId"].as_str().map(str::to_string));
        if let Some(n) = newer {
            let old = self.target.clone();
            self.attach(&n).await?;
            self.raw("Target.closeTarget", json!({ "targetId": old }), false).await?;
        }
        Ok(())
    }

    pub async fn open(&mut self, url: &str) -> Result<(), String> {
        let r = self.call("Page.navigate", json!({ "url": url })).await?;
        if let Some(e) = r["errorText"].as_str().filter(|e| !e.is_empty()) {
            return Err(if e.contains("TUNNEL") || e.contains("PROXY") { "The page could not be reached (it does not exist, or it is on a private network)".into() } else { format!("The page could not be loaded: {e}") });
        }
        self.settle().await;
        Ok(())
    }

    pub async fn back(&mut self) -> Result<(), String> {
        self.eval("history.back()").await?;
        self.settle().await;
        Ok(())
    }

    pub async fn scroll(&mut self, up: bool) -> Result<(), String> {
        self.eval(&format!("window.scrollBy(0, {} * Math.round(innerHeight * 0.85))", if up { -1 } else { 1 })).await?;
        tokio::time::sleep(Duration::from_millis(700)).await;
        Ok(())
    }

    /// The page as text, plus its links, buttons and fields with numbers.
    pub async fn look(&mut self) -> Result<Value, String> {
        let v = self.eval(SNAPSHOT_JS).await?;
        if !v["url"].as_str().is_some_and(|u| u.starts_with("http://") || u.starts_with("https://") || u == "about:blank" || u.starts_with("chrome-error:")) {
            self.call("Page.navigate", json!({ "url": "about:blank" })).await?;
            return Err("That page type cannot be opened".into());
        }
        Ok(v)
    }

    /// A JPEG of what is on screen, as a data: URL.
    pub async fn screenshot(&mut self) -> Result<String, String> {
        let r = self.call("Page.captureScreenshot", json!({ "format": "jpeg", "quality": 70 })).await?;
        Ok(format!("data:image/jpeg;base64,{}", r["data"].as_str().ok_or("No screenshot")?))
    }

    pub async fn click(&mut self, n: u64) -> Result<(), String> {
        let p = self
            .eval(&format!("(() => {{ const e = document.querySelector('[data-blz-ref=\"{n}\"]'); if (!e) return null; e.scrollIntoView({{block:'center', inline:'center', behavior:'instant'}}); const r = e.getBoundingClientRect(); return {{x: r.left + r.width / 2, y: r.top + r.height / 2}}; }})()"))
            .await?;
        let (Some(x), Some(y)) = (p["x"].as_f64(), p["y"].as_f64()) else { return Err(format!("There is no element [{n}] now. Use web_browser \"look\" to see the current numbers.")) };
        for kind in ["mouseMoved", "mousePressed", "mouseReleased"] {
            self.call("Input.dispatchMouseEvent", json!({ "type": kind, "x": x, "y": y, "button": "left", "clickCount": 1 })).await?;
        }
        self.settle().await;
        self.follow_new_tab().await?;
        Ok(())
    }

    /// Fill a text box or choose from a list.
    pub async fn fill(&mut self, n: u64, text: &str, submit: bool) -> Result<(), String> {
        let wanted = serde_json::to_string(text).map_err(|e| e.to_string())?;
        let r = self.eval(&format!("({FILL_JS})({n}, {wanted})")).await?;
        match r.as_str() {
            Some("text") => {
                if text.is_empty() {
                    self.eval("document.execCommand('delete')").await?;
                } else {
                    self.call("Input.insertText", json!({ "text": text })).await?;
                }
            }
            Some("chosen") => {}
            Some("password") => return Err("Blaze does not type into password or payment-card fields. Ask the user to do that step themselves.".into()),
            Some("no-option") => return Err("That list has no such option".into()),
            Some("not-a-field") => return Err(format!("[{n}] is not a text box or list")),
            _ => return Err(format!("There is no element [{n}] now. Use web_browser \"look\" to see the current numbers.")),
        }
        if submit {
            self.call("Input.dispatchKeyEvent", json!({ "type": "keyDown", "key": "Enter", "code": "Enter", "windowsVirtualKeyCode": 13, "text": "\r" })).await?;
            self.call("Input.dispatchKeyEvent", json!({ "type": "keyUp", "key": "Enter", "code": "Enter", "windowsVirtualKeyCode": 13 })).await?;
            self.settle().await;
            self.follow_new_tab().await?;
        } else {
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
        Ok(())
    }

    /// Ask the browser to quit; dropping it kills it if it does not.
    pub async fn close(mut self) {
        let _ = tokio::time::timeout(Duration::from_secs(3), async {
            let _ = self.raw("Browser.close", json!({}), false).await;
            let _ = self.child.wait().await;
        })
        .await;
    }
}

const SNAPSHOT_JS: &str = r#"(() => {
  document.querySelectorAll('[data-blz-ref]').forEach((e) => e.removeAttribute('data-blz-ref'));
  const clean = (s) => String(s || '').replace(/\s+/g, ' ').trim().slice(0, 90);
  const seen = (e) => { const r = e.getBoundingClientRect(); if (r.width < 2 || r.height < 2) return false; const s = getComputedStyle(e); return s.visibility !== 'hidden' && s.display !== 'none' && s.opacity !== '0'; };
  const all = [...document.querySelectorAll('a[href], button, input:not([type=hidden]), textarea, select, summary, [role=button], [role=link], [role=tab], [role=menuitem], [role=checkbox], [contenteditable=true]')].filter(seen);
  const near = (e) => { const r = e.getBoundingClientRect(); return r.bottom > -100 && r.top < innerHeight + 100; };
  const ordered = [...all.filter(near), ...all.filter((e) => !near(e))].slice(0, 120);
  const elements = ordered.map((e, i) => {
    e.setAttribute('data-blz-ref', String(i + 1));
    const tag = e.tagName.toLowerCase();
    const type = (e.getAttribute('type') || '').toLowerCase();
    let kind = tag === 'a' ? 'link' : tag === 'select' ? 'list' : tag === 'textarea' || (tag === 'input' && !['button','submit','checkbox','radio','image','reset'].includes(type)) ? 'text box' : type === 'checkbox' || type === 'radio' ? type : 'button';
    const label = clean(e.getAttribute('aria-label') || (e.labels && e.labels[0] && e.labels[0].innerText) || e.innerText || e.getAttribute('placeholder') || e.getAttribute('title') || e.getAttribute('alt') || (kind === 'button' ? e.value : '') || e.getAttribute('name'));
    let more = '';
    if (kind === 'link') more = ' -> ' + String(e.href).slice(0, 160);
    if (kind === 'text box' && type !== 'password' && e.value) more = ' = "' + clean(e.value) + '"';
    if (kind === 'list') more = ' options: ' + clean([...e.options].map((o) => o.text).join(' | '));
    if ((type === 'checkbox' || type === 'radio') && e.checked) more = ' (on)';
    return `[${i + 1}] ${kind} "${label}"${more}`;
  });
  const h = Math.max(document.documentElement.scrollHeight, 1);
  return {
    url: location.href, title: clean(document.title), elements, more: Math.max(0, all.length - ordered.length),
    scrolled: Math.min(100, Math.round(((scrollY + innerHeight) / h) * 100)),
    text: (document.body ? document.body.innerText : '').replace(/\n{3,}/g, '\n\n').slice(0, 12000),
  };
})()"#;

const FILL_JS: &str = r#"(n, wanted) => {
  const e = document.querySelector(`[data-blz-ref="${n}"]`);
  if (!e) return 'gone';
  const type = (e.getAttribute('type') || '').toLowerCase();
  const auto = (e.getAttribute('autocomplete') || '').toLowerCase();
  if (type === 'password' || auto.includes('password') || auto.startsWith('cc-')) return 'password';
  e.scrollIntoView({ block: 'center', behavior: 'instant' });
  if (e.tagName === 'SELECT') {
    const o = [...e.options].find((o) => o.text.trim().toLowerCase() === wanted.trim().toLowerCase() || o.value === wanted);
    if (!o) return 'no-option';
    e.value = o.value;
    e.dispatchEvent(new Event('input', { bubbles: true }));
    e.dispatchEvent(new Event('change', { bubbles: true }));
    return 'chosen';
  }
  if (!(e.tagName === 'TEXTAREA' || e.isContentEditable || (e.tagName === 'INPUT' && !['button','submit','checkbox','radio','file','image','reset'].includes(type)))) return 'not-a-field';
  e.focus();
  if (e.select) e.select(); else document.execCommand('selectAll');
  return 'text';
}"#;

struct Slot {
    browser: Arc<Mutex<Option<Browser>>>,
    used: Instant,
}

fn slots() -> &'static Mutex<HashMap<String, Slot>> {
    static S: OnceLock<Mutex<HashMap<String, Slot>>> = OnceLock::new();
    S.get_or_init(|| {
        // Close browsers nobody used for a while.
        tokio::spawn(async {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                let idle = {
                    let mut m = slots().lock().await;
                    let keys: Vec<String> = m.iter().filter(|(_, s)| s.used.elapsed() > IDLE).map(|(k, _)| k.clone()).collect();
                    keys.iter().filter_map(|k| m.remove(k)).collect::<Vec<_>>()
                };
                for s in idle {
                    shut(s).await;
                }
            }
        });
        Mutex::new(HashMap::new())
    })
}

async fn shut(s: Slot) {
    if let Some(b) = s.browser.lock().await.take() {
        b.close().await;
    }
}

/// The browser of one chat or bot (kept between its turns). The lock holds
/// `None` until it is started.
pub(crate) async fn slot(key: &str) -> Arc<Mutex<Option<Browser>>> {
    let (found, evicted) = {
        let mut m = slots().lock().await;
        let mut evicted = None;
        if !m.contains_key(key) && m.len() >= MAX_BROWSERS {
            let oldest = m.iter().max_by_key(|(_, s)| s.used.elapsed()).map(|(k, _)| k.clone());
            evicted = oldest.and_then(|k| m.remove(&k));
        }
        let s = m.entry(key.to_string()).or_insert_with(|| Slot { browser: Arc::new(Mutex::new(None)), used: Instant::now() });
        s.used = Instant::now();
        (s.browser.clone(), evicted)
    };
    if let Some(s) = evicted {
        shut(s).await;
    }
    found
}

/// Close every browser (when the app or command quits).
pub async fn close_all() {
    let all: Vec<Slot> = {
        let mut m = slots().lock().await;
        m.drain().map(|(_, s)| s).collect()
    };
    for s in all {
        shut(s).await;
    }
}
