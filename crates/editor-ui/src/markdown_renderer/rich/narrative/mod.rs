//! Structure and narrative kinds: see `docs/rich-blocks.md`.

use std::fmt::Display;
use std::rc::Rc;

use gpui::{ElementId, SharedString};
use serde_json::Value;

use super::{RichBlock, block};

mod callout;
mod compare;

/// Claims this family's kinds; `None` for every other kind.
pub(super) fn parse(kind: &str, value: &Value) -> Option<Result<Rc<dyn RichBlock>, String>> {
    Some(match kind {
        "callout" => block::<callout::Callout>(value),
        "compare" => block::<compare::Compare>(value),
        _ => return None,
    })
}

/// The element id of one part of a view whose base id is `base`.
fn part(base: &SharedString, part: impl Display) -> ElementId {
    ElementId::Name(SharedString::from(format!("{base}-{part}")))
}

#[cfg(test)]
mod tests;
