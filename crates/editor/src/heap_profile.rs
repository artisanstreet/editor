//! `--features dhat-heap` (`nix run .#dev -- editor --dhat`): records every heap allocation
//! with its call stack and, when the Editor quits, writes `dhat-heap.json` beside the logs.
//! Open it in DHAT's viewer (<https://nnethercote.github.io/dh_view/dh_view.html>): sort by
//! "At t-gmax" for what held the heap at its peak, by "At t-end" for what was still held at
//! quit. Every allocation walks the stack, so the Editor runs noticeably slower. GPU driver
//! memory never passes through this allocator; VMMap shows that.

use std::sync::Mutex;

#[global_allocator]
static ALLOCATOR: dhat::Alloc = dhat::Alloc;

static PROFILER: Mutex<Option<dhat::Profiler>> = Mutex::new(None);

pub(crate) fn start() {
    let logs = editor_core::hosts::EditorFiles::default_location()
        .dir
        .join("logs");
    if let Err(error) = std::fs::create_dir_all(&logs) {
        tracing::warn!("heap profile: couldn't create {}: {error}", logs.display());
    }
    let path = logs.join("dhat-heap.json");
    tracing::info!("heap profile: writing {} at quit", path.display());
    let profiler = dhat::Profiler::builder()
        .file_name(path)
        .trim_backtraces(Some(32))
        .build();
    *PROFILER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(profiler);
}

/// Writes the profile; later calls do nothing.
pub(crate) fn finish() {
    let profiler = PROFILER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    drop(profiler);
}
