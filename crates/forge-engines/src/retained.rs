//! Immutable source descriptors retained after the full terminal event has been emitted.
use artisan_api::{ContentRef, INLINE_TEXT_BYTES, ItemKind, TextContent};

pub(crate) fn compact_terminal(kind: &mut ItemKind) -> Result<(), serde_json::Error> {
    let result: Result<(), std::convert::Infallible> = kind.try_visit_texts_mut(|text| {
        compact_text(text);
        Ok(())
    });
    match result {
        Ok(()) => {}
        Err(never) => match never {},
    }
    kind.compact_collections()
}

pub(crate) fn error(key: &str, error: serde_json::Error) -> crate::harness::EngineEvent {
    crate::harness::EngineEvent::Item {
        key: format!("retention-error:{key}"),
        parent: None,
        status: artisan_api::Status::Failed,
        kind: ItemKind::Error {
            kind: artisan_api::ErrorKind::Execution,
            message: format!("cannot normalize native records: {error}").into(),
            retryable: false,
        },
    }
}

pub(crate) fn ownership_error(key: &str, error: anyhow::Error) -> crate::harness::EngineEvent {
    crate::harness::EngineEvent::Item {
        key: format!("retention-error:{key}"),
        parent: None,
        status: artisan_api::Status::Failed,
        kind: ItemKind::Error {
            kind: artisan_api::ErrorKind::Execution,
            message: error.to_string().into(),
            retryable: false,
        },
    }
}

/// Only sources that Forge externalizes may become descriptors. This runs after
/// the complete event has entered the ordered event stream.
pub(crate) fn compact_text(text: &mut TextContent) {
    if let TextContent::Inline(value) = text
        && value.len() > INLINE_TEXT_BYTES
    {
        *text = TextContent::Stored(ContentRef::from_text(value));
    }
}

/// Native callback records must remain editable until answered. Bound their
/// aggregate ownership separately from compact, durable presentation data.
/// Counting serialization writes no temporary JSON string. The conservative
/// factor covers owned strings, vectors, hash entries, and duplicate native /
/// canonical records; it is an admission estimate, not an allocator profiler.
pub(crate) const NATIVE_STATE_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const PENDING_REQUESTS: usize = 128;
pub(crate) fn owned_charge(value: &impl serde::Serialize) -> anyhow::Result<usize> {
    struct Counter(usize);
    impl std::io::Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter(0);
    serde_json::to_writer(&mut counter, value)?;
    Ok(counter.0.saturating_mul(4))
}

pub(crate) fn admit_pending(
    count: usize,
    retained: usize,
    incoming: &serde_json::Value,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        count < PENDING_REQUESTS,
        "native pending requests exceed {PENDING_REQUESTS} records"
    );
    anyhow::ensure!(
        retained.saturating_add(owned_charge(incoming)?.saturating_mul(3)) <= NATIVE_STATE_BYTES,
        "native pending callback ownership exceeds the 32 MiB budget"
    );
    Ok(())
}

#[cfg(test)]
mod native_budget_tests {
    use super::*;
    #[test]
    fn unicode_and_structural_records_are_charged_without_a_json_copy() {
        let value = serde_json::json!({"label":"界🙂".repeat(4096), "ids": (0..5000).map(|id| id.to_string()).collect::<Vec<_>>()});
        assert_eq!(
            owned_charge(&value).unwrap(),
            serde_json::to_vec(&value).unwrap().len() * 4
        );
        assert!(admit_pending(0, NATIVE_STATE_BYTES - 1, &value).is_err());
        assert!(admit_pending(PENDING_REQUESTS, 0, &serde_json::json!({})).is_err());
    }
}

/// Charges only changed native owners; settled descriptors stay accounted until
/// their actual native record is removed. Ordinary token frames never walk the
/// complete historical registry.
#[derive(Default)]
pub(crate) struct Ownership {
    records: std::collections::HashMap<(bool, String), usize>,
    bytes: usize,
    pending: usize,
}
impl Ownership {
    pub(crate) fn replace(&mut self, key: &str, bytes: usize) {
        self.replace_owner(false, key, bytes);
    }
    pub(crate) fn replace_buffer(&mut self, key: &str, bytes: usize) {
        self.replace_owner(true, key, bytes);
    }
    pub(crate) fn append_buffer(&mut self, key: &str, bytes: usize) {
        let owner = (true, key.to_owned());
        let previous = self.records.get(&owner).copied().unwrap_or(0);
        let charge = if previous == 0 {
            key.len().saturating_mul(2).saturating_add(128)
        } else {
            0
        };
        let added = bytes.saturating_add(charge);
        self.records.insert(owner, previous.saturating_add(added));
        self.bytes = self.bytes.saturating_add(added);
    }
    fn replace_owner(&mut self, buffer: bool, key: &str, bytes: usize) {
        let charge = bytes
            .saturating_add(key.len().saturating_mul(2))
            .saturating_add(128);
        let previous = self
            .records
            .insert((buffer, key.to_owned()), charge)
            .unwrap_or(0);
        self.bytes = self.bytes.saturating_sub(previous).saturating_add(charge);
    }
    pub(crate) fn remove(&mut self, key: &str) {
        self.remove_owner(false, key);
    }
    pub(crate) fn remove_buffer(&mut self, key: &str) {
        self.remove_owner(true, key);
    }
    fn remove_owner(&mut self, buffer: bool, key: &str) {
        if let Some(previous) = self.records.remove(&(buffer, key.to_owned())) {
            self.bytes = self.bytes.saturating_sub(previous);
        }
    }
    pub(crate) fn bytes(&self) -> usize {
        self.bytes.saturating_add(self.pending)
    }
    pub(crate) fn set_pending(&mut self, bytes: usize) {
        self.pending = bytes;
    }
    pub(crate) fn check(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.bytes() <= NATIVE_STATE_BYTES,
            "native retained state exceeds the 32 MiB ownership budget"
        );
        Ok(())
    }
}

pub(crate) fn changed_keys(
    events: &[crate::harness::EngineEvent],
) -> std::collections::HashSet<&str> {
    events
        .iter()
        .filter_map(|event| match event {
            crate::harness::EngineEvent::Item { key, .. }
            | crate::harness::EngineEvent::Approval { key, .. }
            | crate::harness::EngineEvent::Status { key, .. } => Some(key.as_str()),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod incremental_ownership_tests {
    use super::*;
    #[test]
    fn changed_owner_replaces_old_charge_and_actual_removal_releases_it() {
        let mut budget = Ownership::default();
        budget.replace("tool", NATIVE_STATE_BYTES / 2);
        budget.replace("agent", NATIVE_STATE_BYTES / 2);
        assert!(budget.check().is_err());
        budget.replace("tool", 128);
        assert!(budget.check().is_ok());
        budget.remove("agent");
        let remaining = budget.bytes;
        budget.remove("agent");
        assert_eq!(budget.bytes, remaining);
        budget.remove("tool");
        assert_eq!(budget.bytes, 0);
    }
}

#[cfg(test)]
mod combined_budget_tests {
    use super::*;
    #[test]
    fn pending_callbacks_and_canonical_owners_share_one_budget_and_release_independently() {
        let mut owner = Ownership::default();
        owner.replace("tool", NATIVE_STATE_BYTES / 2);
        owner.set_pending(NATIVE_STATE_BYTES / 2);
        assert!(owner.check().is_err());
        assert!(admit_pending(0, owner.bytes(), &serde_json::json!({"request":"new"})).is_err());
        owner.set_pending(0);
        assert!(owner.check().is_ok());
        let before = owner.bytes();
        owner.replace("tool", 16);
        assert!(owner.bytes() < before);
        owner.remove("tool");
        assert_eq!(owner.bytes(), 0);
    }
}
