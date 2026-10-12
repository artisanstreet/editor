//! Data kinds: `stats` and `table`. See `docs/rich-blocks.md`.
//!
//! The small controls these share with the agent family (part ids, text
//! buttons, shared slices) live in `agent`.

use std::rc::Rc;

use serde_json::Value;

use super::{RichBlock, block};

mod stats;

#[cfg(test)]
mod tests;

/// Claims this family's kinds; `None` for every other kind.
pub(super) fn parse(kind: &str, value: &Value) -> Option<Result<Rc<dyn RichBlock>, String>> {
    Some(match kind {
        "stats" => block::<stats::Stats>(value),
        _ => return None,
    })
}
