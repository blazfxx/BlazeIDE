//! VS Code extension support, kept small on purpose.
//!
//! Blaze IDE's editor is Monaco (the same editor core as VS Code) but it has no
//! Node extension host, so an extension's *code* is never run. What it does
//! support is the declarative part, which is most of what people install:
//! colour themes and snippets. Extensions come from Open VSX (open-vsx.org), the open
//! registry; Microsoft's marketplace does not allow other apps to use it.
//!
//! Safety: the download must come from open-vsx.org over https and match the
//! SHA-256 the registry publishes; only the files the manifest declares are
//! extracted (never the whole archive), each capped in size, with every path
//! checked to stay inside the extension folder.

use std::fs;
use std::io::{Cursor, Read};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

const REGISTRY: &str = "https://open-vsx.org";
const MAX_VSIX: u64 = 60 * 1024 * 1024;
const MAX_FILE: u64 = 4 * 1024 * 1024;
const MAX_FILES: usize = 400;
/// Icon packs ship many small pictures (Material Icon Theme: ~1,300 SVGs).
const MAX_ICON_FILES: usize = 6000;
const MAX_ICON_FILE: u64 = 256 * 1024;
const MAX_ICON_BYTES: u64 = 40 * 1024 * 1024;
const MANIFEST: &str = "blaze-ext.json";
const CATEGORIES: &[&str] = &["Themes", "Snippets"];

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MarketItem {
    pub id: String,
    pub namespace: String,
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub version: String,
    pub downloads: u64,
    pub rating: Option<f64>,
    pub verified: bool,
    pub icon_url: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ThemeRef {
    pub label: String,
    /// "vs-dark", "vs" or "hc-black" (VS Code's `uiTheme`).
    pub base: String,
    pub path: String,
}

/// A file icon theme (`contributes.iconThemes`): only picture-based ones
/// (iconPath to .svg/.png); font-glyph icon themes are not supported.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct IconThemeRef {
    pub id: String,
    pub label: String,
    pub path: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FileRef {
    pub language: String,
    pub path: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Installed {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub version: String,
    pub themes: Vec<ThemeRef>,
    pub snippets: Vec<FileRef>,
    #[serde(default)]
    pub icon_themes: Vec<IconThemeRef>,
    /// Parts of the extension we cannot run (grammars, commands, its own code…).
    pub unsupported: Vec<String>,
    /// The extension's own icon as a data: URL (from its package; up to
    /// 256 KB). Older installs have none.
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(skip)]
    pub dir: PathBuf,
}

impl Installed {
    pub fn supported_parts(&self) -> usize {
        self.themes.len() + self.snippets.len() + self.icon_themes.len()
    }
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ThemeData {
    pub ext_id: String,
    pub label: String,
    pub base: String,
    pub json: Value,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct FileData {
    pub ext_id: String,
    pub language: String,
    pub json: Value,
}

#[derive(Serialize, Default, Debug)]
pub struct Contributions {
    pub themes: Vec<ThemeData>,
    pub snippets: Vec<FileData>,
}

// ---------------------------------------------------------------- JSONC

/// VS Code's JSON files allow `//` and `/* */` comments and trailing commas.
pub fn parse_jsonc(text: &str) -> Result<Value> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let out = rebuild_utf8(text);
    let cleaned = strip_trailing_commas(&out);
    Ok(serde_json::from_str(&cleaned)?)
}

/// Remove `//` and `/* */` comments that sit outside strings.
fn rebuild_utf8(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut it = text.chars().peekable();
    let (mut in_str, mut esc) = (false, false);
    while let Some(c) = it.next() {
        if in_str {
            out.push(c);
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match (c, it.peek()) {
            ('"', _) => {
                in_str = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for n in it.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                it.next();
                let mut prev = ' ';
                for n in it.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
            }
            _ => out.push(c),
        }
    }
    out
}

fn strip_trailing_commas(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let (mut in_str, mut esc) = (false, false);
    for (i, &c) in chars.iter().enumerate() {
        if in_str {
            out.push(c);
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        if c == '"' {
            in_str = true;
        } else if c == ',' {
            let next = chars[i + 1..].iter().find(|x| !x.is_whitespace());
            if matches!(next, Some('}') | Some(']')) {
                continue;
            }
        }
        out.push(c);
    }
    out
}

// ---------------------------------------------------------------- paths

fn valid_part(s: &str) -> bool {
    !s.is_empty() && s.len() <= 100 && s != "." && s != ".." && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// A path from a manifest, relative to the extension root, never leaving it.
fn clean_rel(raw: &str) -> Result<String> {
    let p = Path::new(raw.trim_start_matches("./"));
    let mut parts = Vec::new();
    for c in p.components() {
        match c {
            Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => return Err(Error::msg(format!("Unsafe path in extension: {raw}"))),
        }
    }
    if parts.is_empty() || raw.contains('\\') {
        return Err(Error::msg(format!("Unsafe path in extension: {raw}")));
    }
    Ok(parts.join("/"))
}

// ---------------------------------------------------------------- registry

/// One registry query for one category.
async fn search_one(http: &reqwest::Client, query: &str, category: &str, offset: u32) -> Result<(Vec<MarketItem>, u64)> {
    let q = [("query", query.trim().to_string()), ("size", "20".into()), ("offset", offset.min(10_000).to_string()), ("sortBy", "relevance".into()), ("category", category.to_string())];
    let resp = http.get(format!("{REGISTRY}/api/-/search")).query(&q).timeout(Duration::from_secs(15)).send().await.map_err(|e| Error::msg(format!("Could not reach the extension registry: {e}")))?;
    if !resp.status().is_success() {
        return Err(Error::msg(format!("The extension registry answered {}", resp.status().as_u16())));
    }
    let v: Value = resp.json().await.map_err(|e| Error::msg(format!("Unreadable registry answer: {e}")))?;
    Ok((parse_search(&v), v.get("totalSize").and_then(Value::as_u64).unwrap_or(0)))
}

/// Search the registry. With no category ("All") it merges Themes and
/// Snippets, the only kinds that do anything here, so the list never offers an
/// extension that cannot run.
pub async fn search(http: &reqwest::Client, query: &str, category: Option<&str>, offset: u32) -> Result<(Vec<MarketItem>, u64)> {
    if let Some(c) = category.filter(|c| CATEGORIES.contains(c)) {
        return search_one(http, query, c, offset).await;
    }
    let half = offset / 2;
    let (a, b) = tokio::join!(search_one(http, query, "Themes", half), search_one(http, query, "Snippets", half));
    let (mut items, mut total) = (Vec::new(), 0);
    let mut last = None;
    for r in [a, b] {
        match r {
            Ok((i, t)) => {
                items.extend(i);
                total += t;
            }
            Err(e) => last = Some(e),
        }
    }
    if items.is_empty() {
        if let Some(e) = last {
            return Err(e);
        }
    }
    items.sort_by_key(|x| std::cmp::Reverse(x.downloads));
    items.dedup_by(|x, y| x.id == y.id);
    Ok((items, total))
}

pub fn parse_search(v: &Value) -> Vec<MarketItem> {
    let s = |x: &Value, k: &str| x.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    v.get("extensions")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|x| x.get("deprecated").and_then(Value::as_bool) != Some(true))
                .filter_map(|x| {
                    let (ns, name) = (s(x, "namespace"), s(x, "name"));
                    if !valid_part(&ns) || !valid_part(&name) {
                        return None;
                    }
                    let display = s(x, "displayName");
                    Some(MarketItem {
                        id: format!("{ns}.{name}"),
                        display_name: if display.is_empty() { name.clone() } else { display },
                        description: s(x, "description"),
                        version: s(x, "version"),
                        downloads: x.get("downloadCount").and_then(Value::as_u64).unwrap_or(0),
                        rating: x.get("averageRating").and_then(Value::as_f64),
                        verified: x.get("verified").and_then(Value::as_bool).unwrap_or(false),
                        icon_url: x.pointer("/files/icon").and_then(Value::as_str).filter(|u| registry_url(u).is_ok()).map(str::to_string),
                        namespace: ns,
                        name,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn registry_url(u: &str) -> Result<&str> {
    if u.starts_with("https://open-vsx.org/api/") && !u.contains("..") {
        Ok(u)
    } else {
        Err(Error::msg("The registry pointed at a file outside open-vsx.org"))
    }
}

async fn download(http: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let resp = http.get(registry_url(url)?).timeout(Duration::from_secs(120)).send().await.map_err(|e| Error::msg(format!("Download failed: {e}")))?;
    if !resp.status().is_success() {
        return Err(Error::msg(format!("Download failed ({})", resp.status().as_u16())));
    }
    if resp.content_length().map(|n| n > MAX_VSIX).unwrap_or(false) {
        return Err(Error::msg("That extension is too large (over 60 MB)"));
    }
    let mut buf = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        buf.extend_from_slice(&chunk.map_err(|e| Error::msg(format!("Download failed: {e}")))?);
        if buf.len() as u64 > MAX_VSIX {
            return Err(Error::msg("That extension is too large (over 60 MB)"));
        }
    }
    Ok(buf)
}

pub async fn install(http: &reqwest::Client, root: &Path, namespace: &str, name: &str) -> Result<Installed> {
    if !valid_part(namespace) || !valid_part(name) {
        return Err(Error::msg("Invalid extension id"));
    }
    let meta: Value = http
        .get(format!("{REGISTRY}/api/{namespace}/{name}"))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| Error::msg(format!("Could not reach the extension registry: {e}")))?
        .json()
        .await
        .map_err(|_| Error::msg("Extension not found"))?;
    let files = meta.get("files").ok_or_else(|| Error::msg("Extension not found"))?;
    let vsix_url = files.get("download").and_then(Value::as_str).ok_or_else(|| Error::msg("That extension has no download"))?;
    let sha_url = files.get("sha256").and_then(Value::as_str).ok_or_else(|| Error::msg("The registry gave no checksum, so the download was refused"))?;
    let want = String::from_utf8(download(http, sha_url).await?).unwrap_or_default();
    let want = want.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
    let bytes = download(http, vsix_url).await?;
    let got: String = Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect();
    if want.len() != 64 || want != got {
        return Err(Error::msg("The download did not match its checksum, so it was discarded"));
    }
    install_vsix(&bytes, root, &format!("{namespace}.{name}"))
}

// ---------------------------------------------------------------- details page

const MAX_README: usize = 400 * 1024;
const MAX_IMAGES: usize = 16;
const MAX_IMAGE: usize = 2 * 1024 * 1024;

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Publisher {
    /// The namespace's display name (what the marketplace shows as publisher).
    pub name: String,
    pub namespace: String,
    pub login: String,
    pub full_name: String,
    /// Avatar as a data: URL (fetched here; the page may only load images
    /// from open-vsx.org).
    pub avatar: Option<String>,
    pub homepage: String,
    pub verified: bool,
    /// Number of extensions in this namespace.
    pub extensions: usize,
}

#[derive(Serialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Details {
    pub id: String,
    pub namespace: String,
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub version: String,
    pub downloads: u64,
    pub rating: Option<f64>,
    pub review_count: u64,
    pub verified: bool,
    pub icon_url: Option<String>,
    pub homepage: String,
    pub repository: String,
    pub bugs: String,
    pub license: String,
    pub categories: Vec<String>,
    pub tags: Vec<String>,
    /// ISO time of the latest version.
    pub updated: String,
    pub size: u64,
    pub versions: usize,
    pub gallery_color: String,
    pub publisher: Publisher,
    pub readme: String,
    pub changelog: String,
    /// Image URL (as written in the readme/changelog) -> data: URL.
    pub images: std::collections::BTreeMap<String, String>,
    /// What the extension contributes, from its package.json.
    pub themes: Vec<String>,
    pub snippets: Vec<String>,
    pub icon_themes: Vec<String>,
    /// Contribution kinds Blaze IDE cannot run (commands, languages, ...).
    pub unsupported: Vec<String>,
}

async fn get_text(http: &reqwest::Client, url: &str, max: usize) -> Option<String> {
    let r = http.get(registry_url(url).ok()?).timeout(Duration::from_secs(15)).send().await.ok()?;
    if !r.status().is_success() {
        return None;
    }
    let b = r.bytes().await.ok()?;
    let mut s = String::from_utf8_lossy(&b[..b.len().min(max)]).into_owned();
    if b.len() > max {
        s.push_str("\n\n…");
    }
    Some(s)
}

/// Fetch one https image for the details page (any host: a README's pictures
/// live on GitHub, shields.io, ...), returned as a data: URL.
async fn get_image(http: &reqwest::Client, url: &str) -> Option<String> {
    use base64::Engine;
    let u = reqwest::Url::parse(url).ok()?;
    if u.scheme() != "https" || u.host_str().is_none() {
        return None;
    }
    let r = http.get(u).timeout(Duration::from_secs(12)).send().await.ok()?;
    if !r.status().is_success() || r.content_length().map(|n| n as usize > MAX_IMAGE).unwrap_or(false) {
        return None;
    }
    let ct = r.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    let mime = match ct.as_str() {
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/svg+xml" => ct,
        _ => {
            // GitHub raw serves images as octet-stream / text/plain: guess from the name.
            let path = url.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
            let ext = path.rsplit('.').next().unwrap_or("");
            match ext {
                "png" => "image/png".into(),
                "jpg" | "jpeg" => "image/jpeg".into(),
                "gif" => "image/gif".into(),
                "webp" => "image/webp".into(),
                "svg" => "image/svg+xml".into(),
                _ => return None,
            }
        }
    };
    let b = r.bytes().await.ok()?;
    (b.len() <= MAX_IMAGE).then(|| format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(&b)))
}

/// `https://github.com/o/r(.git)` -> `https://raw.githubusercontent.com/o/r/HEAD/`.
fn raw_base(repo: &str) -> Option<String> {
    let r = repo.trim().trim_start_matches("git+").trim_end_matches('/').trim_end_matches(".git");
    let rest = r.strip_prefix("https://github.com/").or_else(|| r.strip_prefix("http://github.com/"))?;
    let mut it = rest.split('/');
    let (o, n) = (it.next()?, it.next()?);
    (!o.is_empty() && !n.is_empty()).then(|| format!("https://raw.githubusercontent.com/{o}/{n}/HEAD/"))
}

/// Image references in markdown (`![a](url "t")`) and inline HTML (`<img src="url">`).
pub fn image_refs(md: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = md;
    while let Some(i) = rest.find("](") {
        let before = &rest[..i];
        let is_img = before.rfind("![").map(|j| !before[j..].contains("\n\n")).unwrap_or(false);
        let after = &rest[i + 2..];
        let end = after.find(')').unwrap_or(after.len());
        if is_img {
            let target = after[..end].trim().trim_start_matches('<');
            let url = target.split_whitespace().next().unwrap_or("").trim_end_matches('>');
            if !url.is_empty() {
                out.push(url.to_string());
            }
        }
        rest = &after[end.min(after.len())..];
    }
    let lower = md.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find("<img") {
        let start = from + i;
        let tag_end = lower[start..].find('>').map(|e| start + e).unwrap_or(lower.len());
        let tag = &md[start..tag_end];
        let tl = &lower[start..tag_end];
        if let Some(p) = tl.find("src=") {
            let v = &tag[p + 4..];
            let q = v.chars().next().unwrap_or(' ');
            let url = if q == '"' || q == '\'' { v[1..].split(q).next().unwrap_or("") } else { v.split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("") };
            if !url.is_empty() {
                out.push(url.to_string());
            }
        }
        from = tag_end.max(start + 4);
    }
    out.dedup();
    out
}

fn resolve_image(raw: &str, base: Option<&str>) -> Option<String> {
    if raw.starts_with("https://") {
        // github.com/o/r/blob/x -> raw (the blob page is HTML).
        if let Some(rest) = raw.strip_prefix("https://github.com/") {
            let parts: Vec<&str> = rest.splitn(4, '/').collect();
            if parts.len() == 4 && (parts[2] == "blob" || parts[2] == "raw") {
                return Some(format!("https://raw.githubusercontent.com/{}/{}/{}", parts[0], parts[1], parts[3]));
            }
        }
        return Some(raw.to_string());
    }
    if raw.starts_with("http:") || raw.starts_with("data:") || raw.starts_with("//") || raw.contains("..") {
        return None;
    }
    base.map(|b| format!("{b}{}", raw.trim_start_matches("./").trim_start_matches('/')))
}

fn contributes_summary(manifest: &Value) -> (Vec<String>, Vec<String>, Vec<String>, Vec<String>) {
    let c = manifest.get("contributes");
    let list = |k: &str, f: &str| -> Vec<String> {
        c.and_then(|c| c.get(k)).and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.get(f).and_then(Value::as_str).map(str::to_string)).collect()).unwrap_or_default()
    };
    let themes = list("themes", "label");
    let mut snippets = list("snippets", "language");
    snippets.dedup();
    let icon_themes = list("iconThemes", "label").into_iter().filter(|l| !l.starts_with('%')).collect::<Vec<_>>();
    let icon_themes = if icon_themes.is_empty() { list("iconThemes", "id") } else { icon_themes };
    let mut other: Vec<String> = c
        .and_then(Value::as_object)
        .map(|o| o.keys().filter(|k| *k != "themes" && *k != "snippets" && *k != "iconThemes").map(|k| k.to_string()).collect())
        .unwrap_or_default();
    if manifest.get("main").is_some() || manifest.get("browser").is_some() {
        other.insert(0, "its own code".into());
    }
    (themes, snippets, icon_themes, other)
}

/// Everything the extension details page shows: registry facts, publisher,
/// README and changelog (with their pictures inlined) and what it contributes.
pub async fn details(http: &reqwest::Client, namespace: &str, name: &str) -> Result<Details> {
    if !valid_part(namespace) || !valid_part(name) {
        return Err(Error::msg("Invalid extension id"));
    }
    let resp = http
        .get(format!("{REGISTRY}/api/{namespace}/{name}"))
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| Error::msg(format!("Could not reach the extension registry: {e}")))?;
    if !resp.status().is_success() {
        return Err(Error::msg(if resp.status().as_u16() == 404 { "Extension not found".to_string() } else { format!("The extension registry answered {}", resp.status().as_u16()) }));
    }
    let m: Value = resp.json().await.map_err(|_| Error::msg("Unreadable registry answer"))?;
    let s = |k: &str| m.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let strs = |k: &str| -> Vec<String> { m.get(k).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).filter(|t| !t.starts_with("__")).map(str::to_string).collect()).unwrap_or_default() };
    let file = |k: &str| m.pointer(&format!("/files/{k}")).and_then(Value::as_str).map(str::to_string);
    let display = s("displayName");
    let pubd = m.get("publishedBy");
    let ps = |k: &str| pubd.and_then(|p| p.get(k)).and_then(Value::as_str).unwrap_or("").to_string();

    let readme_url = file("readme");
    let changelog_url = file("changelog");
    let manifest_url = file("manifest");
    let ns_url = format!("{REGISTRY}/api/{namespace}");
    let avatar_url = ps("avatarUrl");
    let (readme, changelog, manifest, ns, avatar) = tokio::join!(
        async { match &readme_url { Some(u) => get_text(http, u, MAX_README).await.unwrap_or_default(), None => String::new() } },
        async { match &changelog_url { Some(u) => get_text(http, u, MAX_README).await.unwrap_or_default(), None => String::new() } },
        async { match &manifest_url { Some(u) => get_text(http, u, 1024 * 1024).await.and_then(|t| serde_json::from_str::<Value>(&t).ok()), None => None } },
        async { get_text(http, &ns_url, 256 * 1024).await.and_then(|t| serde_json::from_str::<Value>(&t).ok()) },
        async { if avatar_url.is_empty() { None } else { get_image(http, &avatar_url).await } },
    );

    let repo = s("repository");
    let base = raw_base(&repo);
    let mut wanted: Vec<(String, String)> = Vec::new();
    for r in image_refs(&readme).into_iter().chain(image_refs(&changelog)) {
        if wanted.len() >= MAX_IMAGES {
            break;
        }
        if wanted.iter().any(|(k, _)| *k == r) {
            continue;
        }
        if let Some(u) = resolve_image(&r, base.as_deref()) {
            wanted.push((r, u));
        }
    }
    let fetched = futures_util::future::join_all(wanted.iter().map(|(_, u)| get_image(http, u))).await;
    let images = wanted.into_iter().zip(fetched).filter_map(|((k, _), v)| v.map(|v| (k, v))).collect();

    let (themes, snippets, icon_themes, unsupported) = manifest.as_ref().map(contributes_summary).unwrap_or_default();
    let ns_display = s("namespaceDisplayName");
    Ok(Details {
        id: format!("{namespace}.{name}"),
        namespace: namespace.to_string(),
        name: name.to_string(),
        display_name: if display.is_empty() { name.to_string() } else { display },
        description: s("description"),
        version: s("version"),
        downloads: m.get("downloadCount").and_then(Value::as_u64).unwrap_or(0),
        rating: m.get("averageRating").and_then(Value::as_f64),
        review_count: m.get("reviewCount").and_then(Value::as_u64).unwrap_or(0),
        verified: m.get("verified").and_then(Value::as_bool).unwrap_or(false),
        icon_url: file("icon").filter(|u| registry_url(u).is_ok()),
        homepage: s("homepage"),
        repository: repo.trim_start_matches("git+").trim_end_matches(".git").to_string(),
        bugs: s("bugs"),
        license: s("license"),
        categories: strs("categories"),
        tags: strs("tags"),
        updated: s("timestamp"),
        size: m.pointer("/downloadSizes/universal").and_then(Value::as_u64).or_else(|| m.get("downloadSizes").and_then(Value::as_object).and_then(|o| o.values().next()).and_then(Value::as_u64)).unwrap_or(0),
        versions: m.get("allVersions").and_then(Value::as_object).map(|o| o.keys().filter(|k| *k != "latest" && *k != "pre-release").count()).unwrap_or(0),
        gallery_color: s("galleryColor"),
        publisher: Publisher {
            name: if ns_display.is_empty() { namespace.to_string() } else { ns_display },
            namespace: namespace.to_string(),
            login: ps("loginName"),
            full_name: ps("fullName"),
            avatar,
            homepage: ps("homepage"),
            verified: m.get("verified").and_then(Value::as_bool).unwrap_or(false),
            extensions: ns.as_ref().and_then(|n| n.get("extensions")).and_then(Value::as_object).map(|o| o.len()).unwrap_or(0),
        },
        readme,
        changelog,
        images,
        themes,
        snippets,
        icon_themes,
        unsupported,
    })
}

// ---------------------------------------------------------------- vsix

fn read_entry<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>, name: &str) -> Result<Vec<u8>> {
    let mut f = zip.by_name(name).map_err(|_| Error::msg(format!("Missing {name} in the extension")))?;
    if f.size() > MAX_FILE {
        return Err(Error::msg(format!("{name} is too large")));
    }
    let mut buf = Vec::new();
    f.by_ref().take(MAX_FILE + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > MAX_FILE {
        return Err(Error::msg(format!("{name} is too large")));
    }
    Ok(buf)
}

/// Pure part of `install`: unpack the supported pieces of a .vsix into
/// `root/<id>-<version>/` and write our own manifest next to them.
pub fn install_vsix(bytes: &[u8], root: &Path, id: &str) -> Result<Installed> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| Error::msg("That file is not a valid extension package"))?;
    if zip.len() > 20_000 {
        return Err(Error::msg("That extension has too many files"));
    }
    let pkg = parse_jsonc(&String::from_utf8_lossy(&read_entry(&mut zip, "extension/package.json")?))?;
    let version = pkg.get("version").and_then(Value::as_str).unwrap_or("0.0.0").to_string();
    if !valid_part(&version) {
        return Err(Error::msg("Unsupported version string"));
    }
    let contributes = pkg.get("contributes").cloned().unwrap_or(Value::Null);
    let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();

    let mut want: Vec<String> = Vec::new();
    let mut themes = Vec::new();
    for t in contributes.get("themes").and_then(Value::as_array).into_iter().flatten() {
        let path = clean_rel(&text(t, "path"))?;
        let label = if text(t, "label").is_empty() { path.clone() } else { text(t, "label") };
        let base = match text(t, "uiTheme").as_str() {
            "vs" => "vs",
            "hc-black" => "hc-black",
            _ => "vs-dark",
        };
        themes.push(ThemeRef { label, base: base.into(), path: path.clone() });
        want.push(path);
    }
    let mut snippets = Vec::new();
    for s in contributes.get("snippets").and_then(Value::as_array).into_iter().flatten() {
        let (language, path) = (text(s, "language"), clean_rel(&text(s, "path"))?);
        if !language.is_empty() {
            snippets.push(FileRef { language, path: path.clone() });
            want.push(path);
        }
    }

    // Icon themes: the theme JSON plus every picture its iconDefinitions use.
    let mut icon_themes = Vec::new();
    let mut icon_files: Vec<String> = Vec::new();
    let mut font_icons = false;
    for it in contributes.get("iconThemes").and_then(Value::as_array).into_iter().flatten() {
        let path = clean_rel(&text(it, "path"))?;
        let Ok(raw) = read_entry(&mut zip, &format!("extension/{path}")) else { continue };
        let Ok(json) = parse_jsonc(&String::from_utf8_lossy(&raw)) else { continue };
        let parent = Path::new(&path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let mut pictures = 0;
        for def in json.get("iconDefinitions").and_then(Value::as_object).into_iter().flat_map(|o| o.values()) {
            let Some(ip) = def.get("iconPath").and_then(Value::as_str) else {
                if def.get("fontCharacter").is_some() {
                    font_icons = true;
                }
                continue;
            };
            let joined = if parent.is_empty() { ip.to_string() } else { format!("{parent}/{ip}") };
            let Ok(rel) = clean_rel(&normalize(&joined)) else { continue };
            let lower = rel.to_ascii_lowercase();
            if lower.ends_with(".svg") || lower.ends_with(".png") {
                icon_files.push(rel);
                pictures += 1;
            }
        }
        if pictures > 0 {
            let id_ = if text(it, "id").is_empty() { path.clone() } else { text(it, "id") };
            let label = if text(it, "label").is_empty() || text(it, "label").starts_with('%') { id_.clone() } else { text(it, "label") };
            icon_themes.push(IconThemeRef { id: id_, label, path: path.clone() });
            want.push(path);
        }
    }
    icon_files.sort();
    icon_files.dedup();
    if icon_files.len() > MAX_ICON_FILES {
        return Err(Error::msg("That icon theme has too many pictures"));
    }

    let dir = root.join(format!("{id}-{version}"));
    let tmp = root.join(format!(".{id}-{version}.tmp"));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp)?;
    let result = (|| -> Result<()> {
        let mut seen = std::collections::HashSet::new();
        let mut queue = want.clone();
        let mut n = 0;
        while let Some(rel) = queue.pop() {
            if !seen.insert(rel.clone()) {
                continue;
            }
            n += 1;
            if n > MAX_FILES {
                return Err(Error::msg("That extension declares too many files"));
            }
            let data = read_entry(&mut zip, &format!("extension/{rel}"))?;
            // A theme can `include` another theme file; bring those along.
            if themes.iter().any(|t| t.path == rel) {
                if let Ok(v) = parse_jsonc(&String::from_utf8_lossy(&data)) {
                    if let Some(inc) = v.get("include").and_then(Value::as_str) {
                        let parent = Path::new(&rel).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
                        let joined = if parent.is_empty() { inc.to_string() } else { format!("{parent}/{inc}") };
                        queue.push(clean_rel(&normalize(&joined))?);
                    }
                }
            }
            let target = tmp.join(&rel);
            if let Some(p) = target.parent() {
                fs::create_dir_all(p)?;
            }
            fs::write(target, data)?;
        }
        // Icon pictures: many small files, each capped; a missing one is skipped.
        let mut bytes = 0u64;
        for rel in &icon_files {
            let Ok(mut f) = zip.by_name(&format!("extension/{rel}")) else { continue };
            if f.size() > MAX_ICON_FILE {
                continue;
            }
            bytes += f.size();
            if bytes > MAX_ICON_BYTES {
                return Err(Error::msg("That icon theme is too large"));
            }
            let mut buf = Vec::new();
            f.by_ref().take(MAX_ICON_FILE + 1).read_to_end(&mut buf)?;
            let target = tmp.join(rel);
            if let Some(p) = target.parent() {
                fs::create_dir_all(p)?;
            }
            fs::write(target, buf)?;
        }
        Ok(())
    })();
    if let Err(e) = result {
        let _ = fs::remove_dir_all(&tmp);
        return Err(e);
    }

    let mut unsupported = Vec::new();
    if pkg.get("main").is_some() || pkg.get("browser").is_some() {
        unsupported.push("its own code (needs the VS Code extension host)".to_string());
    }
    for (key, label) in [("grammars", "syntax grammars"), ("languages", "new languages"), ("commands", "commands"), ("debuggers", "debuggers"), ("productIconThemes", "product icon themes"), ("keybindings", "keybindings"), ("configuration", "settings")] {
        if contributes.get(key).map(|v| !v.is_null()).unwrap_or(false) {
            unsupported.push(label.to_string());
        }
    }
    if font_icons && icon_themes.is_empty() {
        unsupported.push("font-based icon themes".to_string());
    }
    let icon = icon_data_url(&mut zip, &text(&pkg, "icon"));
    let display = text(&pkg, "displayName");
    let info = Installed {
        id: id.to_string(),
        display_name: if display.is_empty() || display.starts_with('%') { id.to_string() } else { display },
        description: if text(&pkg, "description").starts_with('%') { String::new() } else { text(&pkg, "description") },
        version,
        themes,
        snippets,
        icon_themes,
        unsupported,
        icon,
        dir: dir.clone(),
    };
    fs::write(tmp.join(MANIFEST), serde_json::to_vec_pretty(&info)?)?;
    // Replace any previous version of this extension.
    for old in list_installed(root).into_iter().filter(|e| e.id == id) {
        let _ = fs::remove_dir_all(&old.dir);
    }
    let _ = fs::remove_dir_all(&dir);
    fs::rename(&tmp, &dir)?;
    Ok(info)
}

/// The package's declared icon (png/jpg/svg/webp, up to 256 KB) as a data URL.
fn icon_data_url<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>, rel: &str) -> Option<String> {
    use base64::Engine;
    let rel = clean_rel(rel).ok()?;
    let mime = match rel.rsplit('.').next()?.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        _ => return None,
    };
    let mut f = zip.by_name(&format!("extension/{rel}")).ok()?;
    if f.size() > 256 * 1024 {
        return None;
    }
    let mut buf = Vec::new();
    f.by_ref().take(256 * 1024 + 1).read_to_end(&mut buf).ok()?;
    (buf.len() <= 256 * 1024).then(|| format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(&buf)))
}

/// Resolve `a/b/../c` without touching the disk.
fn normalize(p: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    out.join("/")
}

// ---------------------------------------------------------------- installed

pub fn list_installed(root: &Path) -> Vec<Installed> {
    let mut out: Vec<Installed> = fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| {
            let mut info: Installed = serde_json::from_slice(&fs::read(e.path().join(MANIFEST)).ok()?).ok()?;
            info.dir = e.path();
            Some(info)
        })
        .collect();
    out.sort_by_key(|a| a.display_name.to_lowercase());
    out
}

pub fn uninstall(root: &Path, id: &str) -> Result<()> {
    let mut removed = false;
    for e in list_installed(root).into_iter().filter(|e| e.id == id) {
        fs::remove_dir_all(&e.dir)?;
        removed = true;
    }
    if removed { Ok(()) } else { Err(Error::msg("That extension is not installed")) }
}

fn read_json(dir: &Path, rel: &str) -> Option<Value> {
    let rel = clean_rel(rel).ok()?;
    let bytes = fs::read(dir.join(rel)).ok()?;
    if bytes.len() as u64 > MAX_FILE {
        return None;
    }
    parse_jsonc(&String::from_utf8_lossy(&bytes)).ok()
}

/// Merge a theme with the file it `include`s (one level is all real themes use).
fn load_theme(dir: &Path, rel: &str, depth: u8) -> Option<Value> {
    let mut v = read_json(dir, rel)?;
    if depth < 3 {
        if let Some(inc) = v.get("include").and_then(Value::as_str).map(str::to_owned) {
            let parent = Path::new(rel).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            let target = normalize(&if parent.is_empty() { inc } else { format!("{parent}/{inc}") });
            if let Some(base) = load_theme(dir, &target, depth + 1) {
                let mut merged = base;
                if let (Some(m), Some(own)) = (merged.as_object_mut(), v.as_object()) {
                    for key in ["colors", "tokenColors"] {
                        if let Some(extra) = own.get(key) {
                            match (m.get_mut(key), extra) {
                                (Some(Value::Object(a)), Value::Object(b)) => a.extend(b.clone()),
                                (Some(Value::Array(a)), Value::Array(b)) => a.extend(b.clone()),
                                _ => {
                                    m.insert(key.into(), extra.clone());
                                }
                            }
                        }
                    }
                }
                v = merged;
            }
        }
    }
    Some(v)
}

pub fn contributions(root: &Path) -> Contributions {
    let mut c = Contributions::default();
    for e in list_installed(root) {
        for t in &e.themes {
            if let Some(json) = load_theme(&e.dir, &t.path, 0) {
                c.themes.push(ThemeData { ext_id: e.id.clone(), label: t.label.clone(), base: t.base.clone(), json });
            }
        }
        for s in &e.snippets {
            if let Some(json) = read_json(&e.dir, &s.path) {
                c.snippets.push(FileData { ext_id: e.id.clone(), language: s.language.clone(), json });
            }
        }
    }
    c
}

/// A file icon theme ready for the file tree: its mappings (fileNames,
/// fileExtensions, folderNames, languageIds, light, ...) and each icon
/// definition as a data: URL.
#[derive(Serialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct IconThemeData {
    pub ext_id: String,
    pub id: String,
    pub label: String,
    pub map: Value,
    pub icons: std::collections::BTreeMap<String, String>,
}

pub fn icon_theme(root: &Path, ext_id: &str, theme_id: &str) -> Result<IconThemeData> {
    use base64::Engine;
    let e = list_installed(root).into_iter().find(|e| e.id == ext_id).ok_or_else(|| Error::msg("That extension is not installed"))?;
    let t = e.icon_themes.iter().find(|t| t.id == theme_id).ok_or_else(|| Error::msg("That icon theme is not in the extension"))?;
    let mut map = read_json(&e.dir, &t.path).ok_or_else(|| Error::msg("The icon theme file is missing"))?;
    let parent = Path::new(&t.path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let mut icons = std::collections::BTreeMap::new();
    if let Some(defs) = map.get("iconDefinitions").and_then(Value::as_object) {
        for (id, def) in defs {
            let Some(ip) = def.get("iconPath").and_then(Value::as_str) else { continue };
            let joined = if parent.is_empty() { ip.to_string() } else { format!("{parent}/{ip}") };
            let Ok(rel) = clean_rel(&normalize(&joined)) else { continue };
            let Ok(data) = fs::read(e.dir.join(&rel)) else { continue };
            let url = if rel.to_ascii_lowercase().ends_with(".svg") {
                format!("data:image/svg+xml;base64,{}", base64::engine::general_purpose::STANDARD.encode(&data))
            } else {
                format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&data))
            };
            icons.insert(id.clone(), url);
        }
    }
    if let Some(o) = map.as_object_mut() {
        o.remove("iconDefinitions");
        o.remove("fonts");
    }
    Ok(IconThemeData { ext_id: e.id.clone(), id: t.id.clone(), label: t.label.clone(), map, icons })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn vsix(files: &[(&str, &str)]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default();
        for (name, body) in files {
            w.start_file(*name, opts).unwrap();
            w.write_all(body.as_bytes()).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    const PKG: &str = r#"{
      // comments are legal in package.json files from VS Code
      "name": "demo", "displayName": "Demo Pack", "version": "1.2.3", "main": "./out/ext.js",
      "contributes": {
        "themes": [{ "label": "Demo Dark", "uiTheme": "vs-dark", "path": "./themes/dark.json" }],
        "snippets": [{ "language": "javascript", "path": "./snippets/js.json" }],
        "grammars": [{ "language": "x", "scopeName": "s", "path": "./g.json" }],
      }
    }"#;

    #[test]
    fn jsonc_comments_and_trailing_commas() {
        let v = parse_jsonc("{\n // c\n \"a\": \"x // not a comment é\", /* b */ \"b\": [1,2,],\n}").unwrap();
        assert_eq!(v["a"], "x // not a comment é");
        assert_eq!(v["b"][1], 2);
        assert!(parse_jsonc("\u{feff}{\"k\":1}").is_ok());
    }

    #[test]
    fn installs_only_declared_files() {
        let bytes = vsix(&[
            ("extension/package.json", PKG),
            ("extension/themes/dark.json", r##"{"include":"./base.json","colors":{"editor.background":"#101010"},"tokenColors":[{"scope":"comment","settings":{"foreground":"#888"}}]}"##),
            ("extension/themes/base.json", r##"{"colors":{"editor.foreground":"#eee"},"tokenColors":[]}"##),
            ("extension/snippets/js.json", r#"{"log":{"prefix":"cl","body":["console.log($1)"]}}"#),
            ("extension/out/ext.js", "console.log('never extracted')"),
        ]);
        let root = tempfile::tempdir().unwrap();
        let info = install_vsix(&bytes, root.path(), "me.demo").unwrap();
        assert_eq!(info.display_name, "Demo Pack");
        assert_eq!((info.themes.len(), info.snippets.len()), (1, 1));
        assert!(info.unsupported.iter().any(|u| u.contains("extension host")));
        assert!(info.unsupported.contains(&"syntax grammars".to_string()));
        assert!(!root.path().join("me.demo-1.2.3/out/ext.js").exists());
        assert!(info.icon.is_none(), "no icon declared");
        let with_icon = vsix(&[("extension/package.json", r#"{"version":"1.0.0","icon":"images/logo.png","contributes":{}}"#), ("extension/images/logo.png", "PNGDATA")]);
        let ic = install_vsix(&with_icon, root.path(), "me.icon").unwrap();
        assert_eq!(ic.icon.as_deref(), Some("data:image/png;base64,UE5HREFUQQ=="));
        assert_eq!(list_installed(root.path()).iter().find(|e| e.id == "me.icon").unwrap().icon, ic.icon, "kept in the manifest");
        uninstall(root.path(), "me.icon").unwrap();
        let c = contributions(root.path());
        assert_eq!(c.themes[0].json["colors"]["editor.foreground"], "#eee");
        assert_eq!(c.themes[0].json["colors"]["editor.background"], "#101010");
        assert_eq!(c.snippets[0].json["log"]["prefix"], "cl");
        assert_eq!(list_installed(root.path()).len(), 1);
        uninstall(root.path(), "me.demo").unwrap();
        assert!(list_installed(root.path()).is_empty());
        assert!(uninstall(root.path(), "me.demo").is_err());
    }

    #[test]
    fn icon_themes_are_unpacked_and_loaded() {
        let pkg = r#"{"version":"2.0.0","contributes":{"iconThemes":[{"id":"pack","label":"Pack Icons","path":"./dist/icons.json"}],"productIconThemes":[{"id":"p"}]}}"#;
        let theme = r##"{"iconDefinitions":{"js":{"iconPath":"./../icons/js.svg"},"folder":{"iconPath":"./../icons/folder.svg"},"evil":{"iconPath":"./../../../outside.svg"},"glyph":{"fontCharacter":"\\uE001"}},"file":"js","folder":"folder","fileExtensions":{"js":"js"},"light":{"fileExtensions":{"js":"js"}}}"##;
        let bytes = vsix(&[
            ("extension/package.json", pkg),
            ("extension/dist/icons.json", theme),
            ("extension/icons/js.svg", "<svg xmlns='http://www.w3.org/2000/svg'/>"),
            ("extension/icons/folder.svg", "<svg/>"),
            ("extension/icons/unused.svg", "<svg/>"),
        ]);
        let root = tempfile::tempdir().unwrap();
        let info = install_vsix(&bytes, root.path(), "me.icons").unwrap();
        assert_eq!(info.icon_themes, vec![IconThemeRef { id: "pack".into(), label: "Pack Icons".into(), path: "dist/icons.json".into() }]);
        assert!(info.unsupported.contains(&"product icon themes".to_string()));
        assert!(!info.unsupported.iter().any(|u| u == "icon themes"));
        assert!(root.path().join("me.icons-2.0.0/icons/js.svg").exists());
        assert!(!root.path().join("me.icons-2.0.0/icons/unused.svg").exists(), "only referenced pictures");
        let d = icon_theme(root.path(), "me.icons", "pack").unwrap();
        assert_eq!(d.icons.len(), 2);
        assert!(d.icons["js"].starts_with("data:image/svg+xml;base64,"));
        assert_eq!(d.map["fileExtensions"]["js"], "js");
        assert!(d.map.get("iconDefinitions").is_none());
        assert!(icon_theme(root.path(), "me.icons", "nope").is_err());
        // Manifests written before icon themes existed still load.
        let old = root.path().join("old.ext-1.0.0");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join(MANIFEST), r#"{"id":"old.ext","displayName":"Old","description":"","version":"1.0.0","themes":[],"snippets":[],"unsupported":[]}"#).unwrap();
        assert!(list_installed(root.path()).iter().any(|e| e.id == "old.ext" && e.icon_themes.is_empty()));
    }

    /// `MATERIAL_VSIX=/path/PKief.material-icon-theme.vsix cargo test -- --ignored material`
    #[test]
    #[ignore]
    fn material_icon_theme() {
        let bytes = std::fs::read(std::env::var("MATERIAL_VSIX").unwrap()).unwrap();
        let root = tempfile::tempdir().unwrap();
        let info = install_vsix(&bytes, root.path(), "PKief.material-icon-theme").unwrap();
        let d = icon_theme(root.path(), &info.id, &info.icon_themes[0].id).unwrap();
        println!("{} icons, unsupported {:?}", d.icons.len(), info.unsupported);
        assert!(d.icons.len() > 1000);
        assert!(d.map["fileExtensions"]["ts"].is_string());
    }

    #[test]
    fn rejects_paths_that_escape() {
        let pkg = r#"{"version":"1.0.0","contributes":{"themes":[{"label":"x","path":"../../evil.json"}]}}"#;
        let bytes = vsix(&[("extension/package.json", pkg)]);
        let root = tempfile::tempdir().unwrap();
        assert!(install_vsix(&bytes, root.path(), "a.b").is_err());
        assert!(clean_rel("/etc/passwd").is_err());
        assert!(clean_rel("a\\b").is_err());
        assert_eq!(clean_rel("./a/./b.json").unwrap(), "a/b.json");
    }

    #[test]
    fn registry_urls_and_ids_are_checked() {
        assert!(registry_url("https://open-vsx.org/api/a/b/1/file/x.vsix").is_ok());
        assert!(registry_url("https://evil.example/api/a").is_err());
        assert!(registry_url("https://open-vsx.org.evil.example/api/a").is_err());
        assert!(valid_part("dracula-theme") && !valid_part("..") && !valid_part("a/b"));
    }

    #[test]
    fn reinstall_replaces_old_version() {
        let root = tempfile::tempdir().unwrap();
        let mk = |v: &str| vsix(&[("extension/package.json", &format!(r#"{{"version":"{v}","contributes":{{}}}}"#))]);
        install_vsix(&mk("1.0.0"), root.path(), "a.b").unwrap();
        install_vsix(&mk("1.1.0"), root.path(), "a.b").unwrap();
        let l = list_installed(root.path());
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].version, "1.1.0");
    }

    #[test]
    fn finds_readme_images() {
        let md = "# T\n![shot](images/a.png \"t\")\n<p align=center><img width=200 src=\"https://x.dev/b.svg\"></p>\n[not](c.png)";
        assert_eq!(image_refs(md), vec!["images/a.png".to_string(), "https://x.dev/b.svg".to_string()]);
        let base = raw_base("git+https://github.com/dracula/visual-studio-code.git");
        assert_eq!(base.as_deref(), Some("https://raw.githubusercontent.com/dracula/visual-studio-code/HEAD/"));
        assert_eq!(resolve_image("./images/a.png", base.as_deref()).unwrap(), "https://raw.githubusercontent.com/dracula/visual-studio-code/HEAD/images/a.png");
        assert_eq!(resolve_image("https://github.com/o/r/blob/main/x.png", None).unwrap(), "https://raw.githubusercontent.com/o/r/main/x.png");
        assert!(resolve_image("http://insecure/x.png", base.as_deref()).is_none());
        assert!(resolve_image("../up.png", base.as_deref()).is_none());
        let (t, s, i, o) = contributes_summary(&serde_json::json!({"main":"x.js","contributes":{"themes":[{"label":"Dark"}],"snippets":[{"language":"go"}],"iconThemes":[{"id":"m","label":"Icons"}],"commands":[]}}));
        assert_eq!((t, s, i, o), (vec!["Dark".to_string()], vec!["go".to_string()], vec!["Icons".to_string()], vec!["its own code".to_string(), "commands".to_string()]));
    }

    #[test]
    fn parses_search_results() {
        let v = serde_json::json!({"totalSize": 2, "extensions": [
            {"namespace":"dracula-theme","name":"theme-dracula","displayName":"Dracula Official","description":"d","version":"2.0.0","downloadCount":5,"averageRating":4.5,"verified":true},
            {"namespace":"x","name":"old","deprecated":true},
            {"namespace":"../bad","name":"n"}]});
        let items = parse_search(&v);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "dracula-theme.theme-dracula");
    }
}
