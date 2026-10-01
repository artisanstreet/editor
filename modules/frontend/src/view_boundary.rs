//! Cached view boundaries between the desktop frame's sibling regions.
//!
//! GPUI re-renders a view on every frame its window draws unless the view
//! is mounted cached, and notifying any view marks every ancestor view
//! dirty. Without boundaries one animating transcript line therefore
//! rebuilt the sidebar, the inspector, and the composer at the display
//! rate, and one pulsing sidebar dot rebuilt the whole transcript. A cached
//! view reuses its previous frame until it is itself notified (or the window
//! refreshes, as a resize, focus change, or input-modality change does), so
//! each region pays only for its own changes.
//!
//! A cached view is laid out from the style given here alone, never from its
//! content, so the style must fully determine its size (`size_full` inside a
//! sized slot), and the view's own root must fill those bounds.

use gpui::{AnyElement, App, Entity, IntoElement as _, Render, StyleRefinement};

/// Mounts `view` as a cached region laid out at `style`.
///
/// The view renders again only when it is notified; whoever owns the
/// state it renders from must notify it when that state changes.
pub(crate) fn cached_view<V: Render>(
    view: Entity<V>,
    style: StyleRefinement,
    cx: &App,
) -> AnyElement {
    #[cfg(test)]
    if !cx.has_global::<CachedViewsInTests>() {
        // GPUI's test-only `debug_bounds` map is filled while elements
        // prepaint and is not replayed when a cached subtree is reused, so a
        // reused region's selectors would vanish from every assertion made
        // after an unrelated frame. Tests render every region live unless
        // they opt in to measure the boundaries themselves.
        return view.into_any_element();
    }
    #[cfg(not(test))]
    let _ = cx;
    view.cached(style).into_any_element()
}

/// Opts a test's app into the production cached boundaries.
#[cfg(test)]
pub(crate) struct CachedViewsInTests;

#[cfg(test)]
impl gpui::Global for CachedViewsInTests {}
