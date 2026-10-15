//! Summary statistic blocks. Shared presentation helpers live in `kit`.

use std::sync::Arc;

use serde_json::Value;

use super::{RichBlock, block};

mod stats;

#[cfg(test)]
mod tests;

/// Claims this family's kinds; `None` for every other kind.
pub(super) fn parse(kind: &str, value: &Value) -> Option<Result<Arc<dyn RichBlock>, String>> {
    Some(match kind {
        "stats" => block::<stats::Stats>(value),
        _ => return None,
    })
}
