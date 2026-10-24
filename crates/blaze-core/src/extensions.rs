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
    /// Parts of the extension we cannot run (grammars, commands, its own code…).
    pub unsupported: Vec<String>,
    #[serde(skip)]
    pub dir: PathBuf,
}

impl Installed {
    pub fn supported_parts(&self) -> usize {
        self.themes.len() + self.snippets.len()
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
    for (key, label) in [("grammars", "syntax grammars"), ("languages", "new languages"), ("commands", "commands"), ("debuggers", "debuggers"), ("iconThemes", "icon themes"), ("keybindings", "keybindings"), ("configuration", "settings")] {
        if contributes.get(key).map(|v| !v.is_null()).unwrap_or(false) {
            unsupported.push(label.to_string());
        }
    }
    let display = text(&pkg, "displayName");
    let info = Installed {
        id: id.to_string(),
        display_name: if display.is_empty() || display.starts_with('%') { id.to_string() } else { display },
        description: if text(&pkg, "description").starts_with('%') { String::new() } else { text(&pkg, "description") },
        version,
        themes,
        snippets,
        unsupported,
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
