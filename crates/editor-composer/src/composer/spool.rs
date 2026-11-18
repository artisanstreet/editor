use editor_core::{
    HostId,
    config_file::{self, ReadConfigError, WriteConfigError},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct DraftSpool {
    dir: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum ForgetSpooledDraftError {
    #[error("remove {path}")]
    Remove {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Serialize, Deserialize)]
struct SpooledDraft {
    host: HostId,
    scope: String,
    body: Value,
}

impl DraftSpool {
    #[must_use]
    pub fn new(editor_dir: &Path) -> Self {
        Self {
            dir: editor_dir.join("drafts"),
        }
    }

    pub fn keep(&self, host: &HostId, scope: &str, body: &Value) -> Result<(), WriteConfigError> {
        config_file::write(
            &self.path(host, scope),
            &SpooledDraft {
                host: host.clone(),
                scope: scope.to_owned(),
                body: body.clone(),
            },
        )
    }

    pub fn take(&self, host: &HostId, scope: &str) -> Result<Option<Value>, ReadConfigError> {
        let spooled: Option<SpooledDraft> = config_file::read(&self.path(host, scope))?;
        Ok(spooled
            .filter(|spooled| &spooled.host == host && spooled.scope == scope)
            .map(|spooled| spooled.body))
    }

    pub fn forget(&self, host: &HostId, scope: &str) -> Result<(), ForgetSpooledDraftError> {
        let path = self.path(host, scope);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(ForgetSpooledDraftError::Remove { path, source }),
        }
    }

    fn path(&self, host: &HostId, scope: &str) -> PathBuf {
        self.dir
            .join(format!("{}-{}.json", file_safe(&host.0), file_safe(scope)))
    }
}

fn file_safe(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' => char::from(byte).to_string(),
            byte => format!("_{byte:02x}"),
        })
        .collect()
}
