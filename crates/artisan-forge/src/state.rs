//! The Forge state card.
//!
//! Mirrors `modules/forge/src/state.ts`: a small JSON file written atomically
//! so launchers can discover a running Forge (endpoint, instance, pid) and
//! detect crashes from its absence. Removed on clean shutdown.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// The state card contents; `version` is the legacy literal `1`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgeStateCard {
    endpoint: String,
    instance_id: String,
    pid: u32,
    started_at: String,
    version: u8,
}

impl ForgeStateCard {
    /// Builds the card for this process at startup time.
    #[must_use]
    pub fn new(endpoint: impl Into<String>, instance_id: impl Into<String>, pid: u32) -> Self {
        let now = time::OffsetDateTime::now_utc();
        Self {
            endpoint: endpoint.into(),
            instance_id: instance_id.into(),
            pid,
            started_at: format_rfc3339(now),
            version: 1,
        }
    }

    /// Loopback endpoint string of the listener (`host:port`).
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Instance identifier from configuration.
    #[must_use]
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// Owning process id.
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// RFC 3339 UTC start timestamp.
    #[must_use]
    pub fn started_at(&self) -> &str {
        &self.started_at
    }

    /// Writes the card by writing a sibling temp file then renaming over
    /// `path`, so readers never observe partial content.
    ///
    /// # Errors
    /// Returns I/O errors from temp-file creation, serialization-adjacent
    /// writes, or the final rename.
    pub fn write_atomic(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        let temp = path.with_extension("tmp");
        std::fs::write(&temp, bytes)?;
        std::fs::rename(&temp, path)
    }
}

/// Removes the state card on shutdown; tolerant of absence.
pub fn remove_state_card(path: &Path) {
    let _unused = std::fs::remove_file(path);
}

pub(crate) fn state_now_rfc3339() -> String {
    format_rfc3339(time::OffsetDateTime::now_utc())
}

fn format_rfc3339(moment: time::OffsetDateTime) -> String {
    // time's well-known Rfc3339 format keeps us dependency-free here while
    // matching the legacy IsoDateTime shape used by trace metadata.
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        moment.year(),
        u8::from(moment.month()),
        moment.day(),
        moment.hour(),
        moment.minute(),
        moment.second(),
        moment.millisecond(),
    )
}

#[cfg(test)]
mod tests {
    use super::{ForgeStateCard, remove_state_card};
    use std::path::PathBuf;

    fn temp_path(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("artisan-state-test-{name}-{}", std::process::id()));
        let _unused = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir creates");
        dir.join("forge-state.json")
    }

    #[test]
    fn card_round_trips_with_version_literal_one() {
        let card = ForgeStateCard::new("127.0.0.1:5111", "dev-1", 4242);
        assert_eq!(card.endpoint(), "127.0.0.1:5111");
        assert_eq!(card.version, 1);
        assert!(card.started_at().ends_with('Z'));
        assert!(card.started_at().contains('T'));
        let json = serde_json::to_string(&card).expect("serializes");
        assert!(json.contains("\"version\":1"));
        let parsed: ForgeStateCard = serde_json::from_str(&json).expect("parses");
        assert_eq!(parsed, card);
    }

    #[test]
    fn write_and_remove_round_trips() {
        let path = temp_path("write");
        let card = ForgeStateCard::new("127.0.0.1:1", "dev-2", 7);
        card.write_atomic(&path).expect("writes");
        assert!(path.exists());
        remove_state_card(&path);
        assert!(!path.exists());
        let _unused = std::fs::remove_dir_all(path.parent().expect("parent"));
    }
}
