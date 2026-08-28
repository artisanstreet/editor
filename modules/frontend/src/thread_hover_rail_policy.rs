//! Dependency-free geometry and proximity state for the thread hover rail.
//!
//! This is the deterministic counterpart of
//! `routes/components/thread-hover-rail.svelte`. A host supplies observed
//! rectangles, pointer events, suppression state, and thread identities. The
//! policy retains only the resulting layout and interaction state; it does
//! not access a clock, DOM, renderer, router, or asynchronous capability.

#![allow(clippy::module_name_repetitions)]
#![forbid(unsafe_code)]

/// The number of working rows at which the rail starts scrolling.
pub const VISIBLE_WORKING_ROWS: usize = 10;

/// The minimum vertical inset used when placing the hover card.
pub const CARD_EDGE_INSET: f64 = 8.0;

/// A viewport-relative rectangle supplied by a host layout adapter.
///
/// Width is the validity fact used by the legacy hit test: a rectangle with
/// zero or negative width is absent. Height is intentionally not normalized;
/// card placement and inclusive edge checks consume the observed values as
/// supplied, including fractional and negative coordinates.
#[must_use = "pass the observed rectangle to the hover-rail policy"]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rectangle {
    /// The rectangle's left viewport coordinate.
    pub left: f64,
    /// The rectangle's top viewport coordinate.
    pub top: f64,
    /// The observed width.
    pub width: f64,
    /// The observed height.
    pub height: f64,
}

impl Rectangle {
    /// Creates a rectangle from its top-left coordinate and observed size.
    ///
    /// No coordinate or dimension is clamped or otherwise normalized.
    #[must_use = "use the constructed rectangle"]
    pub const fn new(left: f64, top: f64, width: f64, height: f64) -> Self {
        Self {
            left,
            top,
            width,
            height,
        }
    }

    /// Creates a rectangle from its four observed edges.
    ///
    /// This constructor preserves reversed or fractional edges by deriving
    /// the corresponding signed width and height directly.
    #[must_use = "use the constructed rectangle"]
    pub const fn from_edges(left: f64, top: f64, right: f64, bottom: f64) -> Self {
        Self {
            left,
            top,
            width: right - left,
            height: bottom - top,
        }
    }

    /// Returns the right edge implied by the observed left edge and width.
    #[must_use]
    pub const fn right(self) -> f64 {
        self.left + self.width
    }

    /// Returns the bottom edge implied by the observed top edge and height.
    #[must_use]
    pub const fn bottom(self) -> f64 {
        self.top + self.height
    }

    /// Tests inclusive containment using the legacy positive-width rule.
    ///
    /// The horizontal and vertical edges are inclusive. Height is not
    /// separately validated, matching the browser policy's `width > 0`
    /// guard followed by four edge comparisons.
    #[must_use]
    pub fn contains_inclusive(self, pointer: PointerPosition) -> bool {
        self.width > 0.0
            && pointer.x >= self.left
            && pointer.x <= self.right()
            && pointer.y >= self.top
            && pointer.y <= self.bottom()
    }
}

/// Short alias for callers that use the usual geometry vocabulary.
pub type Rect = Rectangle;

/// The most recently observed client pointer coordinate.
#[must_use]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointerPosition {
    /// The pointer's client X coordinate.
    pub x: f64,
    /// The pointer's client Y coordinate.
    pub y: f64,
}

impl PointerPosition {
    /// Creates a pointer position without rounding or clamping it.
    #[must_use = "use the constructed pointer position"]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Returns the X coordinate under the browser-facing name.
    #[must_use]
    pub const fn client_x(self) -> f64 {
        self.x
    }

    /// Returns the Y coordinate under the browser-facing name.
    #[must_use]
    pub const fn client_y(self) -> f64 {
        self.y
    }
}

/// Short alias for callers that use the shorter pointer vocabulary.
pub type Pointer = PointerPosition;

/// The rectangles observed for one pointer tracking pass.
///
/// `zone` is absent when the host has not mounted or measured the rail zone.
/// `card` is the visible card measurement, when one exists. The state policy
/// still checks its own engagement flag and ignores a card supplied while the
/// card is not engaged.
#[must_use]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PointerBounds {
    /// The rail's observed hit zone, if available.
    pub zone: Option<Rectangle>,
    /// The card's observed rectangle, if available.
    pub card: Option<Rectangle>,
}

impl PointerBounds {
    /// Creates one pointer-bound observation.
    #[must_use = "pass the bounds to pointer tracking"]
    pub const fn new(zone: Option<Rectangle>, card: Option<Rectangle>) -> Self {
        Self { zone, card }
    }

    /// Builds the card hit rectangle whose left edge reaches back to the
    /// zone's right edge.
    ///
    /// With no zone, the observed card is returned unchanged. With both
    /// rectangles present, the resulting width is exactly
    /// `card.right - zone.right`, as in the legacy reachable-card object.
    #[must_use]
    pub fn reachable_card(self) -> Option<Rectangle> {
        match (self.zone, self.card) {
            (None, card) => card,
            (Some(zone), Some(card)) => Some(Rectangle::from_edges(
                zone.right(),
                card.top,
                card.right(),
                card.bottom(),
            )),
            (Some(_), None) => None,
        }
    }
}

/// The measured working-row layout derived from one total height and count.
#[must_use]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorkingRowsLayout {
    /// The measured total height divided by the number of rows, or zero for
    /// an empty row list.
    pub row_height: f64,
    /// Whether the working list should receive a scroll container.
    pub scrolls: bool,
    /// The exact ten-row cap when scrolling is enabled.
    pub max_height: Option<f64>,
}

impl WorkingRowsLayout {
    /// Derives the working-row layout from the host's measured total height.
    ///
    /// A positive row height and more than ten rows are both required before
    /// scrolling is enabled. The cap is then exactly ten row heights.
    #[must_use = "use the measured working-row layout"]
    pub fn new(measured_total_height: f64, row_count: usize) -> Self {
        let row_height = if row_count == 0 {
            0.0
        } else {
            // A host can only materialize a finite, bounded number of rows in
            // this UI list. Converting that row count is inherent to dividing
            // its measured CSS-pixel height and is exact for realizable lists.
            #[allow(clippy::cast_precision_loss)]
            let row_count_as_float = row_count as f64;
            measured_total_height / row_count_as_float
        };
        let scrolls = row_height > 0.0 && row_count > VISIBLE_WORKING_ROWS;
        // This fixed ten-row policy constant is exactly representable as an
        // f64; the local allowance keeps the integer policy source explicit.
        #[allow(clippy::cast_precision_loss)]
        let visible_working_rows_as_float = VISIBLE_WORKING_ROWS as f64;
        let max_height = scrolls.then_some(row_height * visible_working_rows_as_float);

        Self {
            row_height,
            scrolls,
            max_height,
        }
    }

    /// Returns the measured row height.
    #[must_use]
    pub const fn row_height(self) -> f64 {
        self.row_height
    }

    /// Returns whether scrolling is enabled.
    #[must_use]
    pub const fn scrolls(self) -> bool {
        self.scrolls
    }

    /// Returns the ten-row cap, when scrolling is enabled.
    #[must_use]
    pub const fn cap_height(self) -> Option<f64> {
        self.max_height
    }
}

/// Derives the measured working-row layout without retaining state.
pub fn working_rows_layout(measured_total_height: f64, row_count: usize) -> WorkingRowsLayout {
    WorkingRowsLayout::new(measured_total_height, row_count)
}

/// A host-facing result from a reveal attempt.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ThreadHoverRailAction {
    /// No host operation is required.
    NoOp,
    /// The host should refresh its `now_ms` value once before rendering the
    /// newly revealed proximity state.
    RequestNowMs,
}

impl ThreadHoverRailAction {
    /// Returns whether this result requests one fresh host clock sample.
    #[must_use]
    pub const fn requests_now_ms(self) -> bool {
        matches!(self, Self::RequestNowMs)
    }

    /// Returns whether this result is inert at the host boundary.
    #[must_use]
    pub const fn is_no_op(self) -> bool {
        matches!(self, Self::NoOp)
    }
}

/// Short action alias for host adapters.
pub type HoverRailAction = ThreadHoverRailAction;

/// Stateful, platform-independent policy for one thread hover rail.
///
/// `T` is an opaque selected-thread identity. The default is [`String`],
/// while callers may use another host-owned identity type. Geometry and
/// pointer values remain independent of that identity.
#[must_use = "use the retained hover-rail state"]
#[derive(Clone, Debug, PartialEq)]
pub struct ThreadHoverRailPolicy<T = String> {
    near: bool,
    last_pointer: PointerPosition,
    card_thread: Option<T>,
    card_engaged: bool,
    card_travelled: bool,
    card_y: f64,
}

impl<T> ThreadHoverRailPolicy<T> {
    /// Creates an initially concealed rail with the legacy zero pointer
    /// memory and zero card position.
    #[must_use = "use the new hover-rail policy"]
    pub const fn new() -> Self {
        Self {
            near: false,
            last_pointer: PointerPosition::new(0.0, 0.0),
            card_thread: None,
            card_engaged: false,
            card_travelled: false,
            card_y: 0.0,
        }
    }

    /// Returns whether the rail currently owns proximity.
    #[must_use]
    pub const fn near(&self) -> bool {
        self.near
    }

    /// Alias for [`Self::near`].
    #[must_use]
    pub const fn proximity_active(&self) -> bool {
        self.near
    }

    /// Returns the newest remembered pointer coordinate.
    pub const fn last_pointer(&self) -> PointerPosition {
        self.last_pointer
    }

    /// Returns the selected card thread retained by the policy.
    #[must_use]
    pub fn card_thread(&self) -> Option<&T> {
        self.card_thread.as_ref()
    }

    /// Alias for [`Self::card_thread`].
    #[must_use]
    pub fn selected_thread(&self) -> Option<&T> {
        self.card_thread()
    }

    /// Returns whether the card currently accepts pointer interaction.
    #[must_use]
    pub const fn card_engaged(&self) -> bool {
        self.card_engaged
    }

    /// Returns whether the most recent card retarget should travel.
    #[must_use]
    pub const fn card_travelled(&self) -> bool {
        self.card_travelled
    }

    /// Returns the retained card Y placement.
    #[must_use]
    pub const fn card_y(&self) -> f64 {
        self.card_y
    }

    /// Applies the exact hover-card placement formula.
    ///
    /// A missing zone leaves the retained geometry unchanged. Present
    /// geometry is evaluated as
    /// `max(8, min(row.top - zone.top, zone.height - card.height - 8))`;
    /// no input is clamped before that formula runs.
    pub fn place_card(
        &mut self,
        row: Rectangle,
        zone: Option<Rectangle>,
        card_height: f64,
    ) -> bool {
        let Some(zone) = zone else {
            return false;
        };

        self.card_y = CARD_EDGE_INSET
            .max((row.top - zone.top).min(zone.height - card_height - CARD_EDGE_INSET));
        true
    }

    /// Retargets the one shared card to a hovered row.
    ///
    /// The first engagement is not travelled. Every retarget while the card
    /// is engaged is travelled, including a repeated identity. The selected
    /// identity and last geometry are retained when the card is later hidden.
    pub fn hover_row(
        &mut self,
        thread: T,
        row: Rectangle,
        zone: Option<Rectangle>,
        card_height: f64,
    ) {
        self.card_travelled = self.card_engaged;
        self.card_thread = Some(thread);
        self.place_card(row, zone, card_height);
        self.card_engaged = true;
    }

    /// Hides the card while retaining its selected identity and geometry.
    pub fn hide_card(&mut self) {
        self.card_engaged = false;
        self.card_travelled = false;
    }

    /// Conceals proximity and hides the card.
    pub fn conceal(&mut self) {
        self.near = false;
        self.hide_card();
    }

    /// Attempts to reveal proximity.
    ///
    /// A suppressed rail remains concealed and does not request a clock
    /// sample. A not-near to near transition requests one host `now_ms`
    /// refresh; repeated reveals are no-ops and do not read time here.
    pub fn reveal(&mut self, suppressed: bool) -> ThreadHoverRailAction {
        if suppressed || self.near {
            return ThreadHoverRailAction::NoOp;
        }

        self.near = true;
        ThreadHoverRailAction::RequestNowMs
    }

    /// Reconciles a host suppression value with remembered state.
    ///
    /// Activating suppression hides the card and clears active proximity.
    /// Deactivating suppression does not replay the old proximity state.
    pub fn reconcile_suppression(&mut self, suppressed: bool) {
        if !suppressed {
            return;
        }

        self.hide_card();
        self.near = false;
    }

    /// Tracks one pointer observation against the host-supplied rectangles.
    ///
    /// Pointer memory is updated before every early return. A missing zone is
    /// a missing host measurement and therefore leaves interaction state
    /// untouched. Once a zone exists, suppression conceals immediately. The
    /// engaged card widens the hit band by extending its left edge back to
    /// the zone's right edge; an unengaged card is ignored. Leaving both hit
    /// rectangles conceals only when proximity was already active.
    pub fn track_pointer(
        &mut self,
        pointer: PointerPosition,
        bounds: PointerBounds,
        suppressed: bool,
    ) -> ThreadHoverRailAction {
        self.last_pointer = pointer;

        let Some(zone) = bounds.zone else {
            return ThreadHoverRailAction::NoOp;
        };

        if suppressed {
            self.conceal();
            return ThreadHoverRailAction::NoOp;
        }

        let inside_zone = zone.contains_inclusive(pointer);
        let inside_card = if self.card_engaged {
            bounds
                .reachable_card()
                .is_some_and(|card| card.contains_inclusive(pointer))
        } else {
            false
        };

        if inside_zone || inside_card {
            self.reveal(false)
        } else {
            if self.near {
                self.conceal();
            }
            ThreadHoverRailAction::NoOp
        }
    }

    /// Applies focus departure using host-observed containment and the last
    /// remembered pointer position.
    ///
    /// The browser adapter supplies `related_target_in_zone` for DOM
    /// containment. When that is false, the remembered pointer keeps the rail
    /// open if it is still inside the measured zone, using inclusive edges.
    pub fn focus_departure(&mut self, related_target_in_zone: bool, zone: Option<Rectangle>) {
        let pointer_inside_zone =
            zone.is_some_and(|zone| zone.contains_inclusive(self.last_pointer));
        if !related_target_in_zone && !pointer_inside_zone {
            self.conceal();
        }
    }
}

impl<T> Default for ThreadHoverRailPolicy<T> {
    fn default() -> Self {
        Self::new()
    }
}
