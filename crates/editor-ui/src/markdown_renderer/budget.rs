//! The renderer's byte budget, in one place: the limit of every pool that
//! retained Markdown state charges, the process-wide pools themselves, and
//! the capacity measures the charges use. Owners (the preparation cache, the
//! literal and native-projection owners, media work) hold permits from here;
//! dropping a permit releases its charge.
//!
//! Cache charges use owned container capacities and retained string payloads.
//! SharedString's private inline/static representation is conservatively
//! charged by payload length; no dependency on its private allocation layout.

use std::sync::{Arc, LazyLock};
use tokio::sync::Semaphore;

/// Prepared documents a renderer keeps, plus the sources its jobs hold.
pub(super) const PREPARED_BYTES: usize = 32 * 1024 * 1024;
/// Published native paragraph pages, across every renderer.
pub(super) const NATIVE_READY_BYTES: usize = 32 * 1024 * 1024;
/// Raw literal leaves one renderer keeps live.
pub(super) const LITERAL_BYTES: usize = 8 * 1024 * 1024;
/// Literal fetches in flight per renderer.
pub(super) const LITERAL_FETCHES: usize = 4;
/// Media fetch/decode jobs at once, across every renderer.
const MEDIA_JOBS: usize = 4;

static NATIVE_READY: LazyLock<Arc<Semaphore>> =
    LazyLock::new(|| Arc::new(Semaphore::new(NATIVE_READY_BYTES)));
/// Native shaping runs one job at a time.
static SHAPING: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(1)));
static MEDIA: LazyLock<Arc<Semaphore>> = LazyLock::new(|| Arc::new(Semaphore::new(MEDIA_JOBS)));

/// A per-owner pool of `limit` permits.
pub(super) fn pool(limit: usize) -> Arc<Semaphore> {
    Arc::new(Semaphore::new(limit))
}

/// The shared pool published native pages charge.
pub(super) fn native_ready() -> Arc<Semaphore> {
    NATIVE_READY.clone()
}

/// The single native shaping slot.
pub(super) fn shaping() -> Arc<Semaphore> {
    SHAPING.clone()
}

/// The media work slots.
pub(super) fn media_work() -> Arc<Semaphore> {
    MEDIA.clone()
}

/// `bytes` as semaphore permits; saturates, so an oversized charge never
/// shrinks into a small one.
pub(in crate::markdown_renderer) fn permits(bytes: usize) -> u32 {
    u32::try_from(bytes).unwrap_or(u32::MAX)
}
pub(in crate::markdown_renderer) const fn vector<T>(values: &Vec<T>) -> usize {
    values.capacity() * std::mem::size_of::<T>()
}
pub(in crate::markdown_renderer) fn optional(value: Option<&String>) -> usize {
    value.map_or(0, String::capacity)
}
pub(in crate::markdown_renderer) fn strings(values: &Vec<String>) -> usize {
    vector(values) + values.iter().map(String::capacity).sum::<usize>()
}
pub(in crate::markdown_renderer) fn shared(values: &[gpui::SharedString]) -> usize {
    std::mem::size_of_val(values) + values.iter().map(|value| value.len()).sum::<usize>()
}
