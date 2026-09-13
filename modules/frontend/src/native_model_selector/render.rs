//! Rendering for the model trigger, menu, engine tabs, model rows, preview
//! pane, policy axes, and option tooltips.
//!
//! Extracted verbatim from `native_model_selector.rs` during the module split;
//! visibility was widened to `pub(super)` for methods read by sibling modules.
//! The trigger/menu half lives in the `menu` child and the rows/preview half in
//! the `rows` child behind the parent re-exports.

use super::*;

use super::interaction::{option_tooltip_key, option_tooltip_text};
use super::state::{fallback_model_view, fallback_model_view_from_state, humanize_variant};

#[path = "render/menu.rs"]
mod menu;

#[path = "render/rows.rs"]
mod rows;

#[cfg(test)]
pub(super) use self::menu::gradient_highlights;
pub(crate) use self::menu::{animate_picker_menu, engine_accent, engine_asset};
pub(crate) use self::rows::render_picker_hover_pill;
