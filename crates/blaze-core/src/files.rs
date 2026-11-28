//! File tree, file CRUD, project search and replace. Callers pass paths that
//! have already been through `Sandbox::require`.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::error::{Error, Result};

pub const SKIP_DIRS: &[&str] = &["node_modules", ".git", ".blazeide", ".voidide", "dist", "target"];
const SKIP_FILES: &[&str] = &[".DS_Store", "Thumbs.db"];
const MAX_DEPTH: usize = 8;
const MAX_TREE_ENTRIES: usize = 20_000;
const MAX_SEARCH_RESULTS: usize = 200;
const MAX_HITS_PER_FILE: usize = 20;
pub const MAX_TEXT_FILE: u64 = 1024 * 1024;
const MAX_PREVIEW: usize = 200;
/// Files larger than this are not opened in the editor.
const MAX_OPEN_FILE: u64 = 20 * 1024 * 1024;

#[derive(Debug, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FileNode {
    pub name: String,
    pub path: String,
    pub is_directory: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<FileNode>>,
    /// A folder whose contents were not read (the tree is big): asking for
    /// the tree of this path gives them.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

#[derive(Debug, Serialize, Clone, PartialEq)]
pub struct SearchHit {
    pub path: String,
    pub line: usize,
    pub preview: String,
}

#[derive(Debug, Serialize, Clone, Copy, PartialEq, Default)]
pub struct ReplaceResult {
    pub files: usize,
    pub replacements: usize,
}

struct Entry {
    name: String,
    path: PathBuf,
    is_dir: bool,
    /// A symlink (only listed for the file tree, see `list_tree`).
    link: bool,
}

/// Directory listing used everywhere: no symlinks (cycles and escapes), no
/// junk, directories first, then case-insensitive by name.
fn list(dir: &Path) -> Vec<Entry> {
    list_with(dir, false)
}

/// The same, for the file tree: symlinks to files and folders are shown too
/// (a folder link is never walked into, only opened on request). Search and
/// replace keep using `list`, which never follows links.
fn list_tree(dir: &Path) -> Vec<Entry> {
    list_with(dir, true)
}

fn list_with(dir: &Path, links: bool) -> Vec<Entry> {
    let Ok(rd) = fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<Entry> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let ft = e.file_type().ok()?;
            let name = e.file_name().to_string_lossy().into_owned();
            if ft.is_symlink() {
                if !links || SKIP_FILES.contains(&name.as_str()) {
                    return None;
                }
                // Where it points (a broken link shows nothing).
                let meta = fs::metadata(e.path()).ok()?;
                if meta.is_dir() && SKIP_DIRS.contains(&name.as_str()) {
                    return None;
                }
                if !meta.is_dir() && !meta.is_file() {
                    return None;
                }
                return Some(Entry { name, path: e.path(), is_dir: meta.is_dir(), link: true });
            }
            if ft.is_dir() {
                if SKIP_DIRS.contains(&name.as_str()) {
                    return None;
                }
            } else if !ft.is_file() || SKIP_FILES.contains(&name.as_str()) {
                return None;
            }
            Some(Entry { name, path: e.path(), is_dir: ft.is_dir(), link: false })
        })
        .collect();
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    out
}

/// The tree of a folder. Every level is read in full before the next one is
/// started, so a huge folder can never hide its siblings: when the whole tree
/// does not fit the entry budget, the deepest levels are left unread (marked
/// `truncated`) and loaded when the folder is opened.
pub fn build_tree(root: &Path) -> Result<FileNode> {
    if !root.is_dir() {
        return Err(Error::msg(format!("Not a directory: {}", root.display())));
    }
    let mut best: Option<Vec<FileNode>> = None;
    for depth in 1..=MAX_DEPTH {
        let mut budget = MAX_TREE_ENTRIES;
        match walk(root, 0, depth, &mut budget) {
            Some(kids) => best = Some(kids),
            None => break,
        }
    }
    // Not even one level fits: show the first entries of the top level.
    let kids = best.unwrap_or_else(|| {
        list_tree(root).into_iter().take(MAX_TREE_ENTRIES).map(|e| FileNode { children: None, truncated: e.is_dir, name: e.name, path: e.path.display().to_string(), is_directory: e.is_dir }).collect()
    });
    Ok(FileNode {
        name: root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| root.display().to_string()),
        path: root.display().to_string(),
        is_directory: true,
        children: Some(kids),
        truncated: false,
    })
}

/// `dir` listed down to `max` levels. None when the entries do not fit.
fn walk(dir: &Path, level: usize, max: usize, budget: &mut usize) -> Option<Vec<FileNode>> {
    let mut nodes = Vec::new();
    for e in list_tree(dir) {
        if *budget == 0 {
            return None;
        }
        *budget -= 1;
        let path = e.path.display().to_string();
        if !e.is_dir {
            nodes.push(FileNode { name: e.name, path, is_directory: false, children: None, truncated: false });
        } else if e.link || level + 1 >= max {
            nodes.push(FileNode { name: e.name, path, is_directory: true, children: None, truncated: true });
        } else {
            let kids = walk(&e.path, level + 1, max, budget)?;
            nodes.push(FileNode { name: e.name, path, is_directory: true, children: Some(kids), truncated: false });
        }
    }
    Some(nodes)
}

pub fn read_text(path: &Path) -> Result<String> {
    let meta = fs::metadata(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => Error::msg(format!("File not found: {}", path.display())),
        _ => Error::msg(format!("Failed to read {}", path.display())),
    })?;
    if meta.is_dir() {
        return Err(Error::msg(format!("Cannot read a directory: {}", path.display())));
    }
    if meta.len() > MAX_OPEN_FILE {
        return Err(Error::msg(format!("{} is too large to open ({} MB)", path.display(), meta.len() / 1_048_576)));
    }
    let bytes = fs::read(path)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Write via a temp file + rename so a crash never leaves a half-written file.
pub fn write_text(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_file_name(format!(
        ".{}.blazeide-tmp-{}",
        path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        uuid::Uuid::new_v4().simple()
    ));
    fs::write(&tmp, content)?;
    if let Ok(meta) = fs::metadata(path) {
        // Keep the original permissions (e.g. an executable script).
        let _ = fs::set_permissions(&tmp, meta.permissions());
    }
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        Error::msg(format!("Failed to write {}: {e}", path.display()))
    })
}

pub fn create_file(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::OpenOptions::new().write(true).create_new(true).open(path).map(|_| ()).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => Error::msg(format!("Already exists: {}", path.display())),
        _ => Error::msg(format!("Failed to create {}", path.display())),
    })
}

pub fn create_folder(path: &Path) -> Result<()> {
    if path.exists() {
        return Err(Error::msg(format!("Already exists: {}", path.display())));
    }
    fs::create_dir_all(path).map_err(|_| Error::msg(format!("Failed to create folder {}", path.display())))
}

pub fn rename(from: &Path, to: &Path) -> Result<()> {
    if from == to {
        return Ok(());
    }
    // A case-only rename on a case-insensitive disk (mac, Windows) is the same
    // file, so the "already exists" check must not fire for it.
    let case_only = from.to_string_lossy().to_lowercase() == to.to_string_lossy().to_lowercase();
    if to.exists() && !case_only {
        return Err(Error::msg(format!("Already exists: {}", to.display())));
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(from, to).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => Error::msg(format!("Not found: {}", from.display())),
        _ => Error::msg(format!("Failed to rename {}", from.display())),
    })
}

/// Deleting from the IDE moves to the OS trash (recoverable), never rm -rf.
pub fn delete_to_trash(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    trash::delete(path).map_err(|e| Error::msg(format!("Failed to move {} to the trash: {e}", path.display())))
}

fn looks_binary(buf: &[u8]) -> bool {
    buf.iter().take(8000).any(|b| *b == 0)
}

fn read_searchable(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    if meta.len() == 0 || meta.len() > MAX_TEXT_FILE {
        return None;
    }
    let buf = fs::read(path).ok()?;
    if looks_binary(&buf) {
        return None;
    }
    String::from_utf8(buf).ok()
}

/// Case-insensitive subsequence match ("idx" matches "index.ts").
pub fn fuzzy_name_match(query: &str, name: &str) -> bool {
    let q: Vec<char> = query.to_lowercase().chars().collect();
    if q.is_empty() {
        return false;
    }
    let mut i = 0;
    for c in name.to_lowercase().chars() {
        if i < q.len() && c == q[i] {
            i += 1;
        }
    }
    i == q.len()
}

fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

pub fn search(root: &Path, query: &str) -> Vec<SearchHit> {
    let query = query.trim();
    let mut hits = Vec::new();
    if query.is_empty() {
        return hits;
    }
    let lowered = query.to_lowercase();
    let mut budget = MAX_TREE_ENTRIES;
    search_dir(root, 0, &mut budget, query, &lowered, &mut hits);
    hits
}

fn search_dir(dir: &Path, depth: usize, budget: &mut usize, query: &str, lowered: &str, hits: &mut Vec<SearchHit>) {
    if depth >= MAX_DEPTH || *budget == 0 || hits.len() >= MAX_SEARCH_RESULTS {
        return;
    }
    let entries = list(dir);
    // Files first so shallow hits surface before deep ones.
    for e in entries.iter().filter(|e| !e.is_dir) {
        if *budget == 0 || hits.len() >= MAX_SEARCH_RESULTS {
            return;
        }
        *budget -= 1;
        let path = e.path.display().to_string();
        if fuzzy_name_match(query, &e.name) {
            hits.push(SearchHit { path: path.clone(), line: 1, preview: e.name.clone() });
        }
        let Some(text) = read_searchable(&e.path) else { continue };
        if !text.to_lowercase().contains(lowered) {
            continue;
        }
        let mut per_file = 0;
        for (i, line) in text.lines().enumerate() {
            if per_file >= MAX_HITS_PER_FILE || hits.len() >= MAX_SEARCH_RESULTS {
                break;
            }
            if line.to_lowercase().contains(lowered) {
                hits.push(SearchHit { path: path.clone(), line: i + 1, preview: truncate_chars(line.trim(), MAX_PREVIEW) });
                per_file += 1;
            }
        }
    }
    for e in entries.iter().filter(|e| e.is_dir) {
        if *budget == 0 || hits.len() >= MAX_SEARCH_RESULTS {
            return;
        }
        *budget -= 1;
        search_dir(&e.path, depth + 1, budget, query, lowered, hits);
    }
}

/// Replace every literal (case-sensitive) occurrence across the project's
/// text files.
pub fn replace_all(root: &Path, query: &str, replacement: &str) -> ReplaceResult {
    let mut res = ReplaceResult::default();
    if query.is_empty() {
        return res;
    }
    let mut budget = MAX_TREE_ENTRIES;
    replace_dir(root, 0, &mut budget, query, replacement, &mut res);
    res
}

fn replace_dir(dir: &Path, depth: usize, budget: &mut usize, query: &str, replacement: &str, res: &mut ReplaceResult) {
    if depth >= MAX_DEPTH || *budget == 0 {
        return;
    }
    for e in list(dir) {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        if e.is_dir {
            replace_dir(&e.path, depth + 1, budget, query, replacement, res);
            continue;
        }
        let Some(text) = read_searchable(&e.path) else { continue };
        let count = text.matches(query).count();
        if count == 0 {
            continue;
        }
        if write_text(&e.path, &text.replace(query, replacement)).is_ok() {
            res.files += 1;
            res.replacements += count;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        fs::create_dir_all(p.join("src/nested")).unwrap();
        fs::create_dir_all(p.join("node_modules/pkg")).unwrap();
        fs::write(p.join("src/index.ts"), "const hello = 1;\nconsole.log(hello);\n").unwrap();
        fs::write(p.join("src/nested/util.ts"), "export const HELLO = 'x';\n").unwrap();
        fs::write(p.join("node_modules/pkg/index.js"), "hello").unwrap();
        fs::write(p.join("image.bin"), [0u8, 1, 2, b'h', b'e', b'l', b'l', b'o']).unwrap();
        fs::write(p.join("README.md"), "# Readme\n").unwrap();
        d
    }

    #[test]
    fn tree_skips_and_orders() {
        let d = project();
        let t = build_tree(d.path()).unwrap();
        let names: Vec<_> = t.children.as_ref().unwrap().iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["src", "image.bin", "README.md"]);
        assert!(t.children.unwrap()[0].is_directory);
    }

    #[test]
    fn search_finds_names_and_content() {
        let d = project();
        let hits = search(d.path(), "hello");
        assert_eq!(hits.iter().filter(|h| h.path.ends_with("index.ts")).count(), 2);
        assert!(hits.iter().any(|h| h.path.ends_with("util.ts")), "case-insensitive");
        assert!(!hits.iter().any(|h| h.path.contains("node_modules")));
        assert!(!hits.iter().any(|h| h.path.ends_with("image.bin")), "binary skipped");
        assert!(search(d.path(), "idx").iter().any(|h| h.path.ends_with("index.ts") && h.preview == "index.ts"));
        assert!(search(d.path(), "   ").is_empty());
    }

    #[test]
    fn replace_is_literal_and_case_sensitive() {
        let d = project();
        let r = replace_all(d.path(), "hello", "world");
        assert_eq!(r, ReplaceResult { files: 1, replacements: 2 });
        assert_eq!(fs::read_to_string(d.path().join("src/index.ts")).unwrap(), "const world = 1;\nconsole.log(world);\n");
        assert_eq!(fs::read_to_string(d.path().join("src/nested/util.ts")).unwrap(), "export const HELLO = 'x';\n");
        assert_eq!(fs::read_to_string(d.path().join("node_modules/pkg/index.js")).unwrap(), "hello");
    }

    #[test]
    fn crud() {
        let d = project();
        let f = d.path().join("new/dir/a.txt");
        create_file(&f).unwrap();
        assert!(create_file(&f).is_err());
        write_text(&f, "abc").unwrap();
        assert_eq!(read_text(&f).unwrap(), "abc");
        let g = d.path().join("new/b.txt");
        rename(&f, &g).unwrap();
        assert!(!f.exists() && g.exists());
        create_folder(&d.path().join("folder")).unwrap();
        assert!(create_folder(&d.path().join("folder")).is_err());
        assert!(read_text(&d.path().join("nope")).is_err());
        assert!(read_text(&d.path().join("folder")).is_err());
    }

    #[test]
    fn fuzzy() {
        assert!(fuzzy_name_match("idx", "index.ts"));
        assert!(fuzzy_name_match("IDX", "index.ts"));
        assert!(!fuzzy_name_match("xyz", "index.ts"));
        assert!(!fuzzy_name_match("", "index.ts"));
    }

    #[test]
    fn a_huge_folder_cannot_hide_its_siblings() {
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        // "_refs" sorts first among the folders and holds more than the whole budget.
        for sub in ["a", "b"] {
            let dir = r.join("_refs").join(sub);
            fs::create_dir_all(&dir).unwrap();
            for i in 0..(MAX_TREE_ENTRIES / 2 + 100) {
                fs::write(dir.join(format!("f{i}.txt")), "x").unwrap();
            }
        }
        fs::create_dir_all(r.join("web/src")).unwrap();
        fs::write(r.join("web/src/page.tsx"), "x").unwrap();
        fs::create_dir_all(r.join("scripts")).unwrap();
        fs::write(r.join("scripts/shoot.mjs"), "x").unwrap();
        fs::write(r.join("README.md"), "x").unwrap();
        let tree = build_tree(r).unwrap();
        let top: Vec<&str> = tree.children.as_ref().unwrap().iter().map(|n| n.name.as_str()).collect();
        assert_eq!(top, vec!["_refs", "scripts", "web", "README.md"]);
        let web = tree.children.as_ref().unwrap().iter().find(|n| n.name == "web").unwrap();
        assert_eq!(web.children.as_ref().unwrap()[0].name, "src", "other folders are still opened");
        // The big ones are not read, and say so; opening one reads it.
        let refs = tree.children.as_ref().unwrap().iter().find(|n| n.name == "_refs").unwrap();
        let a = &refs.children.as_ref().unwrap()[0];
        assert!(a.truncated && a.children.is_none());
        let opened = build_tree(Path::new(&a.path)).unwrap();
        assert_eq!(opened.children.unwrap().len(), MAX_TREE_ENTRIES / 2 + 100);
    }

    #[test]
    fn a_small_tree_is_read_in_full() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("a/b/c")).unwrap();
        fs::write(d.path().join("a/b/c/deep.txt"), "x").unwrap();
        let tree = build_tree(d.path()).unwrap();
        let c = &tree.children.as_ref().unwrap()[0].children.as_ref().unwrap()[0].children.as_ref().unwrap()[0];
        assert_eq!((c.name.as_str(), c.truncated, c.children.as_ref().unwrap()[0].name.as_str()), ("c", false, "deep.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn links_show_in_the_tree_but_search_never_follows_them() {
        let d = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret.txt"), "needle").unwrap();
        fs::write(d.path().join("real.txt"), "needle").unwrap();
        fs::create_dir_all(d.path().join("src")).unwrap();
        std::os::unix::fs::symlink(d.path().join("real.txt"), d.path().join("link-file.txt")).unwrap();
        std::os::unix::fs::symlink(d.path().join("src"), d.path().join("link-dir")).unwrap();
        std::os::unix::fs::symlink(outside.path(), d.path().join("out")).unwrap();
        std::os::unix::fs::symlink(d.path().join("nowhere"), d.path().join("broken")).unwrap();
        let tree = build_tree(d.path()).unwrap();
        let kids = tree.children.unwrap();
        let names: Vec<&str> = kids.iter().map(|n| n.name.as_str()).collect();
        assert_eq!(names, vec!["link-dir", "out", "src", "link-file.txt", "real.txt"], "broken links stay hidden");
        let by = |n: &str| kids.iter().find(|k| k.name == n).unwrap();
        assert!(by("link-dir").is_directory && by("link-dir").truncated, "a folder link is not walked into");
        assert!(!by("link-file.txt").is_directory);
        // Search must not read through the link to the outside folder.
        let hits = search(d.path(), "needle");
        assert!(hits.iter().all(|h| !h.path.contains("secret.txt")), "{hits:?}");
    }
}
