//! Small native leaves reuse exact visible cluster cells, keeping their glyph painter.
use crate::markdown_renderer::native_projection::{ParagraphWrapIndex, VisualClusterRow};
use gpui::{Bounds, Pixels, SharedString, TextLayout, WrappedLineLayout};
use std::sync::Arc;

/// The largest leaf laid out as one native text; longer leaves page through
/// the projected native window.
pub const MAX_NATIVE_SOURCE: usize = 16 * 1024;
#[derive(Default)]
pub(super) struct NativeGeometryCache {
    cached: Option<Cached>,
}
struct Cached {
    text: SharedString,
    keys: Vec<usize>,
    physical: Vec<Arc<WrappedLineLayout>>,
    index: ParagraphWrapIndex,
}
impl NativeGeometryCache {
    pub(super) fn visible(
        &mut self,
        text: &SharedString,
        layout: &TextLayout,
        clip: Bounds<Pixels>,
    ) -> Option<Arc<[VisualClusterRow]>> {
        if text.len() > MAX_NATIVE_SOURCE {
            self.cached = None;
            return None;
        }
        let physical = layout.line_layouts().into_vec();
        let keys: Vec<_> = physical
            .iter()
            .map(|line| Arc::as_ptr(line) as usize)
            .collect();
        let valid = self.cached.as_ref().is_some_and(|cached| {
            cached.text.as_ptr() == text.as_ptr()
                && cached.text.len() == text.len()
                && cached.keys == keys
        });
        if !valid {
            let index = ParagraphWrapIndex::from_native(&physical, text).ok()?;
            self.cached = Some(Cached {
                text: text.clone(),
                keys,
                physical,
                index,
            });
        }
        let cached = self.cached.as_ref()?;
        let height = f32::from(layout.line_height());
        if !height.is_finite() || height <= 0.0 {
            return None;
        }
        let top = f32::from(clip.top() - layout.bounds().top()).max(0.0);
        let bottom = f32::from(clip.bottom() - layout.bounds().top()).max(top);
        let start = ((top / height).floor() as usize).min(cached.index.row_count());
        let end = ((bottom / height).ceil() as usize)
            .saturating_add(1)
            .min(start.saturating_add(128))
            .min(cached.index.row_count());
        Some(
            cached
                .index
                .visual_rows(&cached.physical, start..end)
                .ok()?
                .into(),
        )
    }
}
