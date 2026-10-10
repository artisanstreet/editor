//! Reusable native selectable text for transcript bodies.
//!
//! Pinned GPUI ships hit-testing and platform clipboard writes but no
//! selection concept (`docs/ui/GPUI_CAPABILITIES.md` §2.11), while the
//! transcript renders user bodies and assistant Markdown as many separate
//! text leaves. This module gives every leaf browser selection behavior:
//!
//! - One selection per window ([`document`]), so a drag that starts in one
//!   paragraph extends through list items, code blocks, and later messages
//!   exactly like a drag in Chrome, and `ctrl`+C copies all of it with
//!   block separators.
//! - Chrome click units: a click places the caret at the nearest character
//!   boundary, a double-click selects the word under the pointer (plus its
//!   trailing spaces on Windows), a triple-click the paragraph, and a drag
//!   after a double/triple-click extends by whole words/paragraphs (see
//!   [`crate::text_navigation`]). Shift+click extends the selection.
//! - A press outside any text clears the selection.
//! - The selection paints as boxes behind the glyphs in the theme's
//!   selection color, so text runs never recompile while selecting and
//!   glyph metrics never change.
//!
//! Elements are retained (`SelectableText::retained`): focus and the
//! compiled [`TextRun`]s live in GPUI element state keyed by the caller's
//! stable `ElementId`, so `MarkdownRenderer::render_source` stays
//! synchronous and stateless. Compiled runs are reused while the text,
//! inherited style, caller ranges, and overrides are unchanged; callers
//! that keep their ranges across frames (the Markdown renderer) hand them
//! over as shared slices, so an unchanged leaf compares pointers.
//!
//! Pointer handling: each leaf reports itself from capture-phase listeners
//! (which run in paint order, and which cached rows replay), and the first
//! bubble-phase listener processes the event once for all of them. A
//! gesture that leaves a selection consumes its release, so selecting
//! across a link never follows it; a clean click on a link activates it.
//!
//! Deliberate limits: read-only selection only — no caret, no editing, no
//! IME/composition handling, no keyboard extension of the selection, and
//! no auto-scroll while dragging past the viewport edge.

#![allow(clippy::module_name_repetitions)]

mod document;

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    App, AppContext as _, Bounds, ClipboardItem, CursorStyle, DispatchPhase, Element, ElementId,
    EntityId, FocusHandle, GlobalElementId, HighlightStyle, Hitbox, HitboxBehavior, Hsla,
    InspectorElementId, IntoElement, KeyDownEvent, LayoutId, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, SharedString, StyledText,
    TextLayout, TextRun, TextStyle, Window,
};

use crate::markdown_cache::same_allocation;
use crate::text_navigation::{SelectionUnit, caret_index_for_position};
use crate::theme::ArtisanTheme;

use document::{LeafKey, LiveLeaf, Observed, document};

pub use crate::text_runs::{TextRunOverride, compile_text_runs};

/// Folds an arbitrary byte index into a valid caret position for `text`.
///
/// Out-of-range indices saturate at the text end; mid-character indices
/// floor to the previous character boundary, so every returned value is a
/// legal slicing position.
#[must_use]
pub fn clamp_to_char_boundary(text: &str, index: usize) -> usize {
    let mut clamped = index.min(text.len());
    while clamped > 0 && !text.is_char_boundary(clamped) {
        clamped -= 1;
    }
    clamped
}

/// Returns whether a keystroke copies the current selection.
///
/// Accepts the platform-idiomatic modifier (`control` off macOS, either
/// `control` or `platform` anywhere so deterministic tests can drive the
/// same path) with `c` and no shift/alt held.
#[must_use]
pub fn is_copy_keystroke(key: &str, modifiers: &Modifiers) -> bool {
    (modifiers.control || modifiers.platform)
        && !modifiers.alt
        && !modifiers.shift
        && key.eq_ignore_ascii_case("c")
}

/// Returns whether a keystroke selects the whole element text.
///
/// Same modifier rule as [`is_copy_keystroke`], with `a`.
#[must_use]
pub fn is_select_all_keystroke(key: &str, modifiers: &Modifiers) -> bool {
    (modifiers.control || modifiers.platform)
        && !modifiers.alt
        && !modifiers.shift
        && key.eq_ignore_ascii_case("a")
}

/// The placeholder that reserves an inline image slot; never copied.
const INLINE_IMAGE_PLACEHOLDER: &str = "\u{2003}\u{2060}\u{00a0}";

/// Activation for one link range inside selectable text.
pub type SelectableLinkHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// A color square painted in a reserved slot of selectable text.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InlineSwatch {
    /// Byte offset of the slot's em space.
    pub offset: usize,
    /// The color, as painted.
    pub color: Hsla,
    /// Whether the square stands alone for its value (centered in the slot)
    /// or precedes the value it decorates (set left, a gap after it).
    pub alone: bool,
}

/// A right-click on one context range inside selectable text, with the
/// pointer's window position.
pub type SelectableContextHandler = Rc<dyn Fn(usize, Point<Pixels>, &mut Window, &mut App)>;

/// Caller ranges held either for one frame or shared across frames.
#[derive(Clone, Debug)]
enum RangeSlice<T> {
    Owned(Vec<T>),
    Shared(Rc<[T]>),
}

impl<T> Default for RangeSlice<T> {
    fn default() -> Self {
        Self::Owned(Vec::new())
    }
}

impl<T: PartialEq> RangeSlice<T> {
    fn as_slice(&self) -> &[T] {
        match self {
            Self::Owned(values) => values,
            Self::Shared(values) => values,
        }
    }

    /// Whether both hold the same ranges, comparing shared pointers first.
    fn same(&self, other: &Self) -> bool {
        if let (Self::Shared(left), Self::Shared(right)) = (self, other)
            && Rc::ptr_eq(left, right)
        {
            return true;
        }
        self.as_slice() == other.as_slice()
    }
}

/// One compiled layout and every input it was compiled from.
#[derive(Debug)]
struct CompiledRuns {
    text: SharedString,
    default_style: TextStyle,
    highlights: RangeSlice<(Range<usize>, HighlightStyle)>,
    overrides: RangeSlice<TextRunOverride>,
    runs: Vec<TextRun>,
}

/// The inputs of one layout's run compilation.
struct RunInputs<'a> {
    text: &'a SharedString,
    default_style: &'a TextStyle,
    highlights: &'a mut RangeSlice<(Range<usize>, HighlightStyle)>,
    overrides: &'a mut RangeSlice<TextRunOverride>,
}

impl CompiledRuns {
    fn matches(&self, inputs: &RunInputs<'_>) -> bool {
        self.text.len() == inputs.text.len()
            && (same_allocation(&self.text, inputs.text) || self.text == *inputs.text)
            && self.highlights.same(inputs.highlights)
            && self.overrides.same(inputs.overrides)
            && self.default_style == *inputs.default_style
    }
}

/// Compiles runs for one layout.
fn compile_runs(inputs: &RunInputs<'_>) -> Vec<TextRun> {
    compile_text_runs(
        inputs.text,
        inputs.default_style,
        inputs.highlights.as_slice(),
        inputs.overrides.as_slice(),
    )
}

/// Framework-retained state of one leaf, stored through
/// `Window::with_element_state` under the element's stable id, so it
/// persists exactly while the element keeps painting.
#[derive(Clone, Default)]
struct RetainedLeaf {
    focus: Option<FocusHandle>,
    /// Runs compiled for the last layout, reused while their inputs hold.
    compiled: Rc<RefCell<Option<CompiledRuns>>>,
    /// Compilations performed, so tests can prove reuse.
    #[cfg(test)]
    compilations: Rc<std::cell::Cell<u64>>,
}

impl RetainedLeaf {
    /// Returns the runs for one layout, compiling only when an input
    /// changed since the previous layout.
    ///
    /// A miss moves the caller ranges into the cache (the element is
    /// consumed by this frame and never reads them again), so neither path
    /// copies them.
    fn compiled_runs(&self, inputs: &mut RunInputs<'_>) -> Vec<TextRun> {
        let Ok(mut compiled) = self.compiled.try_borrow_mut() else {
            return compile_runs(inputs);
        };
        if let Some(cached) = compiled.as_ref()
            && cached.matches(inputs)
        {
            return cached.runs.clone();
        }
        let runs = compile_runs(inputs);
        #[cfg(test)]
        self.compilations.set(self.compilations.get() + 1);
        *compiled = Some(CompiledRuns {
            text: inputs.text.clone(),
            default_style: inputs.default_style.clone(),
            highlights: std::mem::take(inputs.highlights),
            overrides: std::mem::take(inputs.overrides),
            runs: runs.clone(),
        });
        runs
    }
}

/// The selection identity of a painted leaf.
#[derive(Clone)]
struct LeafFrame {
    key: LeafKey,
    focus: FocusHandle,
}

/// Read-only selectable text element for transcript bodies.
///
/// Construct one per render from the owning view. Pointer selection
/// (shared across every leaf in the window), focused keyboard copy and
/// select-all, and link activation that stands down during selection are
/// all owned here, so plain user bodies and styled Markdown/code blocks
/// share one behavior.
pub struct SelectableText {
    id: ElementId,
    text: SharedString,
    base_highlights: RangeSlice<(Range<usize>, HighlightStyle)>,
    text_run_overrides: RangeSlice<TextRunOverride>,
    inline_images: Vec<(usize, std::sync::Arc<gpui::RenderImage>)>,
    selection_color: Hsla,
    link_ranges: Rc<[Range<usize>]>,
    on_link: Option<SelectableLinkHandler>,
    context_ranges: Rc<[Range<usize>]>,
    on_context: Option<SelectableContextHandler>,
    inline_swatches: Rc<[InlineSwatch]>,
    swatch_edge: Hsla,
    copies: Rc<[(Range<usize>, SharedString)]>,
    styled: Option<StyledText>,
    leaf: Option<LeafFrame>,
}

impl SelectableText {
    /// Constructs retained selectable text for one render frame.
    ///
    /// `base_highlights` are caller ranges already addressing `text` (for
    /// example inline-code or syntax ranges). Focus and compiled runs
    /// persist in framework element state under `id` across frames and are
    /// released when the element stops painting. Ids must be unique per
    /// text element, like all GPUI element ids; without a view-backed
    /// identity the element paints but cannot be selected.
    #[must_use]
    pub fn retained(
        id: impl Into<ElementId>,
        text: impl Into<SharedString>,
        theme: ArtisanTheme,
        base_highlights: Vec<(Range<usize>, HighlightStyle)>,
    ) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            base_highlights: RangeSlice::Owned(base_highlights),
            text_run_overrides: RangeSlice::default(),
            inline_images: Vec::new(),
            selection_color: theme.interaction.selection_background.to_paint(),
            link_ranges: Rc::from([]),
            on_link: None,
            context_ranges: Rc::from([]),
            on_context: None,
            inline_swatches: Rc::from([]),
            swatch_edge: theme.colors.foreground.with_alpha(0.22).to_paint(),
            copies: Rc::from([]),
            styled: None,
            leaf: None,
        }
    }

    /// Paints color squares in reserved em-space slots, preserving text
    /// layout and selection.
    #[must_use]
    pub fn with_inline_swatches(mut self, swatches: Rc<[InlineSwatch]>) -> Self {
        self.inline_swatches = swatches;
        self
    }

    /// Ranges of the text that copy as other text, sorted and disjoint: a
    /// swatch slot copies as its color value, or as nothing beside a shown
    /// value.
    #[must_use]
    pub fn with_copy_substitutions(mut self, copies: Rc<[(Range<usize>, SharedString)]>) -> Self {
        self.copies = copies;
        self
    }

    /// Supplies ranges that answer a right-click, such as color values,
    /// plus the handler that receives the range's index and the pointer's
    /// window position. A right-click elsewhere in the text passes through.
    #[must_use]
    pub fn shared_context_ranges(
        mut self,
        ranges: Rc<[Range<usize>]>,
        on_context: impl Fn(usize, Point<Pixels>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.context_ranges = ranges;
        self.on_context = Some(Rc::new(on_context));
        self
    }

    /// Supplies link ranges plus their activation handler.
    ///
    /// Activation fires only when press and release land in the same range
    /// and the gesture left no selection, so drag-selecting across a link
    /// never follows it.
    #[must_use]
    pub fn links(
        self,
        ranges: Vec<Range<usize>>,
        on_link: impl Fn(usize, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.shared_links(ranges.into(), on_link)
    }

    /// Supplies link ranges the caller keeps across frames; otherwise
    /// identical to [`Self::links`].
    #[must_use]
    pub fn shared_links(
        mut self,
        ranges: Rc<[Range<usize>]>,
        on_link: impl Fn(usize, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.link_ranges = ranges;
        self.on_link = Some(Rc::new(on_link));
        self
    }

    /// Replaces the caller highlight ranges with ranges the caller keeps
    /// across frames, so an unchanged layout reuses its compiled runs by
    /// pointer instead of comparing or copying ranges.
    #[must_use]
    pub fn with_shared_highlights(
        mut self,
        highlights: Rc<[(Range<usize>, HighlightStyle)]>,
    ) -> Self {
        self.base_highlights = RangeSlice::Shared(highlights);
        self
    }

    /// Supplies per-range font family and letter-spacing overrides.
    ///
    /// Code ranges pass their mono family with `Some(px(0.0))` spacing
    /// while code weight travels in the highlight ranges. Overrides are
    /// validated fail-closed at layout time (see [`compile_text_runs`]).
    #[must_use]
    pub fn with_text_run_overrides(mut self, overrides: Vec<TextRunOverride>) -> Self {
        self.text_run_overrides = RangeSlice::Owned(overrides);
        self
    }

    /// Supplies run overrides the caller keeps across frames; otherwise
    /// identical to [`Self::with_text_run_overrides`].
    #[must_use]
    pub fn with_shared_text_run_overrides(mut self, overrides: Rc<[TextRunOverride]>) -> Self {
        self.text_run_overrides = RangeSlice::Shared(overrides);
        self
    }

    /// Adds decorative images in reserved em-space slots, preserving text layout and selection.
    #[must_use]
    pub fn with_inline_images(
        mut self,
        images: Vec<(usize, std::sync::Arc<gpui::RenderImage>)>,
    ) -> Self {
        self.inline_images = images;
        self
    }

    /// Returns the rendered text.
    #[must_use]
    pub fn text(&self) -> &str {
        self.text.as_ref()
    }

    /// Resolves the retained state, creating the focus handle on first use.
    fn retained_state(
        global_id: Option<&GlobalElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<RetainedLeaf> {
        window.with_optional_element_state(
            global_id,
            |stored: Option<Option<RetainedLeaf>>, _: &mut Window| {
                let Some(stored) = stored else {
                    return (None, None);
                };
                let mut retained = stored.unwrap_or_default();
                if retained.focus.is_none() {
                    retained.focus = Some(cx.focus_handle());
                }
                (Some(retained.clone()), Some(retained))
            },
        )
    }
}

impl Element for SelectableText {
    type RequestLayoutState = ();
    type PrepaintState = Hitbox;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let retained = Self::retained_state(global_id, window, cx);
        let default_style = window.text_style();
        let mut inputs = RunInputs {
            text: &self.text,
            default_style: &default_style,
            highlights: &mut self.base_highlights,
            overrides: &mut self.text_run_overrides,
        };
        let runs = match retained.as_ref() {
            Some(retained) => retained.compiled_runs(&mut inputs),
            None => compile_runs(&inputs),
        };
        self.leaf = retained
            .and_then(|retained| retained.focus)
            .zip(global_id)
            .map(|(focus, global_id)| LeafFrame {
                key: LeafKey::new(global_id),
                focus,
            });
        let mut styled = StyledText::new(self.text.clone()).with_runs(runs);
        let (layout_id, ()) = styled.request_layout(None, inspector_id, window, cx);
        self.styled = Some(styled);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Hitbox {
        if self.styled.is_none() {
            // Defensive: the framework always runs `request_layout` first,
            // so this only covers abnormal embedders. Rebuild from the
            // caller highlights and run overrides (a legal prepaint-phase
            // layout) rather than losing the frame or the shaping.
            let default_style = window.text_style();
            let runs = compile_text_runs(
                self.text.as_ref(),
                &default_style,
                self.base_highlights.as_slice(),
                self.text_run_overrides.as_slice(),
            );
            let mut styled = StyledText::new(self.text.clone()).with_runs(runs);
            let _ = styled.request_layout(None, inspector_id, window, cx);
            self.styled = Some(styled);
        }
        if let Some(leaf) = self.leaf.as_ref() {
            window.set_focus_handle(&leaf.focus, cx);
        }
        let Some(styled) = self.styled.as_mut() else {
            return window.insert_hitbox(bounds, HitboxBehavior::Normal);
        };
        styled.prepaint(None, inspector_id, bounds, state, window, cx);
        if let Some(leaf) = self.leaf.as_ref() {
            let window_id = window.window_handle().window_id();
            let doc = document(cx, window_id);
            if doc.is_active() {
                let observed = Observed {
                    key: leaf.key,
                    text: self.text.clone(),
                    copies: Rc::clone(&self.copies),
                    view: window.current_view(),
                    bounds: styled.layout().bounds(),
                    line_height: styled.layout().line_height(),
                };
                if doc.observe_in_frame(observed) {
                    cx.defer(move |cx| document(cx, window_id).flush_frame());
                }
            }
        }
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_state: &mut Self::RequestLayoutState,
        hitbox: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(layout) = self.styled.as_ref().map(|styled| styled.layout().clone()) else {
            return;
        };

        if hitbox.is_hovered(window) {
            let hovered = layout.index_for_position(window.mouse_position()).ok();
            let over_link = hovered
                .is_some_and(|index| self.link_ranges.iter().any(|range| range.contains(&index)));
            window.set_cursor_style(
                if over_link {
                    CursorStyle::PointingHand
                } else {
                    CursorStyle::IBeam
                },
                hitbox,
            );
        }

        if let Some(leaf) = self.leaf.clone() {
            let doc = document(cx, window.window_handle().window_id());
            // Every prepaint of this frame has run, so leaves new to this
            // frame already have their place in the order.
            doc.flush_frame();
            if let Some(range) = doc.range_in(leaf.key, &self.text) {
                paint_selection(&layout, &range, self.selection_color, window);
            }
            let live = LiveLeaf {
                observed: Observed {
                    key: leaf.key,
                    text: self.text.clone(),
                    copies: Rc::clone(&self.copies),
                    view: window.current_view(),
                    bounds: layout.bounds(),
                    line_height: layout.line_height(),
                },
                layout: layout.clone(),
                hovered: false,
                focus: Some(leaf.focus.clone()),
                links: self.link_ranges.clone(),
                on_link: self.on_link.clone(),
            };
            register_pointer_listeners(&live, hitbox, window);
            if let Some(on_context) = self.on_context.clone()
                && !self.context_ranges.is_empty()
            {
                register_context_listener(
                    layout.clone(),
                    Rc::clone(&self.context_ranges),
                    on_context,
                    hitbox,
                    window,
                );
            }
            register_key_listener(&leaf, live.observed.clone(), window);
        }

        if let Some(styled) = self.styled.as_mut() {
            styled.paint(None, inspector_id, bounds, &mut (), &mut (), window, cx);
        }
        for (index, image) in &self.inline_images {
            if let (Some(origin), Some(end)) = (
                layout.position_for_index(*index),
                layout.position_for_index(*index + '\u{2003}'.len_utf8()),
            ) {
                let side = end.x - origin.x;
                if side > gpui::px(0.0) {
                    let icon_bounds = Bounds::new(
                        origin + gpui::point(gpui::px(0.0), (layout.line_height() - side) / 2.0),
                        gpui::size(side, side),
                    );
                    let _ = window.paint_image(
                        icon_bounds,
                        icon_bounds,
                        gpui::Corners::default(),
                        image.clone(),
                        0,
                        false,
                    );
                }
            }
        }
        for swatch in self.inline_swatches.iter() {
            paint_swatch(&layout, swatch, self.swatch_edge, window);
        }
    }
}

impl IntoElement for SelectableText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// Paints one color square in its slot: as wide as most of the slot and no
/// taller than a little over half the line, rounded, with a hairline edge
/// so dark colors hold their shape on a dark canvas.
fn paint_swatch(layout: &TextLayout, swatch: &InlineSwatch, edge: Hsla, window: &mut Window) {
    let (Some(origin), Some(end)) = (
        layout.position_for_index(swatch.offset),
        layout.position_for_index(swatch.offset + '\u{2003}'.len_utf8()),
    ) else {
        return;
    };
    let slot = end.x - origin.x;
    if slot <= Pixels::ZERO {
        return;
    }
    let side = (slot * 0.78).min(layout.line_height() * 0.62);
    let inset = if swatch.alone {
        (slot - side) / 2.0
    } else {
        Pixels::ZERO
    };
    let bounds = Bounds::new(
        origin + gpui::point(inset, (layout.line_height() - side) / 2.0),
        gpui::size(side, side),
    );
    window.paint_quad(
        gpui::fill(bounds, swatch.color)
            .corner_radii(side * 0.22)
            .border_widths(gpui::px(1.0))
            .border_color(edge),
    );
}

/// Paints the selection behind the glyphs of `range`, one box per visual
/// row at full line height, the way Chrome highlights a text selection. A
/// selected line break adds a quarter-line sliver after the row.
fn paint_selection(layout: &TextLayout, range: &Range<usize>, color: Hsla, window: &mut Window) {
    let bounds = layout.bounds();
    let line_height = layout.line_height();
    let mut row_top = bounds.top();
    let mut line_start = 0usize;
    for line in layout.line_layouts() {
        let unwrapped = &line.unwrapped_layout;
        let line_end = line_start + line.len();
        let row_starts: Vec<usize> =
            std::iter::once(0)
                .chain(line.wrap_boundaries().iter().map(|boundary| {
                    unwrapped.runs[boundary.run_ix].glyphs[boundary.glyph_ix].index
                }))
                .collect();
        for (row, &row_start) in row_starts.iter().enumerate() {
            let row_end = row_starts.get(row + 1).copied().unwrap_or(line.len());
            let start = range.start.max(line_start + row_start);
            let end = range.end.min(line_start + row_end);
            let last_row = row + 1 == row_starts.len();
            let selects_break = last_row && range.start <= line_end && range.end > line_end;
            if start < end || selects_break {
                let origin_x = unwrapped.x_for_index(row_start);
                let left = unwrapped.x_for_index(start - line_start) - origin_x;
                let right = if start < end {
                    unwrapped.x_for_index(end - line_start) - origin_x
                } else {
                    left
                };
                let sliver = if selects_break {
                    line_height * 0.25
                } else {
                    Pixels::ZERO
                };
                #[expect(
                    clippy::cast_precision_loss,
                    reason = "a row index is far below f32's exact integer range"
                )]
                let top = row_top + line_height * row as f32;
                window.paint_quad(gpui::fill(
                    Bounds::new(
                        gpui::point(bounds.left() + left, top),
                        gpui::size(right - left + sliver, line_height),
                    ),
                    color,
                ));
            }
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "a line's visual row count is far below f32's exact integer range"
        )]
        let rows = row_starts.len() as f32;
        row_top += line_height * rows;
        line_start = line_end + 1;
    }
}

const PRESS: u8 = 0;
const MOVE: u8 = 1;
const RELEASE: u8 = 2;

/// Registers the leaf's mouse listeners: a capture-phase check-in and a
/// bubble-phase handler that processes the event once for every leaf.
fn register_pointer_listeners(live: &LiveLeaf, hitbox: &Hitbox, window: &mut Window) {
    let leaf = live.clone();
    let leaf_hitbox = hitbox.clone();
    window.on_mouse_event(
        move |event: &MouseDownEvent, phase, window: &mut Window, cx: &mut App| {
            if event.button != MouseButton::Left {
                return;
            }
            let stamp = (PRESS, event.position);
            let window_id = window.window_handle().window_id();
            match phase {
                DispatchPhase::Capture => {
                    let mut live = leaf.clone();
                    live.hovered = leaf_hitbox.is_hovered(window);
                    document(cx, window_id).check_in(stamp, live);
                }
                DispatchPhase::Bubble => {
                    if let Some(roster) = document(cx, window_id).take_roster(stamp) {
                        press(&roster, event, window, cx);
                    }
                }
            }
        },
    );

    let leaf = live.clone();
    let leaf_hitbox = hitbox.clone();
    window.on_mouse_event(
        move |event: &MouseMoveEvent, phase, window: &mut Window, cx: &mut App| {
            let window_id = window.window_handle().window_id();
            if !document(cx, window_id).is_dragging() {
                return;
            }
            let stamp = (MOVE, event.position);
            match phase {
                DispatchPhase::Capture => {
                    let mut live = leaf.clone();
                    live.hovered = leaf_hitbox.is_hovered(window);
                    document(cx, window_id).check_in(stamp, live);
                }
                DispatchPhase::Bubble => {
                    if let Some(roster) = document(cx, window_id).take_roster(stamp) {
                        if event.pressed_button == Some(MouseButton::Left) {
                            drag(&roster, event.position, window, cx);
                        } else {
                            // The release happened where no leaf listened.
                            release(&roster, event.position, window, cx);
                        }
                    }
                }
            }
        },
    );

    let leaf = live.clone();
    let leaf_hitbox = hitbox.clone();
    window.on_mouse_event(
        move |event: &MouseUpEvent, phase, window: &mut Window, cx: &mut App| {
            let window_id = window.window_handle().window_id();
            if event.button != MouseButton::Left || !document(cx, window_id).is_dragging() {
                return;
            }
            let stamp = (RELEASE, event.position);
            match phase {
                DispatchPhase::Capture => {
                    let mut live = leaf.clone();
                    live.hovered = leaf_hitbox.is_hovered(window);
                    document(cx, window_id).check_in(stamp, live);
                }
                DispatchPhase::Bubble => {
                    if let Some(roster) = document(cx, window_id).take_roster(stamp) {
                        release(&roster, event.position, window, cx);
                    }
                }
            }
        },
    );
}

/// A right-click over one of `ranges` calls `on_context` with its index.
fn register_context_listener(
    layout: TextLayout,
    ranges: Rc<[Range<usize>]>,
    on_context: SelectableContextHandler,
    hitbox: &Hitbox,
    window: &mut Window,
) {
    let hitbox = hitbox.clone();
    window.on_mouse_event(
        move |event: &MouseDownEvent, phase, window: &mut Window, cx: &mut App| {
            if phase != DispatchPhase::Bubble
                || event.button != MouseButton::Right
                || !hitbox.is_hovered(window)
            {
                return;
            }
            let Ok(index) = layout.index_for_position(event.position) else {
                return;
            };
            if let Some(range) = ranges.iter().position(|range| range.contains(&index)) {
                cx.stop_propagation();
                on_context(range, event.position, window, cx);
            }
        },
    );
}

/// `ctrl`+A selects this leaf, `ctrl`+C copies the window's selection.
fn register_key_listener(leaf: &LeafFrame, observed: Observed, window: &mut Window) {
    let leaf = leaf.clone();
    window.on_key_event(
        move |event: &KeyDownEvent, phase, window: &mut Window, cx: &mut App| {
            if phase != DispatchPhase::Bubble || !leaf.focus.is_focused(window) {
                return;
            }
            let key = event.keystroke.key.as_str();
            let modifiers = &event.keystroke.modifiers;
            let window_id = window.window_handle().window_id();
            if is_select_all_keystroke(key, modifiers) {
                let observed = observed.clone();
                change_selection(window_id, cx, |doc| doc.select_leaf(observed));
                window.prevent_default();
                cx.stop_propagation();
            } else if is_copy_keystroke(key, modifiers)
                && let Some(copied) = document(cx, window_id).selected_text()
            {
                cx.write_to_clipboard(ClipboardItem::new_string(
                    copied.replace(INLINE_IMAGE_PLACEHOLDER, ""),
                ));
                window.prevent_default();
                cx.stop_propagation();
            }
        },
    );
}

fn observed(roster: &[LiveLeaf]) -> Vec<Observed> {
    roster.iter().map(|live| live.observed.clone()).collect()
}

/// The glyph under `position`, as links and word units address text.
fn glyph_index(live: &LiveLeaf, position: Point<Pixels>) -> usize {
    let index = match live.layout.index_for_position(position) {
        Ok(index) | Err(index) => index,
    };
    clamp_to_char_boundary(&live.observed.text, index)
}

/// Applies `change` to the window's selection and repaints every view
/// whose leaves paint a different range afterwards.
fn change_selection(
    window_id: gpui::WindowId,
    cx: &mut App,
    change: impl FnOnce(&mut document::Document),
) {
    let doc = document(cx, window_id);
    let before = doc.painted();
    change(doc);
    let after = doc.painted();
    let mut views: Vec<EntityId> = Vec::new();
    let before_by_key: HashMap<_, _> = before
        .iter()
        .map(|(view, key, range)| (*key, (*view, range.clone())))
        .collect();
    let after_by_key: HashMap<_, _> = after
        .iter()
        .map(|(view, key, range)| (*key, (*view, range.clone())))
        .collect();
    for (key, (view, range)) in &before_by_key {
        if after_by_key.get(key).map(|(_, range)| range) != Some(range) {
            views.push(*view);
        }
    }
    for (key, (view, range)) in &after_by_key {
        if before_by_key.get(key).map(|(_, range)| range) != Some(range) {
            views.push(*view);
        }
    }
    views.sort_unstable();
    views.dedup();
    for view in views {
        cx.notify(view);
    }
}

/// A left press: start a selection in the leaf under the pointer, or clear
/// the selection when the press is outside any text.
fn press(roster: &[LiveLeaf], event: &MouseDownEvent, window: &mut Window, cx: &mut App) {
    let window_id = window.window_handle().window_id();
    let Some(hit) = roster.iter().find(|live| live.hovered) else {
        if document(cx, window_id).is_active() {
            change_selection(window_id, cx, document::Document::clear);
        }
        return;
    };
    let unit = SelectionUnit::from_click_count(event.click_count);
    let offset = match unit {
        SelectionUnit::Character => caret_index_for_position(&hit.layout, event.position),
        SelectionUnit::Word | SelectionUnit::Paragraph => glyph_index(hit, event.position),
    };
    let glyph = glyph_index(hit, event.position);
    let extend = event.modifiers.shift && document(cx, window_id).has_selection();
    let live = observed(roster);
    let key = hit.observed.key;
    change_selection(window_id, cx, |doc| {
        if extend {
            doc.observe(&live);
        } else {
            doc.reset_to(&live);
        }
        doc.press(key, offset, unit, extend);
        doc.set_pressed(key, glyph, event.click_count);
    });
    if let Some(focus) = hit.focus.as_ref() {
        window.focus(focus, cx);
        // Native `Div` convention (`div.rs` auto-focus): taking focus here
        // must suppress ancestor refocus during bubbling, or a focusable
        // parent such as the transcript `ScrollArea` steals it back.
        window.prevent_default();
    }
}

/// The leaf and caret offset a drag at `position` extends to: the leaf
/// whose rows span the pointer's height (nearest horizontally), else the
/// end of the nearest leaf above, else the start of the first leaf.
///
/// Returns the caret offset and the glyph under the pointer; word and
/// paragraph drags extend by the unit around the glyph.
fn drag_target(roster: &[LiveLeaf], position: Point<Pixels>) -> Option<(LeafKey, usize, usize)> {
    let distance = |live: &LiveLeaf| {
        let bounds = live.observed.bounds;
        if position.x < bounds.left() {
            bounds.left() - position.x
        } else if position.x > bounds.right() {
            position.x - bounds.right()
        } else {
            Pixels::ZERO
        }
    };
    let spanning = roster
        .iter()
        .filter(|live| {
            let bounds = live.observed.bounds;
            position.y >= bounds.top() && position.y < bounds.bottom()
        })
        .min_by(|left, right| f32::from(distance(left)).total_cmp(&f32::from(distance(right))));
    if let Some(live) = spanning {
        let caret = caret_index_for_position(&live.layout, position);
        return Some((live.observed.key, caret, glyph_index(live, position)));
    }
    let above = roster
        .iter()
        .filter(|live| live.observed.bounds.bottom() <= position.y)
        .max_by(|left, right| {
            f32::from(left.observed.bounds.bottom())
                .total_cmp(&f32::from(right.observed.bounds.bottom()))
        });
    if let Some(live) = above {
        let end = live.observed.text.len();
        return Some((live.observed.key, end, end));
    }
    roster.first().map(|live| (live.observed.key, 0, 0))
}

fn drag(roster: &[LiveLeaf], position: Point<Pixels>, window: &mut Window, cx: &mut App) {
    let window_id = window.window_handle().window_id();
    let live = observed(roster);
    let target = drag_target(roster, position);
    change_selection(window_id, cx, |doc| {
        doc.observe(&live);
        if let Some((key, caret, glyph)) = target {
            doc.drag_to(key, caret, glyph);
        }
    });
}

/// Ends the gesture. A gesture that left a selection consumes the release;
/// a clean single click inside one link range activates the link.
fn release(roster: &[LiveLeaf], position: Point<Pixels>, window: &mut Window, cx: &mut App) {
    let window_id = window.window_handle().window_id();
    drag(roster, position, window, cx);
    let pressed = document(cx, window_id).take_pressed();
    let mut selected = false;
    change_selection(window_id, cx, |doc| selected = doc.release());
    if selected {
        cx.stop_propagation();
        return;
    }
    let Some((key, down, 1)) = pressed else {
        return;
    };
    let Some(hit) = roster
        .iter()
        .find(|live| live.hovered && live.observed.key == key)
    else {
        return;
    };
    let up = glyph_index(hit, position);
    let Some(on_link) = hit.on_link.clone() else {
        return;
    };
    if let Some(index) = hit
        .links
        .iter()
        .position(|range| range.contains(&down) && range.contains(&up))
    {
        on_link(index, window, cx);
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Range;
    use std::rc::Rc;

    use gpui::{FontWeight, HighlightStyle, SharedString, TextStyle};

    use super::{RangeSlice, RetainedLeaf, RunInputs, compile_runs};

    fn bold() -> HighlightStyle {
        HighlightStyle {
            font_weight: Some(FontWeight::BOLD),
            ..Default::default()
        }
    }

    const BODY: &str = "a shared body long enough to live on the heap";

    #[test]
    fn unchanged_layout_reuses_its_compiled_runs() {
        let leaf = RetainedLeaf::default();
        let text = SharedString::from(BODY.to_owned());
        let style = TextStyle::default();
        let shared: Rc<[(Range<usize>, HighlightStyle)]> = Rc::from(vec![(2..8, bold())]);
        let compile = |text: &SharedString, highlights: RangeSlice<_>| {
            let mut highlights = highlights;
            let mut overrides = RangeSlice::default();
            leaf.compiled_runs(&mut RunInputs {
                text,
                default_style: &style,
                highlights: &mut highlights,
                overrides: &mut overrides,
            })
        };
        let first = compile(&text, RangeSlice::Shared(Rc::clone(&shared)));
        let second = compile(&text, RangeSlice::Shared(Rc::clone(&shared)));
        assert_eq!(first, second);
        assert_eq!(leaf.compilations.get(), 1);

        // Equal per-frame ranges and equal text in a fresh allocation reuse
        // the compiled runs too.
        let fresh_text = SharedString::from(BODY.to_owned());
        let reused = compile(&fresh_text, RangeSlice::Owned(vec![(2..8, bold())]));
        assert_eq!(reused, first);
        assert_eq!(leaf.compilations.get(), 1);

        // New bytes recompile, matching a direct compilation.
        let changed = SharedString::from(format!("{BODY}!"));
        let recompiled = compile(&changed, RangeSlice::Shared(Rc::clone(&shared)));
        assert_eq!(leaf.compilations.get(), 2);
        let mut highlights = RangeSlice::Shared(Rc::clone(&shared));
        let mut overrides = RangeSlice::default();
        let direct = compile_runs(&RunInputs {
            text: &changed,
            default_style: &style,
            highlights: &mut highlights,
            overrides: &mut overrides,
        });
        assert_eq!(recompiled, direct);
    }
}
