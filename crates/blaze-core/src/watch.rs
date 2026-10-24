//! Project file watching: one OS watcher per project root, changes debounced
//! into a single "something changed" callback so the tree refreshes once per
//! burst (a git checkout or npm install fires thousands of events).

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::error::{Error, Result};
use crate::files::SKIP_DIRS;

const DEBOUNCE: Duration = Duration::from_millis(300);

pub type ChangeFn = Arc<dyn Fn() + Send + Sync>;

#[derive(Default, Clone)]
pub struct WatchManager {
    watchers: Arc<Mutex<HashMap<PathBuf, RecommendedWatcher>>>,
}

fn ignored(p: &Path) -> bool {
    p.components().any(|c| matches!(c, Component::Normal(n) if SKIP_DIRS.iter().any(|s| n == *s)))
}

impl WatchManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn watch(&self, root: &Path, on_change: ChangeFn) -> Result<()> {
        let mut map = self.watchers.lock().expect("watch lock");
        if map.contains_key(root) {
            return Ok(());
        }
        let (tx, rx) = channel::<()>();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                if ev.paths.iter().any(|p| !ignored(p)) {
                    let _ = tx.send(());
                }
            }
        })
        .map_err(|e| Error::msg(format!("Cannot watch {}: {e}", root.display())))?;
        watcher
            .watch(root, RecursiveMode::Recursive)
            .map_err(|e| Error::msg(format!("Cannot watch {}: {e}", root.display())))?;
        std::thread::Builder::new()
            .name("fs-watch".into())
            .spawn(move || {
                // Ends when the watcher (and with it the sender) is dropped.
                while rx.recv().is_ok() {
                    loop {
                        match rx.recv_timeout(DEBOUNCE) {
                            Ok(()) => continue,
                            Err(RecvTimeoutError::Timeout) => break,
                            Err(RecvTimeoutError::Disconnected) => return,
                        }
                    }
                    on_change();
                }
            })
            .map_err(|e| Error::msg(e.to_string()))?;
        map.insert(root.to_path_buf(), watcher);
        Ok(())
    }

    pub fn unwatch_all(&self) {
        self.watchers.lock().expect("watch lock").clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn ignores_skip_dirs() {
        assert!(ignored(Path::new("/p/node_modules/x/y.js")));
        assert!(ignored(Path::new("/p/.git/index")));
        assert!(!ignored(Path::new("/p/src/main.rs")));
    }

    #[test]
    fn debounces_bursts() {
        let d = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(d.path()).unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        let m = WatchManager::new();
        m.watch(&root, Arc::new(move || { h.fetch_add(1, Ordering::SeqCst); })).unwrap();
        std::thread::sleep(Duration::from_millis(200));
        for i in 0..20 {
            std::fs::write(root.join(format!("f{i}.txt")), "x").unwrap();
        }
        std::thread::sleep(Duration::from_millis(1200));
        let n = hits.load(Ordering::SeqCst);
        assert!((1..=3).contains(&n), "expected a debounced handful, got {n}");
        m.unwatch_all();
    }
}
