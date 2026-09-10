//! Reusable native selectable text for transcript bodies.
//!
//! Pinned GPUI ships hit-testing (`TextLayout::index_for_position`) and
//! platform clipboard writes but no selection concept (`docs/ui/
//! GPUI_CAPABILITIES.md` §2.11), while the transcript surface renders user
//! bodies as plain `Div` text and assistant bodies as inert `StyledText`.
//! This module closes that gap with one coherent primitive usable for both:
//! plain user body text and styled Markdown text/code blocks.
//!
//! The design mirrors two proven in-repo seams:
//!
//! - `InteractiveText` mechanics from pinned GPUI's own `elements/text.rs`:
//!   a custom [`Element`] owns one [`StyledText`] layout and resolves pointer
//!   positions to byte indices through that layout during paint. Selection is painted as one
//!   delayed background highlight merged after caller ranges, so inherited
//!   typography, wraps, and syntax colors are preserved underneath the
//!   theme `::selection` wash.
//! - Controlled state like [`crate::input_state::TextInputState`]: the
//!   caller owns one [`SelectableTextState`] handle per text element and the
//!   element never duplicates transcript text. Selection lives exactly as
//!   long as the owning element's handle, is validated against the rendered
//!   text on every construction, and is cleared the moment the text changes,
//!   so no stale byte range can survive a streaming update.
//!
//! Deliberate limits: read-only selection only — no caret, no editing, no
//! IME/composition handling. Pointer capture is emulated the same way
//! GPUI's own interactive text does it: a left press latches the anchor and
//! every later move updates the head through the layout's nearest-index
//! mapping, even outside the hitbox, until left-button release ends the
//! drag. A drag that produces a selection consumes its release so an outer
//! link/action handler does not fire; consumers additionally guard link
//! activation with [`SelectableTextState::suppresses_click`].

#![allow(clippy::module_name_repetitions)]

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    App, Bounds, ClipboardItem, CursorStyle, DispatchPhase, Element, ElementId, FocusHandle,
    GlobalElementId, HighlightStyle, Hitbox, HitboxBehavior, InspectorElementId, IntoElement,
    KeyDownEvent, LayoutId, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, SharedString, StyledText, Window,
};

use crate::theme::ArtisanTheme;

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

/// Normalizes two raw byte indices into an ordered selection range.
///
/// Both endpoints are clamped with [`clamp_to_char_boundary`]. Returns
/// `None` when the result is collapsed, so callers never retain an empty
/// selection.
#[must_use]
pub fn normalize_selection(anchor: usize, head: usize, text: &str) -> Option<Range<usize>> {
    let first = clamp_to_char_boundary(text, anchor);
    let second = clamp_to_char_boundary(text, head);
    let (start, end) = if first <= second {
        (first, second)
    } else {
        (second, first)
    };
    (start < end).then_some(start..end)
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

/// Merges caller highlight ranges with the selection wash for [`StyledText`].
///
/// The result is sorted and non-overlapping, every range is clamped to a
/// character boundary inside `text`, and the selection range wins wherever
/// it overlaps a caller range (caller ranges are split around it). With no
/// selection the sanitized caller ranges pass through unchanged, so plain
/// and syntax-highlighted bodies render exactly as before.
#[must_use]
pub fn merge_selection_highlight(
    text: &str,
    base: Vec<(Range<usize>, HighlightStyle)>,
    selection: Option<Range<usize>>,
    selection_style: &HighlightStyle,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut sanitized: Vec<(Range<usize>, HighlightStyle)> = base
        .into_iter()
        .filter_map(|(range, style)| {
            let start = clamp_to_char_boundary(text, range.start);
            let end = clamp_to_char_boundary(text, range.end);
            (start < end).then_some((start..end, style))
        })
        .collect();
    sanitized.sort_by(|left, right| {
        left.0
            .start
            .cmp(&right.0.start)
            .then_with(|| left.0.end.cmp(&right.0.end))
    });

    let Some(selection) = selection else {
        return sanitized;
    };
    let start = clamp_to_char_boundary(text, selection.start);
    let end = clamp_to_char_boundary(text, selection.end);
    let Some(selection) = (start < end).then_some(start..end) else {
        return sanitized;
    };

    let mut merged = Vec::with_capacity(sanitized.len().saturating_add(1));
    for (range, style) in sanitized {
        if range.end <= selection.start || range.start >= selection.end {
            merged.push((range, style));
            continue;
        }
        if range.start < selection.start {
            merged.push((range.start..selection.start, style));
        }
        if range.end > selection.end {
            merged.push((selection.end..range.end, style));
        }
    }
    merged.push((selection, *selection_style));
    merged.sort_by(|left, right| left.0.start.cmp(&right.0.start));
    merged
}

/// Resolves the theme `::selection` wash for selectable text.
///
/// Background and foreground come from the audited interaction tokens
/// (`oklch(0.48 0.13 250 / 42%)` over `--foreground`), matching the legacy
/// `::selection` treatment recorded in `INVENTORY` §2.
#[must_use]
pub fn selection_style_for_theme(theme: ArtisanTheme) -> HighlightStyle {
    HighlightStyle {
        background_color: Some(theme.interaction.selection_background.to_paint()),
        color: Some(theme.interaction.selection_foreground.to_paint()),
        ..Default::default()
    }
}

/// Per-element selection state shared between the caller and [`SelectableText`].
///
/// The handle is cheap to clone: every clone observes the same selection.
/// The caller retains one handle for the lifetime of one text element (for
/// example beside one transcript message view) and hands it to the element
/// on every render. [`SelectableText::new`] validates the retained range
/// against the rendered text and clears it when the text changed, which
/// bounds the selection lifetime without a parallel transcript store.
#[derive(Clone, Debug, Default)]
pub struct SelectableTextState {
    inner: Rc<RefCell<SelectionInner>>,
}

#[derive(Clone, Debug, Default)]
struct SelectionInner {
    anchor: Option<usize>,
    head: Option<usize>,
    down_index: Option<usize>,
    dragging: bool,
    selection: Option<(usize, usize)>,
    text_len: usize,
}

impl SelectableTextState {
    /// Creates empty selection state with no retained text length.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Validates the retained range against freshly rendered text.
    ///
    /// When `text` has a different length than the last validated render,
    /// any selection and drag latch are dropped: streaming appends and
    /// corrections must never keep a range that addressed older bytes.
    pub fn validate_for_text(&self, text: &str) {
        let Some(mut inner) = self.inner.try_borrow_mut().ok() else {
            return;
        };
        if inner.text_len != text.len() {
            inner.text_len = text.len();
            inner.selection = None;
            inner.anchor = None;
            inner.head = None;
            inner.down_index = None;
            inner.dragging = false;
        }
    }

    /// Returns the normalized selected byte range, if any.
    #[must_use]
    pub fn selection_range(&self) -> Option<Range<usize>> {
        self.inner
            .borrow()
            .selection
            .map(|(start, end)| start..end)
    }

    /// Returns whether a non-empty selection is retained.
    #[must_use]
    pub fn has_selection(&self) -> bool {
        self.inner.borrow().selection.is_some()
    }

    /// Returns whether a link/action activation must stand down.
    ///
    /// Consumers check this in their own click handlers: while a selection
    /// exists, a press-release inside a link range is a selection gesture,
    /// not an activation. The element also consumes the drag release itself
    /// as a best-effort guard; this predicate is the deterministic one.
    #[must_use]
    pub fn suppresses_click(&self) -> bool {
        self.has_selection()
    }

    /// Returns whether a drag latch is currently held.
    #[must_use]
    pub fn is_dragging(&self) -> bool {
        self.inner.borrow().dragging
    }

    /// Returns the selected slice of `text`, or an empty string.
    ///
    /// The retained range always addresses the validated text (see
    /// [`Self::validate_for_text`}); out-of-date callers still get a safe
    /// empty slice rather than a panic.
    #[must_use]
    pub fn selected_text<'text>(&self, text: &'text str) -> &'text str {
        let selection = self.inner.borrow().selection;
        selection
            .and_then(|(start, end)| text.get(start..end))
            .unwrap_or("")
    }

    /// Returns owned selected bytes for a clipboard write, if any.
    #[must_use]
    pub fn copy_text(&self, text: &str) -> Option<String> {
        let selected = self.selected_text(text);
        (!selected.is_empty()).then(|| selected.to_owned())
    }

    /// Latches a press at `index` as a potential drag.
    pub fn begin_drag(&self, index: usize) {
        if let Ok(mut inner) = self.inner.try_borrow_mut() {
            inner.anchor = Some(index);
            inner.head = Some(index);
            inner.down_index = Some(index);
            inner.dragging = true;
        }
    }

    /// Moves the drag head; returns whether the head actually changed.
    pub fn update_drag(&self, index: usize) -> bool {
        if let Ok(mut inner) = self.inner.try_borrow_mut() {
            if !inner.dragging {
                return false;
            }
            if inner.head == Some(index) {
                return false;
            }
            inner.head = Some(index);
            return true;
        }
        false
    }

    /// Releases the drag latch.
    ///
    /// A drag that moved resolves to the normalized range between anchor
    /// and head; a press without movement collapses (clears) the selection,
    /// matching native click-clears-selection behavior. Returns whether a
    /// non-empty selection is retained afterward.
    pub fn end_drag(&self, text: &str) -> bool {
        if let Ok(mut inner) = self.inner.try_borrow_mut() {
            inner.dragging = false;
            let next = inner
                .anchor
                .zip(inner.head)
                .and_then(|(anchor, head)| normalize_selection(anchor, head, text))
                .map(|range| (range.start, range.end));
            inner.selection = next;
            inner.anchor = None;
            inner.head = None;
            return inner.selection.is_some();
        }
        self.has_selection()
    }

    /// Returns the press index latched by [`Self::begin_drag`], if any.
    #[must_use]
    pub fn down_index(&self) -> Option<usize> {
        self.inner.borrow().down_index
    }

    /// Clears the press latch without touching the selection.
    pub fn clear_press(&self) {
        if let Ok(mut inner) = self.inner.try_borrow_mut() {
            inner.down_index = None;
            inner.dragging = false;
            inner.anchor = None;
            inner.head = None;
        }
    }

    /// Clears any retained selection and drag latch.
    pub fn clear_selection(&self) {
        if let Ok(mut inner) = self.inner.try_borrow_mut() {
            inner.selection = None;
            inner.anchor = None;
            inner.head = None;
            inner.down_index = None;
            inner.dragging = false;
        }
    }

    /// Selects the whole validated text; a no-op on empty text.
    pub fn select_all(&self) {
        if let Ok(mut inner) = self.inner.try_borrow_mut() {
            inner.selection = (inner.text_len > 0).then_some((0, inner.text_len));
            inner.anchor = None;
            inner.head = None;
            inner.down_index = None;
            inner.dragging = false;
        }
    }
}

/// Activation for one link range inside selectable text.
pub type SelectableLinkHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// Read-only selectable text element for transcript bodies.
///
/// Construct one per render from the owning view, exactly like the shared
/// input surfaces: the text and caller highlight ranges are snapshotted for
/// this frame while [`SelectableTextState`] carries the selection across
/// frames. Pointer drag, focused keyboard copy/select-all, and link-range
/// activation that stands down during selection are all owned here, so
/// plain user bodies and styled Markdown/code blocks share one behavior.
pub struct SelectableText {
    id: ElementId,
    text: StyledText,
    text_string: SharedString,
    selection_snapshot: Option<Range<usize>>,
    state: SelectableTextState,
    focus: Option<FocusHandle>,
    link_ranges: Vec<Range<usize>>,
    on_link: Option<SelectableLinkHandler>,
}

impl SelectableText {
    /// Constructs selectable text for one render frame.
    ///
    /// `base_highlights` are caller ranges already addressing `text` (for
    /// example inline-code or syntax ranges); the retained selection is
    /// validated against `text` first and painted over them with the theme
    /// `::selection` wash.
    #[must_use]
    pub fn new(
        id: impl Into<ElementId>,
        text: impl Into<SharedString>,
        state: &SelectableTextState,
        theme: ArtisanTheme,
        base_highlights: Vec<(Range<usize>, HighlightStyle)>,
    ) -> Self {
        let text = text.into();
        state.validate_for_text(text.as_ref());
        let selection_snapshot = state.selection_range();
        let merged = merge_selection_highlight(
            text.as_ref(),
            base_highlights,
            selection_snapshot.clone(),
            &selection_style_for_theme(theme),
        );
        Self {
            id: id.into(),
            text: StyledText::new(text.clone()).with_highlights(merged),
            text_string: text,
            selection_snapshot,
            state: state.clone(),
            focus: None,
            link_ranges: Vec::new(),
            on_link: None,
        }
    }

    /// Supplies the focus handle used for keyboard copy/select-all gating.
    ///
    /// A press inside the text focuses this handle, so a later
    /// `control/command-c` copies without extra wiring. Without a handle,
    /// pointer selection still works but keyboard shortcuts stay disabled.
    #[must_use]
    pub fn focus(mut self, focus: FocusHandle) -> Self {
        self.focus = Some(focus);
        self
    }

    /// Supplies link ranges plus their activation handler.
    ///
    /// Activation fires only when press and release land in the same range
    /// and the gesture produced no selection, so drag-selecting across a
    /// link never follows it. Consumers must additionally consult
    /// [`SelectableTextState::suppresses_click`] in overlapping handlers.
    #[must_use]
    pub fn links(
        mut self,
        ranges: Vec<Range<usize>>,
        on_link: impl Fn(usize, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.link_ranges = ranges;
        self.on_link = Some(Rc::new(on_link));
        self
    }

    /// Returns the rendered text.
    #[must_use]
    pub fn text(&self) -> &str {
        self.text_string.as_ref()
    }

    /// Returns the selection snapshot painted this frame.
    #[must_use]
    pub fn selection(&self) -> Option<Range<usize>> {
        self.selection_snapshot.clone()
    }

    /// Returns whether this frame paints a selection wash.
    #[must_use]
    pub fn shows_selection(&self) -> bool {
        self.selection_snapshot.is_some()
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
        _id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.text.request_layout(None, inspector_id, window, cx)
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
        self.text.prepaint(None, inspector_id, bounds, state, window, cx);
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
        let current_view = window.current_view();
        let layout = self.text.layout().clone();
        let hitbox_snapshot = hitbox.clone();

        if hitbox_snapshot.is_hovered(window) {
            let hovered = layout.index_for_position(window.mouse_position()).ok();
            let over_link = hovered.is_some_and(|index| {
                self.link_ranges.iter().any(|range| range.contains(&index))
            });
            window.set_cursor_style(
                if over_link {
                    CursorStyle::PointingHand
                } else {
                    CursorStyle::IBeam
                },
                &hitbox_snapshot,
            );
        }

        let state = self.state.clone();
        let focus = self.focus.clone();
        let down_layout = layout.clone();
        let down_text = self.text_string.clone();
        window.on_mouse_event(
            move |event: &MouseDownEvent, phase, window: &mut Window, cx: &mut App| {
                if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                    return;
                }
                if !hitbox_snapshot.is_hovered(window) {
                    return;
                }
                let index = clamp_to_char_boundary(
                    &down_text,
                    match down_layout.index_for_position(event.position) {
                        Ok(exact) | Err(exact) => exact,
                    },
                );
                state.begin_drag(index);
                if let Some(focus) = focus.as_ref() {
                    window.focus(focus, cx);
                }
                window.refresh();
                cx.notify(current_view);
            },
        );

        let state = self.state.clone();
        let move_layout = layout.clone();
        let move_text = self.text_string.clone();
        window.on_mouse_event(
            move |event: &MouseMoveEvent, phase, window: &mut Window, cx: &mut App| {
                if phase != DispatchPhase::Bubble || !state.is_dragging() {
                    return;
                }
                let index = clamp_to_char_boundary(
                    &move_text,
                    match move_layout.index_for_position(event.position) {
                        Ok(exact) | Err(exact) => exact,
                    },
                );
                if state.update_drag(index) {
                    window.refresh();
                    cx.notify(current_view);
                }
            },
        );

        let state = self.state.clone();
        let up_layout = layout.clone();
        let up_text = self.text_string.clone();
        let link_ranges = self.link_ranges.clone();
        let on_link = self.on_link.clone();
        window.on_mouse_event(
            move |event: &MouseUpEvent, phase, window: &mut Window, cx: &mut App| {
                if phase != DispatchPhase::Bubble || event.button != MouseButton::Left {
                    return;
                }
                if !state.is_dragging() {
                    return;
                }
                let down = state.down_index();
                let index = clamp_to_char_boundary(
                    &up_text,
                    match up_layout.index_for_position(event.position) {
                        Ok(exact) | Err(exact) => exact,
                    },
                );
                state.update_drag(index);
                if state.end_drag(&up_text) {
                    state.clear_press();
                    cx.stop_propagation();
                    window.refresh();
                    cx.notify(current_view);
                    return;
                }
                state.clear_press();
                if let (Some(down), Some(on_link)) = (down, on_link.as_ref()) {
                    for (range_index, range) in link_ranges.iter().enumerate() {
                        if range.contains(&down) && range.contains(&index) {
                            on_link(range_index, window, cx);
                            break;
                        }
                    }
                }
                window.refresh();
                cx.notify(current_view);
            },
        );

        if let Some(focus) = self.focus.clone() {
            let state = self.state.clone();
            let key_text = self.text_string.clone();
            window.on_key_event(
                move |event: &KeyDownEvent, phase, window: &mut Window, cx: &mut App| {
                    if phase != DispatchPhase::Bubble || !focus.is_focused(window) {
                        return;
                    }
                    let key = event.keystroke.key.as_str();
                    let modifiers = &event.keystroke.modifiers;
                    if is_select_all_keystroke(key, modifiers) {
                        state.select_all();
                        window.prevent_default();
                        cx.stop_propagation();
                        window.refresh();
                        cx.notify(current_view);
                    } else if is_copy_keystroke(key, modifiers) {
                        if let Some(copied) = state.copy_text(&key_text) {
                            cx.write_to_clipboard(ClipboardItem::new_string(copied));
                            window.prevent_default();
                            cx.stop_propagation();
                        }
                    }
                },
            );
        }

        self.text
            .paint(None, inspector_id, bounds, &mut (), &mut (), window, cx);
    }
}

impl IntoElement for SelectableText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}
