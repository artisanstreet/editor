//! Reusable native selectable text for transcript bodies.
//!
//! Transcript bodies are separate text leaves sharing a bounded selection document.
//! Source ordinals determine copy order; rendering supplies hit geometry:
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
//! stable `ElementId`. Compiled runs are reused while the text,
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
use std::sync::Arc;

use gpui::{
    App, AppContext as _, Bounds, ClipboardItem, CursorStyle, DispatchPhase, Element, ElementId,
    EntityId, FocusHandle, GlobalElementId, HighlightStyle, Hitbox, HitboxBehavior, Hsla,
    InspectorElementId, IntoElement, KeyDownEvent, LayoutId, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, SharedString, StyledText,
    TextLayout, TextRun, TextStyle, Window,
};

use crate::text_navigation::SelectionUnit;

mod geometry;
mod native_geometry;
use crate::theme::ArtisanTheme;
use geometry::LeafLayout;

use document::{LeafKey, LiveLeaf, Observed, document};

pub use crate::text_runs::{TextRunOverride, compile_text_runs};
pub use native_geometry::MAX_NATIVE_SOURCE;

fn same_allocation(left: &SharedString, right: &SharedString) -> bool {
    left.as_ptr() == right.as_ptr() && left.len() == right.len()
}

/// Plain source text is owned by its renderer, independently of painted leaves.
pub struct SelectionLeaf {
    pub order: Vec<u64>,
    pub text: SharedString,
    pub separator: &'static str,
    pub copies: Rc<[(Range<usize>, SharedString)]>,
}
pub struct SelectionSource {
    scope: SharedString,
    owner: Rc<[u64]>,
    leaves: Vec<SelectionLeaf>,
}
impl SelectionSource {
    /// Conservative retained allocation cost for the renderer's source cache.
    /// Shared strings/substitutions are charged in full here, even if another
    /// prepared document also owns them. Vec spare capacity and Rc metadata count.
    pub fn resident_bytes(&self) -> usize {
        const RC_HEADER: usize = 2 * std::mem::size_of::<usize>();
        let base = std::mem::size_of::<Self>()
            .saturating_add(RC_HEADER)
            .saturating_add(self.scope.len())
            .saturating_add(RC_HEADER)
            .saturating_add(std::mem::size_of_val(self.owner.as_ref()))
            .saturating_add(RC_HEADER)
            .saturating_add(
                self.leaves
                    .capacity()
                    .saturating_mul(std::mem::size_of::<SelectionLeaf>()),
            );
        self.leaves.iter().fold(base, |bytes, leaf| {
            let bytes = bytes
                .saturating_add(
                    leaf.order
                        .capacity()
                        .saturating_mul(std::mem::size_of::<u64>()),
                )
                .saturating_add(leaf.text.len())
                .saturating_add(RC_HEADER)
                .saturating_add(std::mem::size_of_val(leaf.copies.as_ref()))
                .saturating_add(RC_HEADER);
            leaf.copies.iter().fold(bytes, |bytes, (_, text)| {
                bytes.saturating_add(text.len()).saturating_add(RC_HEADER)
            })
        })
    }

    pub fn single(
        scope: impl Into<SharedString>,
        order: Vec<u64>,
        text: impl Into<SharedString>,
        separator: &'static str,
    ) -> Rc<Self> {
        Self::new(
            scope,
            order.clone(),
            vec![SelectionLeaf {
                order,
                text: text.into(),
                separator,
                copies: Rc::from([]),
            }],
        )
    }
    /// `owner` identifies one document across content revisions, independently
    /// of its changing leaf count. Markdown uses its Item/section prefix.
    pub fn new(
        scope: impl Into<SharedString>,
        owner: Vec<u64>,
        mut leaves: Vec<SelectionLeaf>,
    ) -> Rc<Self> {
        leaves.sort_by(|a, b| a.order.cmp(&b.order));
        Rc::new(Self {
            scope: scope.into(),
            owner: owner.into(),
            leaves,
        })
    }
}
/// Registration is weak: dropping the owning renderer releases the source.
pub fn register_selection_source(source: &Rc<SelectionSource>, window: &Window, cx: &mut App) {
    document(cx, window.window_handle().window_id()).register_source(source, window.current_view());
}

/// Release both source registrations and gesture snapshots when a thread or overlay closes.
pub fn release_selection_scope(scope: &str, window: gpui::WindowId, cx: &mut App) {
    document::release_scope(cx, window, scope);
}

/// Folds an arbitrary byte index into a valid caret position for `text`.
///
/// Out-of-range indices saturate at the text end; mid-character indices
/// floor to the previous character boundary, so every returned value is a
/// legal slicing position.
#[must_use]
pub fn clamp_to_char_boundary(text: &str, index: usize) -> usize {
    text.floor_char_boundary(index.min(text.len()))
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
    runs: Arc<[TextRun]>,
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
    native_geometry: Rc<RefCell<native_geometry::NativeGeometryCache>>,
    native_report: Rc<RefCell<Option<(usize, usize, Pixels, Pixels, Pixels, usize, usize)>>>,
    native_layout_report: Rc<RefCell<Option<(usize, Pixels, Pixels, usize)>>>,
    native_error: Rc<RefCell<Option<String>>>,
}

impl RetainedLeaf {
    /// Returns the runs for one layout, compiling only when an input
    /// changed since the previous layout.
    ///
    /// A miss moves the caller ranges into the cache (the element is
    /// consumed by this frame and never reads them again), so neither path
    /// copies them.
    fn compiled_runs(&self, inputs: &mut RunInputs<'_>) -> Arc<[TextRun]> {
        let Ok(mut compiled) = self.compiled.try_borrow_mut() else {
            return compile_runs(inputs).into();
        };
        if let Some(cached) = compiled.as_ref()
            && cached.matches(inputs)
        {
            return cached.runs.clone();
        }
        let runs: Arc<[TextRun]> = compile_runs(inputs).into();
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

/// Compiled original source styles shared by every visual ordinal of a leaf.
#[derive(Debug)]
pub(crate) struct NativeRunEntry {
    key: SharedString,
    compiled: CompiledRuns,
}
fn native_runs(
    renderer: &crate::markdown_renderer::MarkdownRenderer,
    key: &SharedString,
    inputs: &mut RunInputs<'_>,
) -> Arc<[TextRun]> {
    let mut entries = renderer.native_runs.borrow_mut();
    if let Some(entry) = entries
        .iter()
        .find(|entry| &entry.key == key && entry.compiled.matches(inputs))
    {
        return entry.compiled.runs.clone();
    }
    entries.retain(|entry| &entry.key != key);
    let runs: Arc<[TextRun]> = compile_runs(inputs).into();
    entries.push_back(NativeRunEntry {
        key: key.clone(),
        compiled: CompiledRuns {
            text: inputs.text.clone(),
            default_style: inputs.default_style.clone(),
            highlights: std::mem::take(inputs.highlights),
            overrides: std::mem::take(inputs.overrides),
            runs: runs.clone(),
        },
    });
    while entries.len() > 16
        || entries
            .iter()
            .map(|entry| {
                entry.compiled.text.len() + std::mem::size_of_val(entry.compiled.runs.as_ref())
            })
            .sum::<usize>()
            > 16 * 1024 * 1024
    {
        entries.pop_front();
    }
    runs
}

/// The selection identity of a painted leaf.
#[derive(Clone)]
struct LeafFrame {
    key: LeafKey,
    focus: FocusHandle,
}

/// Exact source/style topology supplied to layout and paint callbacks.
/// Only the paint callback is read evidence.
#[derive(Clone)]
pub struct NativeRowReady {
    pub ordinal: usize,
    pub row_count: usize,
    pub width: Pixels,
    pub font_size: Pixels,
    pub page: Arc<crate::markdown_renderer::NativeWindow>,
}
type NativeNotify = Rc<dyn Fn(&mut App)>;
type NativeReady = Rc<dyn Fn(NativeRowReady, &mut App)>;
type ProjectedState = (
    Arc<crate::markdown_renderer::native_projection::ProjectedNativeRow>,
    Pixels,
    Arc<[TextRun]>,
);
struct MeasuredNative {
    renderer: Rc<crate::markdown_renderer::MarkdownRenderer>,
    key: SharedString,
    ordinal: usize,
    range: Range<usize>,
    notify: NativeNotify,
    ready: NativeReady,
    layout_ready: NativeReady,
    layout_report: Rc<RefCell<Option<(usize, Pixels, Pixels, usize)>>>,
    prepared: Rc<RefCell<Option<(ProjectedState, NativeRowReady)>>>,
    report: Rc<RefCell<Option<(usize, usize, Pixels, Pixels, Pixels, usize, usize)>>>,
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
    selection_order: Rc<[u64]>,
    selection_scope: SharedString,
    separator: &'static str,
    text: SharedString,
    source_slice: Option<Range<usize>>,
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
    projected: Option<ProjectedState>,
    measured_native: Option<MeasuredNative>,
    native_failure: Option<Rc<dyn Fn(String, &mut App)>>,
    reveal_frontier: Option<(usize, Rc<dyn Fn(Option<Pixels>, &mut Window)>)>,
    geometry: Option<LeafLayout>,
    native_geometry: Option<Rc<RefCell<native_geometry::NativeGeometryCache>>>,
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
            selection_order: Rc::from([]),
            selection_scope: "".into(),
            separator: "\n\n",
            text: text.into(),
            source_slice: None,
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
            projected: None,
            measured_native: None,
            native_failure: None,
            reveal_frontier: None,
            geometry: None,
            native_geometry: None,
            leaf: None,
        }
    }

    /// Measures the actual inherited content width, then requests a bounded
    /// background native glyph window. No source is shaped during layout.
    /// `on_layout_ready` publishes topology even if resize removed this ordinal.
    /// Only `on_ready` is paint evidence; loading placeholders never invoke it.
    pub fn native_window(
        mut self,
        renderer: Rc<crate::markdown_renderer::MarkdownRenderer>,
        key: SharedString,
        ordinal: usize,
        range: Range<usize>,
        notify: Rc<dyn Fn(&mut App)>,
        on_layout_ready: Rc<dyn Fn(NativeRowReady, &mut App)>,
        on_ready: Rc<dyn Fn(NativeRowReady, &mut App)>,
    ) -> Self {
        assert!(range.contains(&ordinal) && range.len() <= 128);
        self.measured_native = Some(MeasuredNative {
            renderer,
            key,
            ordinal,
            range,
            notify,
            ready: on_ready,
            layout_ready: on_layout_ready,
            layout_report: Rc::default(),
            prepared: Rc::default(),
            report: Rc::default(),
        });
        self
    }

    /// Reports the bottom of an already shaped logical prefix. The complete
    /// native text remains the layout/selection source; this never shapes a slice.
    pub(crate) fn with_reveal_frontier(
        mut self,
        end: usize,
        report: Rc<dyn Fn(Option<Pixels>, &mut Window)>,
    ) -> Self {
        self.reveal_frontier = Some((end, report));
        self
    }

    /// Reports explicit native admission or shaping errors to the owning view.
    /// A failed source remains noninteractive until the owner retries or replaces it.
    pub fn native_failure_handler(mut self, callback: Rc<dyn Fn(String, &mut App)>) -> Self {
        self.native_failure = Some(callback);
        self
    }

    /// Stable source order, independent of painting, scrolling and layout. The prefix
    /// identifies the transcript item; suffixes identify prepared blocks and cells.
    /// `separator` is inserted before this leaf when a selection crosses into it.
    #[must_use]
    pub fn selection_scope(mut self, scope: impl Into<SharedString>) -> Self {
        self.selection_scope = scope.into();
        self
    }
    pub fn selection_order(mut self, order: Vec<u64>, separator: &'static str) -> Self {
        self.selection_order = order.into();
        self.separator = separator;
        self
    }

    /// Displays a byte range of the full registered semantic leaf. The caller
    /// supplies exactly that UTF-8 slice as `text`; links and style ranges remain
    /// local to the displayed text. Selection/copy offsets address the full source.
    /// A fragment without a matching registered source cannot become a partial
    /// substitute for that source.
    #[must_use]
    pub fn source_slice(mut self, range: Range<usize>) -> Self {
        assert_eq!(range.end.checked_sub(range.start), Some(self.text.len()));
        self.source_slice = Some(range);
        self
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
        self.native_geometry = retained
            .as_ref()
            .map(|retained| retained.native_geometry.clone());
        self.leaf = retained
            .as_ref()
            .and_then(|retained| retained.focus.clone())
            .zip(global_id)
            .map(|(focus, global_id)| LeafFrame {
                key: if self.selection_order.is_empty() {
                    LeafKey::new(global_id)
                } else {
                    LeafKey::semantic(&self.selection_scope, &self.selection_order)
                },
                focus,
            });
        if let Some((row, height, _)) = &self.projected {
            let mut style = gpui::Style::default();
            style.size.width = row.layout.width.into();
            style.size.height = (*height).into();
            return (window.request_layout(style, [], cx), ());
        }
        let default_style = window.text_style();
        let mut inputs = RunInputs {
            text: &self.text,
            default_style: &default_style,
            highlights: &mut self.base_highlights,
            overrides: &mut self.text_run_overrides,
        };
        let runs = if let Some(native) = self.measured_native.as_ref() {
            native_runs(&native.renderer, &native.key, &mut inputs)
        } else {
            match retained.as_ref() {
                Some(retained) => retained.compiled_runs(&mut inputs),
                None => Arc::from(compile_runs(&inputs)),
            }
        };
        if let Some(native) = self.measured_native.as_mut() {
            if let Some(retained) = retained.as_ref() {
                native.report = retained.native_report.clone();
                native.layout_report = retained.native_layout_report.clone();
            }
            let renderer = native.renderer.clone();
            let key = native.key.clone();
            let range = native.range.clone();
            let ordinal = native.ordinal;
            let notify = native.notify.clone();
            let prepared = native.prepared.clone();
            let layout_report = native.layout_report.clone();
            let layout_ready = native.layout_ready.clone();
            let failure = self.native_failure.clone();
            let error_state = retained
                .as_ref()
                .map(|retained| retained.native_error.clone())
                .unwrap_or_default();
            let source = self.text.clone();
            let font_size = default_style.font_size.to_pixels(window.rem_size());
            let height = window.pixel_snap(
                default_style
                    .line_height
                    .to_pixels(font_size.into(), window.rem_size()),
            );
            let layout = window.request_measured_layout(
                gpui::Style::default(),
                move |known, available, _, cx| {
                    // Intrinsic probes provide no wrap width. Await the parent's
                    // definite content constraint instead of guessing viewport width.
                    let width = known.width.or_else(|| match available.width {
                        gpui::AvailableSpace::Definite(width) => Some(width),
                        _ => None,
                    });
                    *prepared.borrow_mut() = None;
                    let Some(width) = width
                        .filter(|width| f32::from(*width).is_finite() && *width > Pixels::ZERO)
                    else {
                        return gpui::size(Pixels::ZERO, height);
                    };
                    let notify = notify.clone();
                    let result = renderer.prepare_native_window(
                        key.clone(),
                        source.clone(),
                        font_size,
                        width,
                        runs.clone(),
                        range.clone(),
                        cx,
                        move |cx| notify(cx),
                    );
                    if let crate::markdown_renderer::NativePreparation::Ready(page) = &result {
                        *error_state.borrow_mut() = None;
                        let ready = NativeRowReady {
                            ordinal,
                            row_count: page.row_count(),
                            width,
                            font_size,
                            page: page.clone(),
                        };
                        let fingerprint = (
                            page.runs.as_ptr() as usize,
                            width,
                            font_size,
                            page.row_count(),
                        );
                        if layout_report.borrow().as_ref() != Some(&fingerprint) {
                            *layout_report.borrow_mut() = Some(fingerprint);
                            let ready = ready.clone();
                            let callback = layout_ready.clone();
                            cx.defer(move |cx| callback(ready, cx));
                        }
                        if let Some(row) = ordinal
                            .checked_sub(page.start)
                            .and_then(|index| page.rows.get(index))
                        {
                            *prepared.borrow_mut() = Some((
                                (row.clone(), height, page.runs.clone()),
                                NativeRowReady {
                                    ordinal,
                                    row_count: page.row_count(),
                                    width,
                                    font_size,
                                    page: page.clone(),
                                },
                            ));
                        }
                    }
                    if let crate::markdown_renderer::NativePreparation::Failed(error) = result
                        && error_state.borrow().as_ref() != Some(&error)
                    {
                        *error_state.borrow_mut() = Some(error.clone());
                        if let Some(callback) = failure.clone() {
                            cx.defer(move |cx| callback(error, cx));
                        }
                    }
                    gpui::size(width, height)
                },
            );
            return (layout, ());
        }
        let mut styled = StyledText::new(self.text.clone()).with_runs(runs.to_vec());
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
        if let Some(native) = self.measured_native.as_ref() {
            self.projected = native
                .prepared
                .borrow()
                .as_ref()
                .map(|(row, _)| row.clone());
            if self.projected.is_none() {
                self.geometry = None;
                return window.insert_hitbox(bounds, HitboxBehavior::Normal);
            }
        }
        if let Some(leaf) = self.leaf.as_ref() {
            window.set_focus_handle(&leaf.focus, cx);
        }
        self.geometry = if let Some((row, height, _)) = &self.projected {
            Some(LeafLayout::Projected {
                row: row.clone(),
                bounds,
                height: *height,
            })
        } else {
            let styled = self
                .styled
                .as_mut()
                .expect("request_layout prepares selectable text before prepaint");
            styled.prepaint(None, inspector_id, bounds, state, window, cx);
            let layout = styled.layout().clone();
            let cells = self.native_geometry.as_ref().and_then(|cache| {
                cache
                    .borrow_mut()
                    .visible(&self.text, &layout, window.content_mask().bounds)
            });
            Some(LeafLayout::Native(layout, cells))
        };
        if let Some((end, report)) = &self.reveal_frontier {
            let end = (*end).min(self.text.len());
            let index = self.text[..end]
                .char_indices()
                .next_back()
                .map(|(index, _)| index);
            let bottom = index.and_then(|index| {
                if let Some((row, height, _)) = &self.projected {
                    row.clusters
                        .iter()
                        .any(|cluster| cluster.source.start < end)
                        .then_some(bounds.top() + *height)
                } else {
                    // The original TextLayout owns every native glyph and wrap.
                    // Do not use the clipped hit-cell cache to infer full geometry.
                    self.styled.as_ref().and_then(|styled| {
                        let layout = styled.layout();
                        layout
                            .position_for_index(index)
                            .map(|position| position.y + layout.line_height())
                    })
                }
            });
            report(bottom, window);
        }
        if let Some(leaf) = self.leaf.as_ref() {
            let window_id = window.window_handle().window_id();
            let doc = document(cx, window_id);
            if doc.is_active() {
                let observed = Observed {
                    key: leaf.key,
                    scope: self.selection_scope.clone(),
                    order: self.selection_order.clone(),
                    separator: self.separator,
                    text: self.text.clone(),
                    source_slice: self.source_slice.clone(),
                    copies: Rc::clone(&self.copies),
                    view: window.current_view(),
                    bounds,
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
        let Some(layout) = self.geometry.clone() else {
            return;
        };

        if let Some(native) = self.measured_native.as_ref()
            && bounds.intersects(&window.content_mask().bounds)
            && let Some((_, ready)) = native.prepared.borrow().as_ref()
        {
            let fingerprint = (
                self.text.as_ptr() as usize,
                ready.page.runs.as_ptr() as usize,
                ready.width,
                ready.font_size,
                self.projected.as_ref().unwrap().1,
                ready.ordinal,
                ready.row_count,
            );
            if native.report.borrow().as_ref() != Some(&fingerprint) {
                *native.report.borrow_mut() = Some(fingerprint);
                let ready = ready.clone();
                let callback = native.ready.clone();
                cx.defer(move |cx| callback(ready, cx));
            }
        }
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
            if let Some(range) = doc.paint_slice_range(
                leaf.key,
                &self.selection_scope,
                &self.text,
                self.source_slice.as_ref(),
            ) {
                layout.paint_selection(&range, self.selection_color, window);
            }
            let live = LiveLeaf {
                observed: Observed {
                    key: leaf.key,
                    scope: self.selection_scope.clone(),
                    order: self.selection_order.clone(),
                    separator: self.separator,
                    text: self.text.clone(),
                    source_slice: self.source_slice.clone(),
                    copies: Rc::clone(&self.copies),
                    view: window.current_view(),
                    bounds: layout.bounds(),
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
        if let Some((row, height, styles)) = &self.projected {
            let baseline =
                (*height - row.layout.ascent - row.layout.descent) / 2.0 + row.layout.ascent;
            let style_at = |index: usize| {
                let mut end = 0;
                styles.iter().find(|style| {
                    end += style.len;
                    index < end
                })
            };
            for cell in &row.clusters {
                let Some(style) = style_at(cell.source.start) else {
                    continue;
                };
                if let Some(color) = style.background_color {
                    window.paint_quad(gpui::fill(
                        Bounds::new(
                            bounds.origin + gpui::point(cell.left, Pixels::ZERO),
                            gpui::size(cell.right - cell.left, *height),
                        ),
                        color,
                    ));
                }
                if let Some(underline) = &style.underline {
                    let mut underline = underline.clone();
                    underline.color = Some(underline.color.unwrap_or(style.color));
                    window.paint_underline(
                        bounds.origin
                            + gpui::point(cell.left, baseline + row.layout.descent * 0.618),
                        cell.right - cell.left,
                        &underline,
                    );
                }
                if let Some(strike) = &style.strikethrough {
                    let mut strike = strike.clone();
                    strike.color = Some(strike.color.unwrap_or(style.color));
                    window.paint_strikethrough(
                        bounds.origin
                            + gpui::point(cell.left, (row.layout.ascent * 0.5 + baseline) * 0.5),
                        cell.right - cell.left,
                        &strike,
                    );
                }
            }
            for run in &row.layout.runs {
                for glyph in &run.glyphs {
                    let Some(style) = style_at(glyph.index) else {
                        continue;
                    };
                    let origin =
                        bounds.origin + glyph.position + gpui::point(Pixels::ZERO, baseline);
                    if glyph.is_emoji {
                        let _ =
                            window.paint_emoji(origin, run.font_id, glyph.id, row.layout.font_size);
                    } else {
                        let _ = window.paint_glyph(
                            origin,
                            run.font_id,
                            glyph.id,
                            row.layout.font_size,
                            style.color,
                        );
                    }
                }
            }
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
fn paint_swatch(layout: &LeafLayout, swatch: &InlineSwatch, edge: Hsla, window: &mut Window) {
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
fn paint_native_selection(
    layout: &TextLayout,
    range: &Range<usize>,
    color: Hsla,
    window: &mut Window,
) {
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
    layout: LeafLayout,
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

/// Ctrl+A selects the focused source scope; Ctrl+C copies the selection.
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
                change_selection(window_id, cx, |doc| doc.select_source(observed));
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
    let offset = hit.observed.source_offset(match unit {
        SelectionUnit::Character => hit.layout.caret(event.position),
        SelectionUnit::Word | SelectionUnit::Paragraph => glyph_index(hit, event.position),
    });
    let glyph = glyph_index(hit, event.position);
    let extend = event.modifiers.shift && document(cx, window_id).has_selection();
    let live: Vec<_> = observed(roster)
        .into_iter()
        .filter(|leaf| leaf.scope == hit.observed.scope)
        .collect();
    let key = hit.observed.key;
    change_selection(window_id, cx, |doc| {
        if extend {
            doc.observe(&live);
        } else {
            doc.reset_with_sources(&live);
        }
        doc.press(key, offset, unit, extend);
        doc.set_pressed(key, hit.observed.source_offset(glyph), event.click_count);
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
        let caret = live.layout.caret(position);
        return Some((
            live.observed.key,
            live.observed.source_offset(caret),
            live.observed.source_offset(glyph_index(live, position)),
        ));
    }
    let above = roster
        .iter()
        .filter(|live| live.observed.bounds.bottom() <= position.y)
        .max_by(|left, right| {
            f32::from(left.observed.bounds.bottom())
                .total_cmp(&f32::from(right.observed.bounds.bottom()))
        });
    if let Some(live) = above {
        let end = live
            .observed
            .source_offset(live.layout.source_range(live.observed.text.len()).end);
        return Some((live.observed.key, end, end));
    }
    roster.first().map(|live| {
        let start = live
            .observed
            .source_offset(live.layout.source_range(live.observed.text.len()).start);
        (live.observed.key, start, start)
    })
}

fn drag(roster: &[LiveLeaf], position: Point<Pixels>, window: &mut Window, cx: &mut App) {
    let window_id = window.window_handle().window_id();
    let live = observed(roster);
    let target = drag_target(roster, position);
    let doc = document(cx, window_id);
    doc.observe(&live);
    if let Some((key, caret, glyph)) = target {
        for view in doc.drag_views(key, caret, glyph) {
            cx.notify(view);
        }
    }
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
    let Some(down) = down.checked_sub(hit.observed.source_offset(0)) else {
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
mod native_tests;
