//! Small persistent app state: settings overrides, recent projects and the
//! cached model list. One JSON file in the per-user config directory, written
//! atomically. Secrets never go here (they live in the OS keychain).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::Result;

const MAX_RECENT: usize = 10;
const MAX_SETTINGS_BYTES: usize = 256 * 1024;

#[derive(Default, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct State {
    /// Only the keys the user changed. The UI merges them over its defaults,
    /// so new settings get their default without a migration.
    #[serde(default)]
    settings: Map<String, Value>,
    #[serde(default)]
    recent_projects: Vec<String>,
    #[serde(default)]
    model_cache: Option<Value>,
    #[serde(default)]
    account: Option<Value>,
    /// Random id for anonymous usage stats (only ever sent if the user opts in).
    #[serde(default)]
    install_id: Option<String>,
    #[serde(default)]
    providers: Vec<crate::providers::CustomProvider>,
    /// Sections other modules own (see `Store::section`).
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    extra: Map<String, Value>,
}

pub struct Store {
    path: PathBuf,
    state: Mutex<State>,
}

impl Store {
    pub fn open(dir: &Path) -> Self {
        let path = dir.join("state.json");
        let state = Self::read_disk_at(&path).unwrap_or_default();
        Store { path, state: Mutex::new(state) }
    }

    fn read_disk_at(path: &Path) -> Option<State> {
        fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok())
    }

    fn save(&self, st: &State) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = self.path.with_extension(format!("json.{}.tmp", std::process::id()));
        fs::write(&tmp, serde_json::to_vec_pretty(st)?)?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// Change the state safely when the IDE and the CLI both run: take an
    /// exclusive lock on `state.lock`, re-read the file (the other process
    /// may have written it), apply the change, write, unlock.
    fn mutate<R>(&self, f: impl FnOnce(&mut State) -> Result<R>) -> Result<R> {
        let mut st = self.state.lock().expect("store lock");
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let lock = fs::OpenOptions::new().create(true).truncate(false).write(true).open(self.path.with_extension("lock"))?;
        lock.lock()?;
        if let Some(disk) = Self::read_disk_at(&self.path) {
            *st = disk;
        }
        let out = f(&mut st)?;
        let saved = self.save(&st);
        let _ = lock.unlock();
        saved.map(|_| out)
    }

    /// Pick up changes another process (IDE or CLI) wrote.
    pub fn reload(&self) {
        if let Some(disk) = Self::read_disk_at(&self.path) {
            *self.state.lock().expect("store lock") = disk;
        }
    }

    pub fn settings(&self) -> Map<String, Value> {
        self.state.lock().expect("store lock").settings.clone()
    }

    /// Shallow-merge a patch from the UI. Rejects absurd payloads so a buggy
    /// or hostile page cannot bloat the file.
    pub fn patch_settings(&self, patch: Map<String, Value>) -> Result<()> {
        self.mutate(|st| {
            let mut next = st.settings.clone();
            for (k, v) in patch {
                if k.len() <= 64 {
                    next.insert(k, v);
                }
            }
            if serde_json::to_vec(&next)?.len() > MAX_SETTINGS_BYTES {
                return Err(crate::error::Error::msg("Settings are too large to save"));
            }
            st.settings = next;
            Ok(())
        })
    }

    pub fn recent_projects(&self) -> Vec<String> {
        self.state.lock().expect("store lock").recent_projects.clone()
    }

    pub fn add_recent_project(&self, root: &str) -> Result<()> {
        self.mutate(|st| {
            st.recent_projects.retain(|p| p != root);
            st.recent_projects.insert(0, root.to_string());
            st.recent_projects.truncate(MAX_RECENT);
            Ok(())
        })
    }

    pub fn model_cache(&self) -> Option<Value> {
        self.state.lock().expect("store lock").model_cache.clone()
    }

    pub fn set_model_cache(&self, v: Value) -> Result<()> {
        self.mutate(|st| {
            st.model_cache = Some(v);
            Ok(())
        })
    }

    pub fn install_id(&self) -> String {
        if let Some(id) = &self.state.lock().expect("store lock").install_id {
            return id.clone();
        }
        let fresh = uuid::Uuid::new_v4().to_string();
        self.mutate(|st| Ok(st.install_id.get_or_insert(fresh.clone()).clone())).unwrap_or(fresh)
    }

    pub fn providers(&self) -> Vec<crate::providers::CustomProvider> {
        self.state.lock().expect("store lock").providers.clone()
    }

    pub fn upsert_provider(&self, p: crate::providers::CustomProvider) -> Result<()> {
        self.mutate(|st| {
            match st.providers.iter_mut().find(|x| x.id == p.id) {
                Some(slot) => *slot = p,
                None => {
                    if st.providers.len() >= 20 {
                        return Err(crate::error::Error::msg("That is the maximum number of custom providers"));
                    }
                    st.providers.push(p);
                }
            }
            Ok(())
        })
    }

    pub fn remove_provider(&self, id: &str) -> Result<()> {
        self.mutate(|st| {
            st.providers.retain(|p| p.id != id);
            Ok(())
        })
    }

    /// Non-secret account details (email, username) for the signed-in user.
    pub fn account(&self) -> Option<Value> {
        self.state.lock().expect("store lock").account.clone()
    }

    pub fn set_account(&self, v: Option<Value>) -> Result<()> {
        self.mutate(|st| {
            st.account = v;
            Ok(())
        })
    }

    /// Free-form sections owned by other modules (BlBots, mobile access).
    /// Each is one JSON value under `extra.<name>`.
    pub fn section(&self, name: &str) -> Option<Value> {
        self.state.lock().expect("store lock").extra.get(name).cloned()
    }

    pub fn update_section<R>(&self, name: &str, f: impl FnOnce(&mut Value) -> Result<R>) -> Result<R> {
        self.mutate(|st| {
            let slot = st.extra.entry(name.to_string()).or_insert(Value::Null);
            f(slot)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn persists_and_merges() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::open(d.path());
        let mut p = Map::new();
        p.insert("theme".into(), json!("light"));
        s.patch_settings(p).unwrap();
        let mut p = Map::new();
        p.insert("editorFontSize".into(), json!(15));
        s.patch_settings(p).unwrap();
        for i in 0..12 {
            s.add_recent_project(&format!("/p{i}")).unwrap();
        }
        s.add_recent_project("/p5").unwrap();
        let s2 = Store::open(d.path());
        assert_eq!(s2.settings()["theme"], json!("light"));
        assert_eq!(s2.settings()["editorFontSize"], json!(15));
        let r = s2.recent_projects();
        assert_eq!(r.len(), 10);
        assert_eq!(r[0], "/p5");
        assert_eq!(r.iter().filter(|p| *p == "/p5").count(), 1);
    }

    #[test]
    fn rejects_huge() {
        let d = tempfile::tempdir().unwrap();
        let s = Store::open(d.path());
        let mut p = Map::new();
        p.insert("backgroundImage".into(), json!("x".repeat(300_000)));
        assert!(s.patch_settings(p).is_err());
    }

    #[test]
    fn two_processes_do_not_clobber_each_other() {
        let d = tempfile::tempdir().unwrap();
        let ide = Store::open(d.path());
        let cli = Store::open(d.path());
        let mut p = Map::new();
        p.insert("theme".into(), json!("light"));
        ide.patch_settings(p).unwrap();
        // The CLI's in-memory copy is stale, but its write re-reads first.
        let mut p = Map::new();
        p.insert("selectedModel".into(), json!("glm-5.3"));
        cli.patch_settings(p).unwrap();
        ide.add_recent_project("/x").unwrap();
        let fresh = Store::open(d.path());
        assert_eq!(fresh.settings()["theme"], json!("light"));
        assert_eq!(fresh.settings()["selectedModel"], json!("glm-5.3"));
        assert_eq!(fresh.recent_projects(), vec!["/x".to_string()]);
        cli.update_section("bots", |v| { *v = json!({"n": 1}); Ok(()) }).unwrap();
        ide.reload();
        assert_eq!(ide.section("bots"), Some(json!({"n": 1})));
        // Stable install id across processes.
        let id = ide.install_id();
        cli.reload();
        assert_eq!(cli.install_id(), id);
    }

    #[test]
    fn corrupt_file_starts_fresh() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("state.json"), "{not json").unwrap();
        assert!(Store::open(d.path()).recent_projects().is_empty());
    }
}
