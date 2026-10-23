//! Cold native shaping runs on the renderer's preparation thread, ahead of
//! queued parsing, never in a visible row.
//! A per-job WindowTextSystem drops its full shaped-line cache after projecting
//! the requested hot range. Platform-internal allocation peaks are not a hard
//! bound implied by the conservative admission estimate below.
use super::native_projection::{ParagraphWrapIndex, ProjectedNativeRow};
use gpui::{
    App, Pixels, SharedString, Task, TextRun, TextSystem, WindowTextSystem, WrappedLineLayout,
};
use std::ops::Range;
use std::sync::Arc;

const MAX_INPUT_BYTES: usize = 1024 * 1024;
const MAX_RUN_BYTES: usize = 128 * 1024;
const ADMITTED_ESTIMATE_BYTES: usize = 128 * 1024 * 1024;
const MAX_NATIVE_RESIDENT_BYTES: usize = 64 * 1024 * 1024;

/// A shaped glyph window, or why it couldn't be.
pub(super) type ShapeResult = Result<NativeWindow, String>;

/// Whether two paragraph sources are the same immutable text.
pub(super) fn same_source(a: &SharedString, b: &SharedString) -> bool {
    a.as_ptr() == b.as_ptr() && a.len() == b.len()
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(super) enum StartError {
    #[error("The native shaping worker is busy")]
    Busy,
    #[error("{0}")]
    Rejected(String),
}
impl From<String> for StartError {
    fn from(message: String) -> Self {
        Self::Rejected(message)
    }
}
impl From<&str> for StartError {
    fn from(message: &str) -> Self {
        Self::Rejected(message.into())
    }
}
pub(super) async fn wait_for_owner() {
    if let Ok(owner) = super::budget::shaping().acquire_owned().await {
        drop(owner);
    }
}
pub(super) fn admits(source: &SharedString, runs: &[TextRun]) -> bool {
    source.len() <= MAX_INPUT_BYTES
        && std::mem::size_of_val(runs) <= MAX_RUN_BYTES
        && source
            .len()
            .saturating_mul(256)
            .saturating_add(std::mem::size_of_val(runs))
            <= ADMITTED_ESTIMATE_BYTES
}

pub struct NativeWindow {
    pub index: Arc<ParagraphWrapIndex>,
    pub text: SharedString,
    pub runs: Arc<[TextRun]>,
    pub start: usize,
    pub rows: Vec<Arc<ProjectedNativeRow>>,
    _resident: Option<Arc<tokio::sync::OwnedSemaphorePermit>>,
}
impl NativeWindow {
    pub fn row_count(&self) -> usize {
        self.index.row_count()
    }
    pub fn resident_bytes(&self) -> usize {
        self.text.len()
            + std::mem::size_of_val(self.runs.as_ref())
            + std::mem::size_of::<Self>()
            + self.index.resident_bytes()
            + self.rows.capacity() * std::mem::size_of::<Arc<ProjectedNativeRow>>()
            + self
                .rows
                .iter()
                .map(|row| row.resident_bytes() + 2 * std::mem::size_of::<usize>())
                .sum::<usize>()
    }
}
/// Source, width and resolved style revisions belong to the caller's immutable
/// generation key. Publish only if that key still matches after awaiting.
pub(super) fn start(
    cx: &App,
    worker: &Arc<super::async_preparation::Worker>,
    source: SharedString,
    font_size: Pixels,
    runs: Arc<[TextRun]>,
    width: Pixels,
    window: Range<usize>,
) -> Result<Task<Result<NativeWindow, String>>, StartError> {
    if window.start > window.end || window.len() > 128 {
        return Err("Native projection request exceeds its visual row budget".into());
    }
    if !admits(&source, &runs) {
        return Err(
            "This paragraph exceeds the native shaping admission budget; open the full source"
                .into(),
        );
    }
    if runs
        .iter()
        .try_fold(0usize, |sum, run| sum.checked_add(run.len))
        != Some(source.len())
        || !f32::from(width).is_finite()
        || !f32::from(font_size).is_finite()
        || width <= Pixels::ZERO
        || font_size <= Pixels::ZERO
    {
        return Err("Native paragraph source, runs, or geometry do not match".into());
    }
    // `admits` bounds admitted work, not opaque native allocations. One
    // actual owner holds the permit until synchronous native shaping returns.
    let owner = super::budget::shaping()
        .try_acquire_owned()
        .map_err(|_busy| StartError::Busy)?;
    let system: Arc<TextSystem> = Arc::clone(cx.text_system());
    let (result, shaped) = futures::channel::oneshot::channel();
    let job = super::async_preparation::ShapeJob {
        work: Box::new(move || shape(owner, system, source, font_size, runs, width, window)),
        result,
    };
    let queue = worker.queue.clone();
    Ok(cx.background_executor().spawn(async move {
        if queue
            .send(super::async_preparation::Job::Shape(job))
            .await
            .is_err()
        {
            return Err("Markdown worker closed".to_owned());
        }
        shaped
            .await
            .unwrap_or_else(|_| Err("Markdown worker stopped before shaping".to_owned()))
    }))
}

/// Shapes the paragraph and projects the visual rows of `window`.
#[expect(
    clippy::used_underscore_binding,
    reason = "the charge is attached once the page knows its own size"
)]
fn shape(
    owner: tokio::sync::OwnedSemaphorePermit,
    system: Arc<TextSystem>,
    source: SharedString,
    font_size: Pixels,
    runs: Arc<[TextRun]>,
    width: Pixels,
    window: Range<usize>,
) -> ShapeResult {
    {
        let _owner = owner;
        let text = source;
        let worker = WindowTextSystem::new(system);
        let shaped = worker
            .shape_text(text.clone(), font_size, &runs, Some(width), None)
            .map_err(|error| error.to_string())?;
        let native_resident_bytes = shaped
            .iter()
            .map(|line| {
                std::mem::size_of::<WrappedLineLayout>()
                    + 2 * std::mem::size_of::<usize>()
                    + std::mem::size_of::<gpui::LineLayout>()
                    + 2 * std::mem::size_of::<usize>()
                    + line.wrap_boundaries.capacity() * std::mem::size_of::<gpui::WrapBoundary>()
                    + line.unwrapped_layout.runs.capacity() * std::mem::size_of::<gpui::ShapedRun>()
                    + line
                        .unwrapped_layout
                        .runs
                        .iter()
                        .map(|run| run.glyphs.capacity() * std::mem::size_of::<gpui::ShapedGlyph>())
                        .sum::<usize>()
            })
            .sum::<usize>();
        if native_resident_bytes > MAX_NATIVE_RESIDENT_BYTES {
            return Err(
                "Native paragraph snapshot exceeds its resident budget; open the full source"
                    .into(),
            );
        }
        let physical: Vec<_> = shaped
            .iter()
            .map(|line| {
                Arc::new(WrappedLineLayout {
                    unwrapped_layout: Arc::clone(&line.unwrapped_layout),
                    wrap_boundaries: line.wrap_boundaries.clone(),
                    wrap_width: line.wrap_width,
                })
            })
            .collect();
        let index = Arc::new(
            ParagraphWrapIndex::from_native(&physical, &text).map_err(|error| error.to_string())?,
        );
        let start = window.start.min(index.row_count());
        let end = window.end.min(index.row_count()).max(start);
        let rows = index
            .project(&physical, start..end)
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(Arc::new)
            .collect();
        // Every full native layout and per-job cache drops at this return.
        let mut ready = NativeWindow {
            index,
            text,
            runs,
            start,
            rows,
            _resident: None,
        };
        let bytes = ready.resident_bytes();
        if bytes > super::budget::NATIVE_READY_BYTES {
            return Err("Native paragraph page exceeds its presentation budget".into());
        }
        let capacity = Arc::new(
            super::budget::native_ready()
                .try_acquire_many_owned(super::budget::permits(bytes))
                .map_err(|_occupied| {
                    "Native paragraph presentation budget is occupied".to_owned()
                })?,
        );
        #[expect(
            clippy::expect_used,
            reason = "unpublished rows have exactly one owner"
        )]
        for row in &mut ready.rows {
            // These rows have not escaped publication yet; there is exactly
            // one Arc owner. All later geometry clones inherit the charge.
            Arc::get_mut(row)
                .expect("unpublished native row")
                .ready_budget = Some(Arc::clone(&capacity));
        }
        ready._resident = Some(capacity);
        Ok(ready)
    }
}
