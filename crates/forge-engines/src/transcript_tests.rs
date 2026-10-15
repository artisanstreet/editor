//! Canonical snapshot cleanup and file diagnostics, shared by native drivers.
//! Request IDs, approvals, steers and provider responses stay in each replay.
use crate::{EngineEvent, LiveKind};
use std::path::Path;

pub(crate) fn read(provider: &str, case: &str) -> String {
    read_path(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("transcripts/{provider}/{case}.jsonl")),
    )
}
pub(crate) fn read_path(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
}

/// Merge only adjacent canonical live events. Driver replies/responses are
/// barriers because callers invoke this only for two neighboring Event steps.
/// Status and empty replacement events remain explicit snapshot evidence.
pub(crate) fn merge_live(previous: &mut EngineEvent, current: &EngineEvent) -> bool {
    if let EngineEvent::Live {
        target,
        text,
        append,
    } = current
        && let EngineEvent::Live {
            target: last_target,
            text: last_text,
            append: last_append,
        } = previous
        && last_target == target
        && *target != LiveKind::Status
        && (*append && *last_append
            || !*append && !*last_append && !text.is_empty() && !last_text.is_empty())
    {
        if *append {
            last_text.push_str(text);
        } else {
            last_text.clone_from(text);
        }
        return true;
    }
    false
}
