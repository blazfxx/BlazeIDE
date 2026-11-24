//! The project sandbox. Every path that arrives from the UI or from the AI
//! goes through here before the core touches the disk.
//!
//! A path is allowed only when, after resolving `..` and following every
//! symlink in the part of it that exists, it is inside a granted project root.
//! Following symlinks is what stops `project/link -> /etc` style escapes; a
//! purely lexical check would let them through.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::RwLock;

use crate::error::{Error, Result};

#[derive(Default)]
pub struct Sandbox {
    roots: RwLock<Vec<PathBuf>>,
    /// Single files the user picked in a native dialog (backgrounds, audio).
    /// They may be served to the webview but are not writable.
    picked: RwLock<HashSet<PathBuf>>,
    /// The app's own media folder (copies of picked backgrounds/audio): always
    /// readable, never writable through the UI.
    media_dirs: RwLock<Vec<PathBuf>>,
}

impl Sandbox {
    pub fn new() -> Self {
        Self::default()
    }

    /// Trust a directory the user chose (open-folder dialog, recent list).
    pub fn grant_root(&self, dir: &Path) -> Result<PathBuf> {
        let canon = dunce::canonicalize(dir)
            .map_err(|_| Error::msg(format!("Not a folder: {}", dir.display())))?;
        if !canon.is_dir() {
            return Err(Error::msg(format!("Not a folder: {}", canon.display())));
        }
        let mut roots = self.roots.write().expect("sandbox lock");
        if !roots.contains(&canon) {
            roots.push(canon.clone());
        }
        Ok(canon)
    }

    pub fn grant_picked_file(&self, file: &Path) -> Result<PathBuf> {
        let canon = dunce::canonicalize(file)?;
        self.picked.write().expect("sandbox lock").insert(canon.clone());
        Ok(canon)
    }

    /// Make a folder readable (not writable) for the webview, creating it if
    /// needed. Used for the app's media folder.
    pub fn grant_media_dir(&self, dir: &Path) -> Result<PathBuf> {
        std::fs::create_dir_all(dir)?;
        let canon = dunce::canonicalize(dir)?;
        let mut dirs = self.media_dirs.write().expect("sandbox lock");
        if !dirs.contains(&canon) {
            dirs.push(canon.clone());
        }
        Ok(canon)
    }

    pub fn roots(&self) -> Vec<PathBuf> {
        self.roots.read().expect("sandbox lock").clone()
    }

    /// Resolve a path from the UI and require it to be inside a project.
    pub fn require(&self, raw: &str) -> Result<PathBuf> {
        let resolved = resolve_absolute(raw)?;
        if self.roots().iter().any(|r| resolved.starts_with(r)) {
            Ok(resolved)
        } else {
            Err(Error::OutsideProject(resolved.display().to_string()))
        }
    }

    /// Like `require`, but the path must BE one of the granted roots.
    pub fn require_root(&self, raw: &str) -> Result<PathBuf> {
        let resolved = resolve_absolute(raw)?;
        if self.roots().iter().any(|r| r == &resolved) {
            Ok(resolved)
        } else {
            Err(Error::OutsideProject(resolved.display().to_string()))
        }
    }

    pub fn is_root(&self, p: &Path) -> bool {
        self.roots().iter().any(|r| r == p)
    }

    /// Files the webview may load: anything inside a project, or a file the
    /// user explicitly picked.
    pub fn require_readable(&self, raw: &str) -> Result<PathBuf> {
        let resolved = resolve_absolute(raw)?;
        if self.roots().iter().any(|r| resolved.starts_with(r))
            || self.picked.read().expect("sandbox lock").contains(&resolved)
            || self.media_dirs.read().expect("sandbox lock").iter().any(|d| resolved.starts_with(d))
        {
            Ok(resolved)
        } else {
            Err(Error::OutsideProject(resolved.display().to_string()))
        }
    }
}

/// Resolve `rel` (as written by the AI: relative, `/`-separated) inside `root`.
/// Absolute paths, drive letters and anything that lands outside the root are
/// refused.
pub fn resolve_in(root: &Path, rel: &str) -> Result<PathBuf> {
    let rel = rel.trim();
    if rel.is_empty() {
        return Err(Error::msg("A path is required"));
    }
    let candidate = Path::new(rel);
    if candidate.is_absolute() || candidate.components().any(|c| matches!(c, Component::Prefix(_) | Component::RootDir)) {
        return Err(Error::msg(format!("Use a path relative to the project root, not {rel}")));
    }
    let root = dunce::canonicalize(root)?;
    let resolved = resolve_existing_prefix(&normalize(&root.join(candidate)))?;
    if resolved.starts_with(&root) {
        Ok(resolved)
    } else {
        Err(Error::msg(format!("Path escapes the project root: {rel}")))
    }
}

fn resolve_absolute(raw: &str) -> Result<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(Error::msg("Invalid path: expected a non-empty path"));
    }
    let p = Path::new(raw);
    if !p.is_absolute() {
        return Err(Error::msg(format!("Invalid path: expected an absolute path, got {raw}")));
    }
    resolve_existing_prefix(&normalize(p))
}

/// Lexically remove `.` and `..` (never above the root/prefix).
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                // Only pop normal segments; never climb past the root.
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Canonicalize the longest prefix of `p` that exists (resolving symlinks in
/// it), then re-append the rest. The rest does not exist yet, so it cannot
/// contain a symlink.
fn resolve_existing_prefix(p: &Path) -> Result<PathBuf> {
    let mut existing = p.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if existing.symlink_metadata().is_ok() {
            let mut base = dunce::canonicalize(&existing)?;
            for seg in tail.iter().rev() {
                base.push(seg);
            }
            return Ok(base);
        }
        match (existing.file_name().map(|s| s.to_os_string()), existing.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name);
                existing = parent.to_path_buf();
            }
            _ => return Ok(p.to_path_buf()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_inside_and_refuses_outside() {
        let dir = tempfile::tempdir().unwrap();
        let sb = Sandbox::new();
        let root = sb.grant_root(dir.path()).unwrap();
        std::fs::write(root.join("a.txt"), "x").unwrap();
        assert!(sb.require(root.join("a.txt").to_str().unwrap()).is_ok());
        assert!(sb.require(root.join("new/deep/file.rs").to_str().unwrap()).is_ok());
        let escape = format!("{}/../../etc/passwd", root.display());
        assert!(sb.require(&escape).is_err());
        assert!(sb.require("relative/path").is_err());
        assert!(sb.require("").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlink_escape() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let sb = Sandbox::new();
        let root = sb.grant_root(dir.path()).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("link")).unwrap();
        let through = root.join("link/secret.txt");
        assert!(sb.require(through.to_str().unwrap()).is_err());
        assert!(resolve_in(&root, "link/secret.txt").is_err());
    }

    #[test]
    fn resolve_in_rules() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        assert!(resolve_in(&root, "src/main.rs").unwrap().starts_with(&root));
        assert!(resolve_in(&root, "../x").is_err());
        assert!(resolve_in(&root, "a/../../x").is_err());
        assert!(resolve_in(&root, "/etc/passwd").is_err());
        assert!(resolve_in(&root, "").is_err());
        assert_eq!(resolve_in(&root, "a/./b/../c").unwrap(), root.join("a/c"));
    }

    #[test]
    fn root_and_picked() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let sb = Sandbox::new();
        let root = sb.grant_root(dir.path()).unwrap();
        assert!(sb.require_root(root.to_str().unwrap()).is_ok());
        assert!(sb.require_root(root.join("sub").to_str().unwrap()).is_err());
        let pic = other.path().join("bg.png");
        std::fs::write(&pic, "png").unwrap();
        assert!(sb.require_readable(pic.to_str().unwrap()).is_err());
        sb.grant_picked_file(&pic).unwrap();
        assert!(sb.require_readable(pic.to_str().unwrap()).is_ok());
        assert!(sb.require(pic.to_str().unwrap()).is_err(), "picked files are not writable paths");
    }

    #[test]
    fn media_dir_is_readable_after_a_restart() {
        let app = tempfile::tempdir().unwrap();
        let dir = app.path().join("media");
        // First run: grant, store a file.
        let sb = Sandbox::new();
        let d = sb.grant_media_dir(&dir).unwrap();
        let f = d.join("abc.mp4");
        std::fs::write(&f, "v").unwrap();
        // A fresh process (new Sandbox) grants the folder again at startup.
        let sb2 = Sandbox::new();
        assert!(sb2.require_readable(f.to_str().unwrap()).is_err());
        sb2.grant_media_dir(&dir).unwrap();
        assert!(sb2.require_readable(f.to_str().unwrap()).is_ok());
        assert!(sb2.require(f.to_str().unwrap()).is_err(), "media files are not writable paths");
        let outside = app.path().join("other.mp4");
        std::fs::write(&outside, "x").unwrap();
        assert!(sb2.require_readable(outside.to_str().unwrap()).is_err());
    }
}
