//! The app's own copy of media the user picked for the workbench (background
//! image or video, custom ambient audio).
//!
//! Picked files used to be referenced in place: an image was inlined into the
//! settings as a data URL (which the 256 KB settings cap rejected for any real
//! photo) and a video was trusted only until the app restarted (the sandbox
//! grant lived in memory), so the background vanished on the next launch.
//! Now the file is copied once into `<app config>/media/<hash>.<ext>`; that
//! folder is always readable by the `blazemedia:` protocol and nothing else.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    Video,
    Audio,
}

impl MediaKind {
    pub fn parse(s: &str) -> Self {
        match s {
            "video" => MediaKind::Video,
            "audio" => MediaKind::Audio,
            _ => MediaKind::Image,
        }
    }

    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            MediaKind::Image => &["png", "jpg", "jpeg", "webp", "gif", "avif", "bmp"],
            MediaKind::Video => &["mp4", "webm", "mov", "m4v", "ogv"],
            MediaKind::Audio => &["mp3", "ogg", "oga", "wav", "m4a", "aac", "flac"],
        }
    }

    pub fn max_bytes(self) -> u64 {
        match self {
            MediaKind::Image => 50 * 1024 * 1024,
            MediaKind::Video => 1024 * 1024 * 1024,
            MediaKind::Audio => 200 * 1024 * 1024,
        }
    }

    fn label(self) -> &'static str {
        match self {
            MediaKind::Image => "image",
            MediaKind::Video => "video",
            MediaKind::Audio => "audio file",
        }
    }
}

/// Settings keys that may point at a file in the media folder.
pub const MEDIA_SETTING_KEYS: [(&str, MediaKind); 3] =
    [("backgroundImage", MediaKind::Image), ("backgroundVideo", MediaKind::Video), ("ambientCustomPath", MediaKind::Audio)];

pub fn media_dir(app_config_dir: &Path) -> PathBuf {
    app_config_dir.join("media")
}

/// Copy `src` into `dir` as `<first 32 hex of sha256>.<ext>` and return the
/// stored path. Re-importing the same content returns the same path.
pub fn import(dir: &Path, src: &Path, kind: MediaKind) -> Result<PathBuf> {
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .filter(|e| kind.extensions().contains(&e.as_str()))
        .ok_or_else(|| Error::msg(format!("That is not a supported {} ({})", kind.label(), kind.extensions().join(", "))))?;
    let meta = fs::metadata(src).map_err(|_| Error::msg(format!("Cannot read {}", src.display())))?;
    if !meta.is_file() {
        return Err(Error::msg(format!("Not a file: {}", src.display())));
    }
    if meta.len() > kind.max_bytes() {
        return Err(Error::msg(format!(
            "That {} is {} MB; the limit is {} MB",
            kind.label(),
            meta.len() / (1024 * 1024),
            kind.max_bytes() / (1024 * 1024)
        )));
    }
    fs::create_dir_all(dir)?;
    // Already one of ours: nothing to copy.
    if let (Ok(d), Ok(s)) = (dunce::canonicalize(dir), dunce::canonicalize(src)) {
        if s.parent() == Some(d.as_path()) {
            return Ok(s);
        }
    }
    let tmp = dir.join(format!(".import-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| -> Result<PathBuf> {
        let mut input = fs::File::open(src)?;
        let mut out = fs::File::create(&tmp)?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 256 * 1024];
        let mut total: u64 = 0;
        loop {
            let n = input.read(&mut buf)?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > kind.max_bytes() {
                return Err(Error::msg(format!("That {} is larger than {} MB", kind.label(), kind.max_bytes() / (1024 * 1024))));
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n])?;
        }
        out.sync_all()?;
        drop(out);
        let hash = hasher.finalize();
        let name: String = hash.iter().take(16).map(|b| format!("{b:02x}")).collect();
        let dest = dir.join(format!("{name}.{ext}"));
        if dest.exists() {
            let _ = fs::remove_file(&tmp);
        } else {
            fs::rename(&tmp, &dest)?;
        }
        Ok(dunce::canonicalize(&dest)?)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// True when `p` is a file directly inside `dir`.
pub fn is_stored(dir: &Path, p: &Path) -> bool {
    match (dunce::canonicalize(dir), dunce::canonicalize(p)) {
        (Ok(d), Ok(f)) => f.parent() == Some(d.as_path()),
        _ => false,
    }
}

/// Delete every stored file that `keep` does not reference (and leftover
/// temp files). Returns how many were removed.
pub fn prune(dir: &Path, keep: &[PathBuf]) -> usize {
    let keep: Vec<PathBuf> = keep.iter().filter_map(|k| dunce::canonicalize(k).ok()).collect();
    let Ok(entries) = fs::read_dir(dir) else { return 0 };
    let mut removed = 0;
    for e in entries.flatten() {
        let p = e.path();
        if !p.is_file() {
            continue;
        }
        let canon = dunce::canonicalize(&p).unwrap_or(p.clone());
        if !keep.contains(&canon) && fs::remove_file(&p).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// The media paths the settings currently reference (data URLs and empty
/// values are skipped).
pub fn referenced(settings: &serde_json::Map<String, serde_json::Value>) -> Vec<PathBuf> {
    MEDIA_SETTING_KEYS
        .iter()
        .filter_map(|(k, _)| settings.get(*k).and_then(|v| v.as_str()))
        .filter(|s| !s.is_empty() && !s.starts_with("data:"))
        .map(PathBuf::from)
        .collect()
}

/// Startup migration for settings written by older versions: media paths that
/// point outside the media folder are copied into it. Returns the settings
/// patch to save and the old paths that could not be copied (still present
/// but too large, say), which the caller may grant for this session so they
/// keep working as before.
pub fn migrate(dir: &Path, settings: &serde_json::Map<String, serde_json::Value>) -> (serde_json::Map<String, serde_json::Value>, Vec<PathBuf>) {
    let mut patch = serde_json::Map::new();
    let mut legacy = Vec::new();
    for (key, kind) in MEDIA_SETTING_KEYS {
        let Some(v) = settings.get(key).and_then(|v| v.as_str()) else { continue };
        if v.is_empty() || v.starts_with("data:") {
            continue;
        }
        let p = PathBuf::from(v);
        if is_stored(dir, &p) || !p.is_file() {
            continue;
        }
        match import(dir, &p, kind) {
            Ok(stored) => {
                patch.insert(key.to_string(), serde_json::Value::String(stored.display().to_string()));
            }
            Err(_) => legacy.push(p),
        }
    }
    (patch, legacy)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_dedupes_and_checks_kind() {
        let src = tempfile::tempdir().unwrap();
        let app = tempfile::tempdir().unwrap();
        let dir = media_dir(app.path());
        let pic = src.path().join("Wall Paper.JPG");
        fs::write(&pic, vec![7u8; 3 * 1024 * 1024]).unwrap();
        let a = import(&dir, &pic, MediaKind::Image).unwrap();
        assert!(a.starts_with(dunce::canonicalize(&dir).unwrap()));
        assert_eq!(a.extension().unwrap(), "jpg");
        assert_eq!(fs::read(&a).unwrap().len(), 3 * 1024 * 1024);
        // Same content again: same file, no duplicate.
        let b = import(&dir, &pic, MediaKind::Image).unwrap();
        assert_eq!(a, b);
        // Importing our own stored file is a no-op.
        assert_eq!(import(&dir, &a, MediaKind::Image).unwrap(), a);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        // Wrong kind for the extension.
        assert!(import(&dir, &pic, MediaKind::Video).is_err());
        let txt = src.path().join("x.txt");
        fs::write(&txt, "x").unwrap();
        assert!(import(&dir, &txt, MediaKind::Image).is_err());
        assert!(is_stored(&dir, &a));
        assert!(!is_stored(&dir, &pic));
    }

    #[test]
    fn migrates_old_paths() {
        let src = tempfile::tempdir().unwrap();
        let app = tempfile::tempdir().unwrap();
        let dir = media_dir(app.path());
        let vid = src.path().join("loop.webm");
        fs::write(&vid, "frames").unwrap();
        let mut settings = serde_json::Map::new();
        settings.insert("backgroundVideo".into(), serde_json::json!(vid.display().to_string()));
        settings.insert("ambientCustomPath".into(), serde_json::json!("/nope/missing.mp3"));
        settings.insert("backgroundImage".into(), serde_json::json!("data:image/png;base64,AAAA"));
        let (patch, legacy) = migrate(&dir, &settings);
        assert_eq!(patch.len(), 1);
        let stored = PathBuf::from(patch["backgroundVideo"].as_str().unwrap());
        assert!(is_stored(&dir, &stored));
        assert!(legacy.is_empty());
        // Running again on the migrated settings changes nothing.
        settings.extend(patch);
        assert!(migrate(&dir, &settings).0.is_empty());
    }

    #[test]
    fn prune_keeps_referenced() {
        let src = tempfile::tempdir().unwrap();
        let app = tempfile::tempdir().unwrap();
        let dir = media_dir(app.path());
        let one = src.path().join("a.png");
        let two = src.path().join("b.mp4");
        fs::write(&one, "one").unwrap();
        fs::write(&two, "two").unwrap();
        let a = import(&dir, &one, MediaKind::Image).unwrap();
        let b = import(&dir, &two, MediaKind::Video).unwrap();
        fs::write(dir.join(".import-x.tmp"), "junk").unwrap();
        let mut settings = serde_json::Map::new();
        settings.insert("backgroundVideo".into(), serde_json::json!(b.display().to_string()));
        settings.insert("backgroundImage".into(), serde_json::json!("data:image/png;base64,AAAA"));
        assert_eq!(prune(&dir, &referenced(&settings)), 2);
        assert!(!a.exists());
        assert!(b.exists());
    }
}
