use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{info, warn};

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct StateStore {
    path: String,
    #[serde(skip)]
    map: HashMap<String, String>,
}

impl StateStore {
    pub fn load(path: &str) -> Result<StateStore> {
        let mut store = StateStore {
            path: path.to_string(),
            map: HashMap::new(),
        };
        match std::fs::read_to_string(path) {
            Ok(raw) => match serde_json::from_str::<HashMap<String, String>>(&raw) {
                Ok(parsed) => store.map = parsed,
                Err(e) => warn!("state file {path} is corrupt, starting empty: {e}"),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                info!("no state file at {path}, starting empty");
            }
            Err(e) => {
                warn!("failed to read state file {path}, starting empty: {e}");
            }
        }
        Ok(store)
    }

    pub fn last_seen(&self, repo: &str) -> Option<&str> {
        self.map.get(repo).map(|s| s.as_str())
    }

    pub fn set(&mut self, repo: &str, tag: &str) {
        self.map.insert(repo.to_string(), tag.to_string());
    }

    pub fn save(&self) -> Result<()> {
        let tmp = format!("{}.tmp", self.path);
        let raw = serde_json::to_string_pretty(&self.map)
            .map_err(|e| anyhow!("failed to serialize state: {e}"))?;
        std::fs::write(&tmp, raw)
            .map_err(|e| anyhow!("failed to write state tmp file {tmp}: {e}"))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| {
            anyhow!(
                "failed to rename state tmp file {tmp} -> {}: {e}",
                self.path
            )
        })?;
        info!("state saved to {}", self.path);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        let p = path.to_str().unwrap();

        let mut store = StateStore::load(p).unwrap();
        store.set("fosrl/pangolin", "1.19.4");
        store.set("fosrl/newt", "1.13.0");
        store.save().unwrap();

        let loaded = StateStore::load(p).unwrap();
        assert_eq!(loaded.last_seen("fosrl/pangolin"), Some("1.19.4"));
        assert_eq!(loaded.last_seen("fosrl/newt"), Some("1.13.0"));
        assert_eq!(loaded.last_seen("other/repo"), None);
    }

    #[test]
    fn load_missing_file_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.json");
        let store = StateStore::load(path.to_str().unwrap()).unwrap();
        assert_eq!(store.last_seen("anything"), None);
    }

    #[test]
    fn load_corrupt_file_starts_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(&path, "{not valid json").unwrap();
        let store = StateStore::load(path.to_str().unwrap()).unwrap();
        assert_eq!(store.last_seen("anything"), None);
    }

    #[test]
    fn set_overwrites_existing_tag() {
        let mut store = StateStore {
            path: "ignored".to_string(),
            map: HashMap::new(),
        };
        store.set("a/b", "1.0");
        store.set("a/b", "2.0");
        assert_eq!(store.last_seen("a/b"), Some("2.0"));
    }
}
