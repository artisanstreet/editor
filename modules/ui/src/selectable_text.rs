//! Reusable native selectable text for transcript bodies.
//!
//! Pinned GPUI ships hit-testing (`TextLayout::index_for_position`) and
//! platform clipboard writes but no selection concept (`docs/ui/`
//! `GPUI_CAPABILITIES.md` §2.11), while the transcript surface renders user
//! bodies as plain `Div` text and assistant bodies as inert `StyledText`.
//! This module closes that gap with one coherent primitive usable for both:
//! plain user body text and styled Markdown text/code blocks.
//!
//! Two ownership modes share one element and one behavior:
//!
//! - Controlled (`SelectableText::new`): the caller owns one
//!   [`SelectableTextState`] handle per text element — the same seam as
//!   [`crate::input_state::TextInputState`] — and hands it over on every
//!   render. Useful for tests and for callers that already retain per-view
//!   state.
//! - Retained (`SelectableText::retained`): the default for transcript
//!   rendering. Selection, drag latch, text fingerprint, and focus live in
//!   GPUI element state keyed by the caller's stable `ElementId`, so
//!   `MarkdownRenderer::render_source` stays synchronous and stateless
//!   across many messages with no caller-side per-block caches. Unpainted
//!   state is discarded by the framework, so scrolled-away messages release
//!   their selection without transcript-level bookkeeping.
//!
//! Selection is painted by overlaying only foreground/background onto the
//! caller ranges it intersects, so consumer weight, style, and underline
//! survive selection and glyph metrics never change while selecting. The
//! retained range is validated against a text fingerprint on every layout:
//! any content change — including same-length replacements — clears it, so
//! no stale byte range can survive a streaming update. While a drag is in
//! flight the normalized anchor/head range is live, so the wash tracks the
//! pointer before release.
//!
//! Deliberate limits: read-only selection only — no caret, no editing, no
//! IME/composition handling. Pointer capture is emulated the same way
//! GPUI's own interactive text does it: a left press latches the anchor and
//! every later move updates the head through the layout's nearest-index
//! mapping, even outside the hitbox, until left-button release ends the
//! drag. A drag that produces a selection consumes its release, and
//! [`SelectableTextState::suppresses_click`] stays true for the whole live
//! range, so selection gestures never fire links/actions.

#![allow(clippy::module_name_repetitions)]

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    App, AppContext as _, Bounds, ClipboardItem, CursorStyle, DispatchPhase, Element, ElementId,
    FocusHandle, GlobalElementId, HighlightStyle, Hitbox, HitboxBehavior, InspectorElementId,
    IntoElement, KeyDownEvent, LayoutId, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, SharedString, StyledText, Window,
};

use crate::theme::ArtisanTheme;

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

/// Overlays a selection wash onto one caller style.
///
/// Only foreground and background are replaced (wherever the wash defines
/// them); weight, font style, underline, strikethrough, and fade survive,
/// so selecting bold/italic/code text never changes glyph metrics.
#[must_use]
fn overlay_style(base: &HighlightStyle, wash: &HighlightStyle) -> HighlightStyle {
    HighlightStyle {
        color: wash.color.or(base.color),
        background_color: wash.background_color.or(base.background_color),
        ..*base
    }
}

/// Merges caller highlight ranges with the selection wash for [`StyledText`].
///
/// The result is sorted and non-overlapping, every range is clamped to a
/// character boundary inside `text`, and caller ranges keep their own
/// styles outside the selection. Inside the selection each intersecting
/// caller range keeps its style with only foreground/background overlaid,
/// and gaps with no caller range paint the pure wash. With no selection
/// the sanitized caller ranges pass through unchanged, so plain and
/// syntax-highlighted bodies render exactly as before.
///
/// `base` is expected sorted and non-overlapping, as produced by the
/// Markdown syntax seam; the merge is one ordered pass over it. Overlapping
/// input still resolves deterministically without panicking, but overlapping
/// coverage is not normalized — callers keep their ranges disjoint.
#[must_use]
pub fn merge_selection_highlight(
    text: &str,
    base: Vec<(Range<usize>, HighlightStyle)>,
    selection: Option<Range<usize>>,
    wash: &HighlightStyle,
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

    let mut merged = Vec::with_capacity(sanitized.len().saturating_add(2));
    let mut covered = selection.start;
    for (range, style) in &sanitized {
        if range.end <= selection.start || range.start >= selection.end {
            merged.push((range.clone(), *style));
            continue;
        }
        if range.start < selection.start {
            merged.push((range.start..selection.start, *style));
        }
        let segment_start = range.start.max(selection.start);
        if covered < segment_start {
            merged.push((covered..segment_start, *wash));
        }
        let segment_end = range.end.min(selection.end);
        merged.push((segment_start..segment_end, overlay_style(style, wash)));
        covered = segment_end;
        if range.end > selection.end {
            merged.push((selection.end..range.end, *style));
        }
    }
    if covered < selection.end {
        merged.push((covered..selection.end, *wash));
    }

    merged.sort_by_key(|left| left.0.start);
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

/// Fingerprints rendered text for selection validation.
///
/// Length plus a 64-bit hash. Collision policy: an accidental collision is
/// ~2^-64 per comparison, and adversarial collisions are not a threat model
/// for locally rendered transcript text; a match is treated as identical
/// content, any difference clears the selection.
fn text_fingerprint(text: &str) -> (usize, u64) {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    (text.len(), hasher.finish())
}

/// Per-element selection state shared between the caller and [`SelectableText`].
///
/// The handle is cheap to clone: every clone observes the same selection.
/// Controlled callers retain one handle for the lifetime of one text
/// element and hand it to the element on every render; retained mode keeps
/// one inside GPUI element state instead. Either way the range is validated
/// against the rendered text fingerprint on every layout and cleared the
/// moment the content changes, which bounds the selection lifetime without
/// a parallel transcript store.
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
    fingerprint: (usize, u64),
}

impl SelectableTextState {
    /// Creates empty selection state with no retained fingerprint.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Validates the retained range against freshly rendered text.
    ///
    /// When the fingerprint differs from the last validated render — an
    /// append, a correction, or even a same-length replacement — any
    /// selection and drag latch are dropped: new bytes must never keep a
    /// range that addressed older content.
    pub fn validate_for_text(&self, text: &str) {
        let next = text_fingerprint(text);
        let Some(mut inner) = self.inner.try_borrow_mut().ok() else {
            return;
        };
        if inner.fingerprint != next {
            inner.fingerprint = next;
            inner.selection = None;
            inner.anchor = None;
            inner.head = None;
            inner.down_index = None;
            inner.dragging = false;
        }
    }

    /// Returns the normalized selected byte range, if any.
    ///
    /// While a drag is in flight this is the live anchor/head range, so the
    /// wash tracks the pointer before release; otherwise it is the retained
    /// range. A press that has not moved yet keeps reporting the previous
    /// retained range.
    #[must_use]
    pub fn selection_range(&self) -> Option<Range<usize>> {
        let inner = self.inner.borrow();
        if inner.dragging
            && let (Some(anchor), Some(head)) = (inner.anchor, inner.head)
            && anchor != head
        {
            let (start, end) = if anchor < head {
                (anchor, head)
            } else {
                (head, anchor)
            };
            return Some(start..end);
        }
        inner.selection.map(|(start, end)| start..end)
    }

    /// Returns whether a non-empty selection is retained or live.
    #[must_use]
    pub fn has_selection(&self) -> bool {
        self.selection_range().is_some()
    }

    /// Returns whether a link/action activation must stand down.
    ///
    /// Consumers check this in their own click handlers: while a selection
    /// exists — including a live drag — a press-release inside a link range
    /// is a selection gesture, not an activation. The element also consumes
    /// the drag release itself as a best-effort guard; this predicate is the
    /// deterministic one.
    #[must_use]
    pub fn suppresses_click(&self) -> bool {
        self.selection_range().is_some()
    }

    /// Returns whether a drag latch is currently held.
    #[must_use]
    pub fn is_dragging(&self) -> bool {
        self.inner.borrow().dragging
    }

    /// Returns the selected slice of `text`, or an empty string.
    ///
    /// Reads the live-or-retained range (see [`Self::selection_range`]);
    /// out-of-date callers still get a safe empty slice rather than a
    /// panic.
    #[must_use]
    pub fn selected_text<'text>(&self, text: &'text str) -> &'text str {
        self.selection_range()
            .and_then(|range| text.get(range))
            .unwrap_or("")
    }

    /// Returns owned selected bytes for a clipboard write, if any.
    #[must_use]
    pub fn copy_text(&self, text: &str) -> Option<String> {
        let selected = self.selected_text(text);
        (!selected.is_empty()).then(|| selected.to_owned())
    }

    /// Latches a press at `index` as a potential drag.
    ///
    /// Callers pass indices already clamped with
    /// [`clamp_to_char_boundary`]; the element handlers always do.
    pub fn begin_drag(&self, index: usize) {
        if let Ok(mut inner) = self.inner.try_borrow_mut() {
            inner.anchor = Some(index);
            inner.head = Some(index);
            inner.down_index = Some(index);
            inner.dragging = true;
        }
    }

    /// Moves the drag head; returns whether the head actually changed.
    #[must_use]
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
    /// A drag that moved finalizes the live range into the retained
    /// selection; a press without movement collapses (clears) it, matching
    /// native click-clears-selection behavior. A release with no latched
    /// press leaves the retained selection untouched. Returns whether a
    /// non-empty selection is retained afterward.
    #[must_use]
    pub fn end_drag(&self, text: &str) -> bool {
        if let Ok(mut inner) = self.inner.try_borrow_mut() {
            inner.dragging = false;
            if let Some((anchor, head)) = inner.anchor.zip(inner.head) {
                inner.selection =
                    normalize_selection(anchor, head, text).map(|range| (range.start, range.end));
            }
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
            let len = inner.fingerprint.0;
            inner.selection = (len > 0).then_some((0, len));
            inner.anchor = None;
            inner.head = None;
            inner.down_index = None;
            inner.dragging = false;
        }
    }
}

/// Activation for one link range inside selectable text.
pub type SelectableLinkHandler = Rc<dyn Fn(usize, &mut Window, &mut App)>;

/// Which ownership mode an element uses.
enum SelectionSource {
    /// Caller-retained handle plus optional caller focus handle.
    Controlled {
        state: SelectableTextState,
        focus: Option<FocusHandle>,
    },
    /// Framework-retained state keyed by the element id.
    Retained,
}

/// Framework-retained selection for [`SelectionSource::Retained`].
///
/// Stored through `Window::with_element_state` under the element's stable
/// id, so it persists across frames exactly while the element keeps
/// painting and is discarded when it stops — no caller cache, no transcript
/// authority.
#[derive(Clone, Default)]
struct RetainedSelection {
    state: SelectableTextState,
    focus: Option<FocusHandle>,
}

/// Per-frame paint inputs resolved during `request_layout`.
#[derive(Clone)]
struct PaintFrame {
    state: SelectableTextState,
    focus: Option<FocusHandle>,
    text: SharedString,
    links: Vec<Range<usize>>,
    on_link: Option<SelectableLinkHandler>,
    /// Whether to attach the focus handle to the dispatch tree in
    /// `prepaint`. True for retained mode, which owns its handle, and for
    /// controlled mode when the caller supplied one through `.focus()`.
    register_focus: bool,
}

/// Merged highlight runs, the retained selection range, and the prepared
/// paint frame resolved from element state.
type RetainedFramePaint = (
    Vec<(Range<usize>, HighlightStyle)>,
    Option<Range<usize>>,
    PaintFrame,
);

/// Read-only selectable text element for transcript bodies.
///
/// Construct one per render from the owning view, exactly like the shared
/// input surfaces: pointer drag, focused keyboard copy/select-all, and
/// link-range activation that stands down during selection are all owned
/// here, so plain user bodies and styled Markdown/code blocks share one
/// behavior.
pub struct SelectableText {
    id: ElementId,
    text: SharedString,
    base_highlights: Vec<(Range<usize>, HighlightStyle)>,
    text_run_overrides: Vec<TextRunOverride>,
    selection_style: HighlightStyle,
    source: SelectionSource,
    link_ranges: Vec<Range<usize>>,
    on_link: Option<SelectableLinkHandler>,
    snapshot: Option<Range<usize>>,
    styled: Option<StyledText>,
    frame: Option<PaintFrame>,
}

impl SelectableText {
    /// Constructs controlled selectable text for one render frame.
    ///
    /// `base_highlights` are caller ranges already addressing `text` (for
    /// example inline-code or syntax ranges); the retained selection is
    /// validated against `text` on every layout and painted over them with
    /// the theme `::selection` wash.
    #[must_use]
    pub fn new(
        id: impl Into<ElementId>,
        text: impl Into<SharedString>,
        state: &SelectableTextState,
        theme: ArtisanTheme,
        base_highlights: Vec<(Range<usize>, HighlightStyle)>,
    ) -> Self {
        Self::build(
            id,
            text,
            &theme,
            base_highlights,
            SelectionSource::Controlled {
                state: state.clone(),
                focus: None,
            },
        )
    }

    /// Constructs retained selectable text for one render frame.
    ///
    /// The default for transcript rendering: no caller state, no caller
    /// focus handle. Selection, drag latch, text fingerprint, and focus
    /// persist in framework element state under `id` across frames and are
    /// released when the element stops painting. Ids must be unique per
    /// text element, like all GPUI element ids; without a view-backed
    /// identity the element degrades to ephemeral per-frame state.
    #[must_use]
    pub fn retained(
        id: impl Into<ElementId>,
        text: impl Into<SharedString>,
        theme: ArtisanTheme,
        base_highlights: Vec<(Range<usize>, HighlightStyle)>,
    ) -> Self {
        Self::build(id, text, &theme, base_highlights, SelectionSource::Retained)
    }

    fn build(
        id: impl Into<ElementId>,
        text: impl Into<SharedString>,
        theme: &ArtisanTheme,
        base_highlights: Vec<(Range<usize>, HighlightStyle)>,
        source: SelectionSource,
    ) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            base_highlights,
            text_run_overrides: Vec::new(),
            selection_style: selection_style_for_theme(*theme),
            source,
            link_ranges: Vec::new(),
            on_link: None,
            snapshot: None,
            styled: None,
            frame: None,
        }
    }

    /// Supplies the focus handle used for keyboard copy/select-all gating.
    ///
    /// Controlled mode only: a press inside the text focuses this handle,
    /// so a later `control/command-c` copies without extra wiring. Without
    /// a handle, pointer selection still works but keyboard shortcuts stay
    /// disabled. Retained mode owns its handle and ignores this.
    #[must_use]
    pub fn focus(mut self, focus: FocusHandle) -> Self {
        if let SelectionSource::Controlled { focus: slot, .. } = &mut self.source {
            *slot = Some(focus);
        }
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

    /// Supplies per-range font family and letter-spacing overrides.
    ///
    /// Code ranges pass their mono family with `Some(px(0.0))` spacing
    /// while code weight travels in the highlight ranges; the selection
    /// wash still only touches foreground/background, so selecting never
    /// changes shaping. Overrides are validated fail-closed at layout
    /// time (see [`compile_text_runs`]).
    #[must_use]
    pub fn with_text_run_overrides(mut self, overrides: Vec<TextRunOverride>) -> Self {
        self.text_run_overrides = overrides;
        self
    }

    /// Returns the rendered text.
    #[must_use]
    pub fn text(&self) -> &str {
        self.text.as_ref()
    }

    /// Returns the selection snapshot painted this frame.
    #[must_use]
    pub fn selection(&self) -> Option<Range<usize>> {
        self.snapshot.clone()
    }

    /// Returns whether this frame paints a selection wash.
    #[must_use]
    pub fn shows_selection(&self) -> bool {
        self.snapshot.is_some()
    }

    /// Resolves the paint frame for retained mode from element state.
    fn retained_frame(
        &self,
        global_id: Option<&GlobalElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> RetainedFramePaint {
        let (merged, snapshot, frame, needs_focus) = window.with_optional_element_state(
            global_id,
            |stored: Option<Option<RetainedSelection>>, _: &mut Window| {
                let Some(inner) = stored else {
                    let state = SelectableTextState::default();
                    state.validate_for_text(self.text.as_ref());
                    let merged = merge_selection_highlight(
                        self.text.as_ref(),
                        self.base_highlights.clone(),
                        state.selection_range(),
                        &self.selection_style,
                    );
                    let frame = PaintFrame {
                        state,
                        focus: None,
                        text: self.text.clone(),
                        links: self.link_ranges.clone(),
                        on_link: self.on_link.clone(),
                        register_focus: false,
                    };
                    return ((merged, None, frame, false), None);
                };
                let retained: RetainedSelection = inner.unwrap_or_default();
                retained.state.validate_for_text(self.text.as_ref());
                let snapshot = retained.state.selection_range();
                let needs_focus = retained.focus.is_none();
                let merged = merge_selection_highlight(
                    self.text.as_ref(),
                    self.base_highlights.clone(),
                    snapshot.clone(),
                    &self.selection_style,
                );
                let frame = PaintFrame {
                    state: retained.state.clone(),
                    focus: retained.focus.clone(),
                    text: self.text.clone(),
                    links: self.link_ranges.clone(),
                    on_link: self.on_link.clone(),
                    register_focus: true,
                };
                ((merged, snapshot, frame, needs_focus), Some(retained))
            },
        );
        let frame = if needs_focus {
            let focus = cx.focus_handle();
            window.with_optional_element_state(
                global_id,
                |stored: Option<Option<RetainedSelection>>, _: &mut Window| {
                    let Some(inner) = stored else {
                        return ((), None);
                    };
                    let mut retained = inner.unwrap_or_default();
                    retained.focus = Some(focus.clone());
                    ((), Some(retained))
                },
            );
            PaintFrame {
                focus: Some(focus),
                ..frame
            }
        } else {
            frame
        };
        (merged, snapshot, frame)
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
        let (merged, snapshot, frame) = match &self.source {
            SelectionSource::Controlled { state, focus } => {
                state.validate_for_text(self.text.as_ref());
                let merged = merge_selection_highlight(
                    self.text.as_ref(),
                    self.base_highlights.clone(),
                    state.selection_range(),
                    &self.selection_style,
                );
                let frame = PaintFrame {
                    state: state.clone(),
                    focus: focus.clone(),
                    text: self.text.clone(),
                    links: self.link_ranges.clone(),
                    on_link: self.on_link.clone(),
                    register_focus: focus.is_some(),
                };
                (merged, state.selection_range(), frame)
            }
            SelectionSource::Retained => self.retained_frame(global_id, window, cx),
        };
        self.snapshot = snapshot;
        self.frame = Some(frame);
        let default_style = window.text_style();
        let runs = compile_text_runs(
            self.text.as_ref(),
            &default_style,
            &merged,
            &self.text_run_overrides,
        );
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
                &self.base_highlights,
                &self.text_run_overrides,
            );
            let mut styled = StyledText::new(self.text.clone()).with_runs(runs);
            let _ = styled.request_layout(None, inspector_id, window, cx);
            self.styled = Some(styled);
        }
        if let Some(frame) = self.frame.as_ref()
            && frame.register_focus
            && let Some(focus) = frame.focus.as_ref()
        {
            window.set_focus_handle(focus, cx);
        }
        if let Some(styled) = self.styled.as_mut() {
            styled.prepaint(None, inspector_id, bounds, state, window, cx);
        }
        window.insert_hitbox(bounds, HitboxBehavior::Normal)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "paint keeps retained/controlled selection, hover, and link handling in one \
                  sequential pass sharing one frame snapshot; extraction would thread frame locals"
    )]
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
        let Some(frame) = self.frame.clone() else {
            if let Some(styled) = self.styled.as_mut() {
                styled.paint(None, inspector_id, bounds, &mut (), &mut (), window, cx);
            }
            return;
        };
        let Some(layout) = self.styled.as_ref().map(|styled| styled.layout().clone()) else {
            return;
        };
        let current_view = window.current_view();
        let hitbox_snapshot = hitbox.clone();

        if hitbox_snapshot.is_hovered(window) {
            let hovered = layout.index_for_position(window.mouse_position()).ok();
            let over_link =
                hovered.is_some_and(|index| frame.links.iter().any(|range| range.contains(&index)));
            window.set_cursor_style(
                if over_link {
                    CursorStyle::PointingHand
                } else {
                    CursorStyle::IBeam
                },
                &hitbox_snapshot,
            );
        }

        let state = frame.state.clone();
        let focus = frame.focus.clone();
        let down_layout = layout.clone();
        let down_text = frame.text.clone();
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
                    // Native `Div` convention (`div.rs` auto-focus): taking
                    // focus here must suppress ancestor refocus during
                    // bubbling, or a focusable parent such as the transcript
                    // `ScrollArea` steals it back. No `stop_propagation` —
                    // other listeners still run.
                    window.prevent_default();
                }
                window.refresh();
                cx.notify(current_view);
            },
        );

        let state = frame.state.clone();
        let move_layout = layout.clone();
        let move_text = frame.text.clone();
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

        let state = frame.state.clone();
        let up_layout = layout.clone();
        let up_text = frame.text.clone();
        let link_ranges = frame.links.clone();
        let on_link = frame.on_link.clone();
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
                let _ = state.update_drag(index);
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

        if let Some(focus) = frame.focus.clone() {
            let state = frame.state.clone();
            let key_text = frame.text.clone();
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
                    } else if is_copy_keystroke(key, modifiers)
                        && let Some(copied) = state.copy_text(&key_text)
                    {
                        cx.write_to_clipboard(ClipboardItem::new_string(copied));
                        window.prevent_default();
                        cx.stop_propagation();
                    }
                },
            );
        }

        if let Some(styled) = self.styled.as_mut() {
            if self.snapshot.is_some() {
                window.with_text_color_map(None, |window| {
                    styled.paint(None, inspector_id, bounds, &mut (), &mut (), window, cx);
                });
            } else {
                styled.paint(None, inspector_id, bounds, &mut (), &mut (), window, cx);
            }
        }
    }
}

impl IntoElement for SelectableText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}
