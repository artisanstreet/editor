//! The desktop sidebar as its own view, so it is a cached region of the
//! frame instead of part of the application root's render.
//!
//! The sidebar's content is still composed by [`NativeApplication`]: its
//! recent threads, selection, attention dots, collapsed state, and the
//! profile footer and menu are application state with many writers. What
//! this view adds is the boundary. The application root re-renders on every
//! frame any of its descendants is dirty (a live transcript line animates at
//! the display rate), while this view re-renders only when the application
//! itself changes state, which it always announces with `cx.notify()`, or
//! when the sidebar's own animations (the working dot's pulse) ask for a
//! frame. A conversation or composer frame therefore reuses the sidebar's
//! previous paint, and a sidebar frame reuses theirs.

use gpui::{
    AnyElement, Context, Entity, IntoElement, Render, Subscription, WeakEntity, Window, div,
};

use super::NativeApplication;

/// The sidebar region: renders [`NativeApplication::desktop_sidebar`] behind
/// its own view identity.
pub(super) struct SidebarView {
    application: WeakEntity<NativeApplication>,
    /// Every application state change can change what the sidebar shows
    /// (its listing, the open thread, the profile menu), and every one
    /// notifies the application, so the sidebar follows those notifications
    /// rather than an enumerated list of fields that would silently go stale
    /// when a new writer forgets it.
    _application_observation: Subscription,
    /// How often the sidebar has rendered: the seam the render-boundary
    /// tests count.
    #[cfg(test)]
    renders: usize,
}

impl SidebarView {
    pub(super) fn new(application: &Entity<NativeApplication>, cx: &mut Context<Self>) -> Self {
        Self {
            application: application.downgrade(),
            _application_observation: cx.observe(application, |_, _, cx| cx.notify()),
            #[cfg(test)]
            renders: 0,
        }
    }

    #[cfg(test)]
    pub(super) const fn renders(&self) -> usize {
        self.renders
    }
}

impl Render for SidebarView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        {
            self.renders += 1;
        }
        // The application owns this view, so it outlives it; the empty
        // fallback only covers the frame a closing window may still draw.
        let Some(application) = self.application.upgrade() else {
            return div().into_any_element();
        };
        application.update(cx, |application, cx| -> AnyElement {
            application.desktop_sidebar(window, cx).into_any_element()
        })
    }
}
