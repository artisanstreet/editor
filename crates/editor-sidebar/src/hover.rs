//! One sliding hover surface (`render_picker_hover_pill` with its probes):
//! the pill flies between the rows measured against the surface, as in the
//! previous Editor's sidebar, thread list and profile menu.

use editor_ui::picker_motion::{HoverRect, SlidingHoverState, sliding_hover_pill};
use editor_ui::theme::ArtisanTheme;
use gpui::{AnyElement, Bounds, IntoElement, Pixels, Styled as _, canvas};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Default)]
pub(crate) struct HoverSurface {
    pub(crate) state: Rc<RefCell<SlidingHoverState>>,
    bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
}

impl HoverSurface {
    /// Records the surface's bounds; place it first inside the surface.
    pub(crate) fn surface_probe(&self) -> impl IntoElement {
        let bounds = Rc::clone(&self.bounds);
        canvas(
            |_, _, _| {},
            move |measured, (), window, cx| {
                if bounds.borrow_mut().replace(measured) != Some(measured) {
                    window.defer(cx, |window, _| window.refresh());
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    /// Measures one row into the pill; place it inside the row.
    pub(crate) fn row_probe(&self, id: impl Into<String>) -> impl IntoElement {
        let id = id.into();
        let (state, surface) = (Rc::clone(&self.state), Rc::clone(&self.bounds));
        canvas(
            |_, _, _| {},
            move |bounds, (), window, cx| {
                let Some(surface) = *surface.borrow() else {
                    return;
                };
                if state
                    .borrow_mut()
                    .measure(&id, HoverRect::within(bounds, surface))
                {
                    window.defer(cx, |window, _| window.refresh());
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    /// The pill, painted under the rows.
    pub(crate) fn pill(
        &self,
        id: &str,
        theme: &ArtisanTheme,
        radius: Pixels,
        reduce_motion: bool,
    ) -> AnyElement {
        sliding_hover_pill(id, theme, &self.state, radius, true, reduce_motion)
    }

    /// The pointer entered or left row `id`.
    pub(crate) fn hovered(&self, id: &str, hovered: bool) {
        let mut state = self.state.borrow_mut();
        if hovered {
            state.set_active(id.to_owned());
        } else if state.active_id() == Some(id) {
            state.hide();
        }
    }

    pub(crate) fn clear(&self) {
        self.state.borrow_mut().clear();
    }

    pub(crate) fn hide(&self) {
        self.state.borrow_mut().hide();
    }
}
