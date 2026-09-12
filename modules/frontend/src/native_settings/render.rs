//! Static settings chrome and per-section rendering.
//!
//! Extracted verbatim from `native_settings.rs` during the module split; the
//! `chrome` child owns the shared chrome helpers and the `sections` child owns
//! the section renderers behind the parent re-exports.

use super::*;

#[path = "render/chrome.rs"]
mod chrome;

#[path = "render/sections.rs"]
mod sections;

pub use self::chrome::{notification_gap_notice, telemetry_choice_caption};
