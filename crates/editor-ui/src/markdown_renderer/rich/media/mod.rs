//! Media: see `docs/rich-blocks.md`.
//!
//! `image`, `image-compare`, `model`, and `audio`. Every `src` resolves
//! through [`source`], against the open project. The two image kinds share
//! [`picture`], which loads through GPUI's asset cache so the block knows the
//! image's intrinsic size before it lays out its frame or its divider.

use std::sync::Arc;

use serde_json::Value;

use super::{RichBlock, block};

mod audio;
mod color;
mod image;
mod image_compare;
mod model;
mod picture;
mod resource_cache;
mod source;

pub(in crate::markdown_renderer) use color::{color_copy_entries, color_paint, color_ranges};
pub use picture::{decode_image_bytes, image_resident_bytes};
pub use source::{MediaFetch, MediaFetcher, MediaRequest};

/// The actual fetch/decode future owns its permit, including synchronous native
/// parsing. Dropping a view cannot admit replacement work while that poll runs.
async fn work_permit() -> tokio::sync::OwnedSemaphorePermit {
    super::budget::media_work()
        .acquire_owned()
        .await
        .expect("media capacity never closes")
}

/// Claims this family's kinds; `None` for every other kind.
pub(super) fn parse(kind: &str, value: &Value) -> Option<Result<Arc<dyn RichBlock>, String>> {
    Some(match kind {
        "audio" => block::<audio::AudioBlock>(value),
        "image" => block::<image::ImageBlock>(value),
        "image-compare" => block::<image_compare::ImageCompare>(value),
        "model" => block::<model::ModelBlock>(value),
        _ => return None,
    })
}

#[cfg(test)]
mod tests;
