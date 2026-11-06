use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CheckpointState {
    pub name: String,
    pub route: String,
    pub focus: Option<String>,
    pub pointer: Option<(i32, i32)>,
    pub scroll: BTreeMap<String, (i64, i64)>,
    pub animation_time_ns: u64,
    pub pending: Vec<String>,
    pub observations: BTreeMap<String, serde_json::Value>,
}

impl CheckpointState {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() || self.route.trim().is_empty() {
            return Err("checkpoint name and route are required".into());
        }
        if self.scroll.keys().any(|key| key.trim().is_empty())
            || self.observations.keys().any(|key| key.trim().is_empty())
        {
            return Err("state keys cannot be empty".into());
        }
        if self.pending.iter().any(|item| item.trim().is_empty())
            || self.pending.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return Err("pending operations must be named, sorted and unique".into());
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<String, String> {
        self.validate()?;
        let value = serde_json::to_value(self).map_err(|error| error.to_string())?;
        let bytes = serde_json::to_vec(&canonical(value)).map_err(|error| error.to_string())?;
        Ok(hash(&bytes))
    }
}

fn canonical(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(values) => {
            let sorted: BTreeMap<_, _> = values
                .into_iter()
                .map(|(key, value)| (key, canonical(value)))
                .collect();
            serde_json::Value::Object(sorted.into_iter().collect())
        }
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.into_iter().map(canonical).collect())
        }
        value => value,
    }
}

fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) enum SourceEntry {
    Missing,
    Directory,
    File { sha256: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SourceSnapshot {
    pub entries: BTreeMap<String, SourceEntry>,
}

impl SourceSnapshot {
    pub fn capture(root: &Path, paths: &[PathBuf]) -> Result<Self, String> {
        if paths.is_empty() {
            return Err("source scope is empty".into());
        }
        let mut entries = BTreeMap::new();
        for path in paths {
            if path.as_os_str().is_empty()
                || !path
                    .components()
                    .all(|component| matches!(component, Component::Normal(_)))
            {
                return Err(format!("invalid source path: {}", path.display()));
            }
            let mut parent = PathBuf::new();
            for component in path.components() {
                parent.push(component.as_os_str());
                match std::fs::symlink_metadata(root.join(&parent)) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        return Err(format!("symlink in source scope: {}", parent.display()));
                    }
                    Ok(_) => (),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                    Err(error) => return Err(error.to_string()),
                }
            }
            visit(root, path, &mut entries)?;
        }
        Ok(Self { entries })
    }

    pub fn digest(&self) -> Result<String, String> {
        let bytes = serde_json::to_vec(self).map_err(|error| error.to_string())?;
        Ok(hash(&bytes))
    }
}

fn visit(
    root: &Path,
    relative: &Path,
    entries: &mut BTreeMap<String, SourceEntry>,
) -> Result<(), String> {
    let key = relative
        .to_str()
        .ok_or_else(|| "source path is not UTF-8".to_owned())?
        .replace('\\', "/");
    let path = root.join(relative);
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            entries.insert(key, SourceEntry::Missing);
            return Ok(());
        }
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    if metadata.file_type().is_symlink() {
        return Err(format!("symlink in source scope: {}", path.display()));
    }
    if metadata.is_file() {
        let bytes = std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        entries.insert(
            key,
            SourceEntry::File {
                sha256: hash(&bytes),
            },
        );
    } else if metadata.is_dir() {
        entries.insert(key, SourceEntry::Directory);
        for child in
            std::fs::read_dir(&path).map_err(|error| format!("{}: {error}", path.display()))?
        {
            let child = child.map_err(|error| error.to_string())?;
            visit(root, &relative.join(child.file_name()), entries)?;
        }
    } else {
        return Err(format!("unsupported source entry: {}", path.display()));
    }
    Ok(())
}

pub(super) struct SourceGuard {
    root: PathBuf,
    paths: Vec<PathBuf>,
    before: SourceSnapshot,
}

impl SourceGuard {
    pub fn start(root: PathBuf, paths: Vec<PathBuf>) -> Result<Self, String> {
        let before = SourceSnapshot::capture(&root, &paths)?;
        Ok(Self {
            root,
            paths,
            before,
        })
    }

    pub fn finish(self) -> Result<SourceSnapshot, String> {
        let after = SourceSnapshot::capture(&self.root, &self.paths)?;
        if self.before != after {
            return Err("source changed during capture".into());
        }
        Ok(after)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_guard_detects_modification_addition_and_deletion() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        let file = root.path().join("src/a");
        std::fs::write(&file, b"one").unwrap();
        let paths = vec![PathBuf::from("src"), PathBuf::from("deleted")];
        let snapshot = SourceSnapshot::capture(root.path(), &paths).unwrap();
        assert!(matches!(
            snapshot.entries.get("deleted"),
            Some(SourceEntry::Missing)
        ));
        let guard = SourceGuard::start(root.path().to_path_buf(), paths.clone()).unwrap();
        std::fs::write(&file, b"two").unwrap();
        assert!(guard.finish().is_err());
        let guard = SourceGuard::start(root.path().to_path_buf(), paths.clone()).unwrap();
        std::fs::write(root.path().join("src/new"), b"new").unwrap();
        assert!(guard.finish().is_err());
        let guard = SourceGuard::start(root.path().to_path_buf(), paths.clone()).unwrap();
        std::fs::remove_file(&file).unwrap();
        assert!(guard.finish().is_err());
        let guard = SourceGuard::start(root.path().to_path_buf(), paths).unwrap();
        assert!(guard.finish().is_ok());
        assert!(SourceSnapshot::capture(root.path(), &[PathBuf::from("../outside")]).is_err());
    }

    #[test]
    fn state_hash_is_canonical_and_rejects_ambiguous_pending_operations() {
        let mut state = CheckpointState {
            name: "ready".into(),
            route: "conversation".into(),
            focus: None,
            pointer: None,
            scroll: BTreeMap::new(),
            animation_time_ns: 0,
            pending: vec![],
            observations: BTreeMap::new(),
        };
        let first: serde_json::Value = serde_json::from_str(r#"{"z":1,"a":2}"#).unwrap();
        let second: serde_json::Value = serde_json::from_str(r#"{"a":2,"z":1}"#).unwrap();
        state.observations.insert("data".into(), first);
        let digest = state.digest().unwrap();
        state.observations.insert("data".into(), second);
        assert_eq!(digest, state.digest().unwrap());
        state.pending = vec!["b".into(), "a".into()];
        assert!(state.digest().is_err());
    }
}
