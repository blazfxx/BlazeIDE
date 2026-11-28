//! Reading a page without a browser: download it, turn the HTML into plain
//! text with links, and hand it to the model one part at a time.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use scraper::{ElementRef, Html, Node, Selector};
use url::Url;

use super::guard::{Policy, Resolver};

const MAX_BODY: usize = 4_000_000;
pub const PART: usize = 18_000;
const MAX_REDIRECTS: usize = 6;
pub(crate) const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0 Safari/537.36";

pub(crate) fn client(policy: &Policy) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        // A system proxy would do the name lookup itself and skip the check.
        .no_proxy()
        .dns_resolver(Arc::new(Resolver(policy.clone())))
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(25))
        .build()
        .map_err(|e| e.to_string())
}

pub struct Downloaded {
    pub url: Url,
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
}

/// GET with every redirect checked.
pub async fn download(policy: &Policy, raw: &str) -> Result<Downloaded, String> {
    let http = client(policy)?;
    let mut url = policy.check(raw).await?;
    for _ in 0..=MAX_REDIRECTS {
        let res = http
            .get(url.clone())
            .header("accept", "text/html,application/xhtml+xml,text/plain,application/json;q=0.9,*/*;q=0.5")
            .header("accept-language", "en-US,en;q=0.9")
            .send()
            .await
            .map_err(|e| if e.is_timeout() { "The website took too long to answer".to_string() } else { format!("Could not load {url}: {}", root_cause(&e)) })?;
        let status = res.status();
        if status.is_redirection() {
            let next = res.headers().get("location").and_then(|v| v.to_str().ok()).ok_or("The website redirected nowhere")?;
            let next = url.join(next).map_err(|_| "The website redirected to a bad address")?;
            url = policy.check(next.as_str()).await?;
            continue;
        }
        let content_type = res.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_ascii_lowercase();
        let mut body = Vec::new();
        let mut stream = res.bytes_stream();
        while let Some(chunk) = stream.next().await {
            body.extend_from_slice(&chunk.map_err(|e| format!("The download broke off: {}", root_cause(&e)))?);
            if body.len() >= MAX_BODY {
                body.truncate(MAX_BODY);
                break;
            }
        }
        return Ok(Downloaded { url, status: status.as_u16(), content_type, body });
    }
    Err("Too many redirects".into())
}

fn root_cause(e: &dyn std::error::Error) -> String {
    let mut cur = e;
    while let Some(next) = cur.source() {
        cur = next;
    }
    cur.to_string()
}

const SKIP: &[&str] = &["script", "style", "noscript", "template", "svg", "canvas", "iframe", "form", "button", "select", "dialog"];
const BLOCK: &[&str] = &["p", "div", "section", "article", "main", "header", "footer", "aside", "nav", "ul", "ol", "table", "tr", "blockquote", "figure", "figcaption", "dl", "dt", "dd", "details", "summary", "br", "hr"];

fn push_text(out: &mut String, t: &str) {
    let mut space = t.starts_with(char::is_whitespace);
    for word in t.split_whitespace() {
        if space && !out.is_empty() && !out.ends_with(['\n', ' ', '[']) {
            out.push(' ');
        }
        out.push_str(word);
        space = true;
    }
    if t.ends_with(char::is_whitespace) && !out.is_empty() && !out.ends_with(['\n', ' ', '[']) {
        out.push(' ');
    }
}

fn newline(out: &mut String, blank: bool) {
    while out.ends_with(' ') {
        out.pop();
    }
    if out.is_empty() {
        return;
    }
    let want = if blank { "\n\n" } else { "\n" };
    while !out.ends_with(want) {
        out.push('\n');
    }
}

fn walk(el: ElementRef, base: &Url, out: &mut String, depth: usize) {
    if depth > 200 || out.len() > MAX_BODY / 4 {
        return;
    }
    for child in el.children() {
        match child.value() {
            Node::Text(t) => push_text(out, t),
            Node::Element(e) => {
                let Some(c) = ElementRef::wrap(child) else { continue };
                let name = e.name();
                if SKIP.contains(&name) || e.attr("hidden").is_some() || e.attr("aria-hidden") == Some("true") {
                    continue;
                }
                match name {
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                        newline(out, true);
                        out.push_str(&"#".repeat((name.as_bytes()[1] - b'0') as usize));
                        out.push(' ');
                        walk(c, base, out, depth + 1);
                        newline(out, true);
                    }
                    "li" => {
                        newline(out, false);
                        out.push_str("- ");
                        walk(c, base, out, depth + 1);
                    }
                    "pre" => {
                        newline(out, true);
                        out.push_str("```\n");
                        out.push_str(c.text().collect::<String>().trim_matches('\n'));
                        out.push_str("\n```");
                        newline(out, true);
                    }
                    "td" | "th" => {
                        walk(c, base, out, depth + 1);
                        out.push_str(" | ");
                    }
                    "a" => {
                        let href = e.attr("href").and_then(|h| base.join(h).ok()).filter(|u| matches!(u.scheme(), "http" | "https"));
                        let mut label = String::new();
                        walk(c, base, &mut label, depth + 1);
                        let label = label.split_whitespace().collect::<Vec<_>>().join(" ");
                        if label.is_empty() {
                            continue;
                        }
                        if !out.is_empty() && !out.ends_with(['\n', ' ']) && c.text().next().is_some_and(|t| t.starts_with(char::is_whitespace)) {
                            out.push(' ');
                        }
                        match href {
                            // Links within the page add nothing.
                            Some(u) if u.as_str().split('#').next() != base.as_str().split('#').next() && u.as_str().len() < 300 => out.push_str(&format!("[{label}]({u})")),
                            _ => out.push_str(&label),
                        }
                    }
                    "img" => {
                        if let Some(alt) = e.attr("alt").map(str::trim).filter(|a| !a.is_empty()) {
                            push_text(out, &format!(" (image: {alt}) "));
                        }
                    }
                    _ => {
                        let block = BLOCK.contains(&name);
                        if block {
                            newline(out, matches!(name, "p" | "section" | "article" | "table" | "blockquote"));
                        }
                        walk(c, base, out, depth + 1);
                        if block {
                            newline(out, matches!(name, "p" | "table" | "blockquote"));
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// (title, readable text) of an HTML page. When the page marks its main
/// content, only that is kept: menus and footers waste the model's reading.
pub fn html_to_text(html: &str, base: &Url) -> (String, String) {
    let doc = Html::parse_document(html);
    let sel = |s: &str| Selector::parse(s).expect("selector");
    let title = doc.select(&sel("title")).next().map(|t| t.text().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")).unwrap_or_default();
    let render = |el: ElementRef| {
        let mut s = String::new();
        walk(el, base, &mut s, 0);
        s.trim().to_string()
    };
    let main = ["main", "article", "[role=main]"].iter().find_map(|s| {
        let selector = sel(s);
        let mut found = doc.select(&selector);
        let first = found.next()?;
        // Several <article>s are a list: keep the whole page then.
        if found.next().is_some() {
            return None;
        }
        Some(render(first)).filter(|t| t.len() > 600)
    });
    let text = main.unwrap_or_else(|| doc.select(&sel("body")).next().map(render).unwrap_or_default());
    (title, text)
}

/// Part `n` (from 1) of a long text, cut at line ends: (text, parts).
pub fn part(text: &str, n: usize) -> (String, usize) {
    let mut parts: Vec<&str> = Vec::new();
    let mut rest = text;
    while rest.len() > PART {
        let mut cut = PART;
        while !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        if let Some(i) = rest[..cut].rfind('\n').filter(|i| *i > PART / 2) {
            cut = i + 1;
        }
        parts.push(&rest[..cut]);
        rest = &rest[cut..];
    }
    parts.push(rest);
    let total = parts.len();
    (parts.get(n.clamp(1, total) - 1).copied().unwrap_or("").to_string(), total)
}

/// The text of a page for the model, or why it cannot be read.
pub async fn read(policy: &Policy, raw: &str, n: usize) -> Result<(Url, String), String> {
    let d = download(policy, raw).await?;
    let ct = d.content_type.split(';').next().unwrap_or("").trim().to_string();
    let looks_html = ct.contains("html") || (ct.is_empty() && d.body.starts_with(b"<"));
    let textual = ct.is_empty() || ct.starts_with("text/") || ["json", "xml", "javascript", "yaml", "toml"].iter().any(|k| ct.contains(k));
    if !looks_html && !textual {
        return Err(format!("{} is not a text page (it is {ct}, {} KB)", d.url, d.body.len() / 1000));
    }
    let raw_text = String::from_utf8_lossy(&d.body).into_owned();
    let (title, text) = if looks_html { html_to_text(&raw_text, &d.url) } else { (String::new(), raw_text) };
    let (text, total) = part(&text, n);
    let mut out = String::new();
    if d.status >= 400 {
        out.push_str(&format!("The website answered with error {}.\n", d.status));
    }
    if !title.is_empty() {
        out.push_str(&format!("Title: {title}\n\n"));
    }
    if text.trim().is_empty() {
        out.push_str("(No readable text: this page probably builds itself with JavaScript. Open it with web_browser instead.)");
    } else {
        out.push_str(&text);
    }
    if total > 1 {
        out.push_str(&format!("\n\n(Part {} of {total}. Call web_fetch with \"part\" to read another.)", n.clamp(1, total)));
    }
    Ok((d.url, out))
}
