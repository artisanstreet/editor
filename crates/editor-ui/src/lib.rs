//! Shared Artisan presentation and interaction primitives for `GPUI` consumers.

pub mod action_row;
pub mod asset_seam;
pub mod attachment_tile;
pub mod badge;
pub mod button;
pub mod card;
pub mod code_card;
pub mod copy_feedback;
pub mod dialog;
pub mod empty_state;
pub mod fade_arc;
mod fence_tokens;
pub mod focus_navigation;
pub mod fonts;
pub mod glass;
pub mod gradient;
pub mod hover_card;
pub mod icon;
pub mod image_preview;
pub mod inline_code_text;
pub mod markdown;
pub mod markdown_renderer;
pub mod measure;
pub mod media_frame;
pub mod motion;
pub mod overlay;
pub mod page;
pub mod picker_motion;
pub mod popover;
pub mod progress;
pub mod segmented_control;
pub mod select;
pub mod selectable_text;
pub mod shimmer_text;
pub mod slider;
pub mod status_chip;
pub mod switch;
pub mod tab_card;
pub mod terminal_text;
pub mod text_editor;
mod text_navigation;
pub mod text_runs;
mod textarea;
pub mod theme;
pub mod tree_row;
pub mod wheel_glide;
pub mod window_controls;
pub mod wordmark_loader;

pub use editor_assets::AssetId;

#[cfg(test)]
mod component_tests;

pub mod chart_navigation;
pub mod chat_bubble;
