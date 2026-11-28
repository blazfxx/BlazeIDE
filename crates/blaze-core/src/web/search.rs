//! Web search without an API key. DuckDuckGo's plain-HTML page first; when
//! it asks for a bot check, the same page through the agent's browser; Bing
//! as the last resort.

use base64::Engine as _;
use scraper::{ElementRef, Html, Selector};
use url::Url;

use super::chrome::{self, Browser};
use super::fetch;
use super::guard::Policy;

#[derive(Debug, PartialEq)]
pub struct Hit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

fn text(e: ElementRef) -> String {
    e.text().collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

fn sel(s: &str) -> Selector {
    Selector::parse(s).expect("selector")
}

fn param(url: &Url, name: &str) -> Option<String> {
    url.query_pairs().find(|(k, _)| k == name).map(|(_, v)| v.into_owned())
}

/// Search result links go through the engine's own redirector: unwrap them.
fn real_link(href: &str, base: &str) -> Option<String> {
    let u = Url::parse(base).ok()?.join(href).ok()?;
    let host = u.host_str().unwrap_or("");
    if host.ends_with("duckduckgo.com") {
        // /l/?uddg=<target>; anything else on this host is an advert.
        return param(&u, "uddg").filter(|t| t.starts_with("http") && !t.contains("duckduckgo.com/y.js"));
    }
    if host.ends_with("bing.com") {
        let raw = param(&u, "u")?;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(raw.strip_prefix("a1")?.trim_end_matches('=')).ok()?;
        return String::from_utf8(bytes).ok().filter(|t| t.starts_with("http"));
    }
    matches!(u.scheme(), "http" | "https").then(|| u.to_string())
}

fn parse(html: &str, base: &str, result: &str, link: &str, snippet: &str) -> Vec<Hit> {
    let doc = Html::parse_document(html);
    let (result, link, snippet) = (sel(result), sel(link), sel(snippet));
    let mut out: Vec<Hit> = Vec::new();
    for r in doc.select(&result) {
        let Some(a) = r.select(&link).next() else { continue };
        let Some(url) = a.value().attr("href").and_then(|h| real_link(h, base)) else { continue };
        let title = text(a);
        if title.is_empty() || out.iter().any(|h| h.url == url) {
            continue;
        }
        out.push(Hit { title, url, snippet: r.select(&snippet).next().map(text).unwrap_or_default() });
        if out.len() == 8 {
            break;
        }
    }
    out
}

pub fn parse_ddg(html: &str) -> Vec<Hit> {
    parse(html, "https://html.duckduckgo.com/html/", ".result:not(.result--ad)", "a.result__a", ".result__snippet")
}

pub fn parse_bing(html: &str) -> Vec<Hit> {
    parse(html, "https://www.bing.com/search", "li.b_algo", "h2 a", ".b_caption p, p")
}

fn address(base: &str, query: &str) -> String {
    let mut u = Url::parse(base).expect("search address");
    u.query_pairs_mut().append_pair("q", query);
    u.to_string()
}

async fn over_http(policy: &Policy, url: &str) -> Option<String> {
    let d = fetch::download(policy, url).await.ok()?;
    (d.status == 200).then(|| String::from_utf8_lossy(&d.body).into_owned())
}

async fn in_browser(policy: &Policy, url: &str) -> Option<String> {
    chrome::find()?;
    let mut b = Browser::launch(policy.clone()).await.ok()?;
    let html = match b.open(url).await {
        Ok(()) => b.eval("document.documentElement.outerHTML").await.ok().and_then(|v| v.as_str().map(str::to_string)),
        Err(_) => None,
    };
    b.close().await;
    html
}

pub async fn search(policy: &Policy, query: &str) -> Result<Vec<Hit>, String> {
    let ddg = address("https://html.duckduckgo.com/html/", query);
    if let Some(h) = over_http(policy, &ddg).await.map(|h| parse_ddg(&h)).filter(|h| !h.is_empty()) {
        return Ok(h);
    }
    if let Some(h) = in_browser(policy, &ddg).await.map(|h| parse_ddg(&h)).filter(|h| !h.is_empty()) {
        return Ok(h);
    }
    let bing = address("https://www.bing.com/search?setlang=en&cc=US", query);
    if let Some(h) = over_http(policy, &bing).await.map(|h| parse_bing(&h)).filter(|h| !h.is_empty()) {
        return Ok(h);
    }
    Err("No search results came back (the search engines may be limiting this connection). Try again in a minute, or open a website you know with web_fetch.".into())
}
