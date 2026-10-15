//! The one way a surface learns where an element sits: a probe records the
//! element's bounds while it paints, and the next frame reads them. Popups
//! that need a trigger's place (a tooltip beside a row, a menu at a button,
//! a hover pill between rows) all go through [`Measured`].

use gpui::{Bounds, IntoElement, Pixels, Point, Styled as _, canvas};
use std::{cell::Cell, rc::Rc};

/// The last painted bounds of one element.
#[derive(Clone, Debug, Default)]
pub struct Measured(Rc<Cell<Option<Bounds<Pixels>>>>);

impl Measured {
    /// Where the element painted last, in window coordinates.
    #[must_use]
    pub fn get(&self) -> Option<Bounds<Pixels>> {
        self.0.get()
    }

    /// Whether `position` falls inside the element's last bounds.
    #[must_use]
    pub fn contains(&self, position: &Point<Pixels>) -> bool {
        self.0.get().is_some_and(|bounds| bounds.contains(position))
    }

    /// Forgets the bounds (the element left the screen).
    pub fn clear(&self) {
        self.0.set(None);
    }

    /// A probe filling its parent that records the parent's bounds. Use it
    /// when the bounds are only read by later input (hit tests, a menu
    /// opened from the keyboard).
    pub fn record(&self) -> impl IntoElement {
        let bounds = Rc::clone(&self.0);
        canvas(
            move |measured, _, _| bounds.set(Some(measured)),
            |_, (), _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    /// Like [`Self::record`], and repaints the window when the bounds moved,
    /// for paint that is placed from them.
    pub fn track(&self) -> impl IntoElement {
        let bounds = Rc::clone(&self.0);
        canvas(
            move |measured, window, cx| {
                if bounds.replace(Some(measured)) != Some(measured) {
                    window.defer(cx, |window, _| window.refresh());
                }
            },
            |_, (), _, _| {},
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }
}
