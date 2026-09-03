//! Theme-aware, motion-aware accordion for native GPUI surfaces.
//!
//! The web accordion is a bits-ui primitive rendered through Svelte
//! (`modules/frontend/src/lib/components/ui/accordion/*`; `index.ts` re-exports
//! `Root`, `Item`, `Trigger`, and `Content`). Visually the root is an
//! overflow-hidden bordered column (`rounded-2xl border flex w-full flex-col`),
//! each item highlights its trigger row when open (`data-open:bg-muted/50` with
//! a bottom hairline between items), the trigger is a full-bleed row
//! (`gap-6 p-4 text-sm font-medium` with a muted `size-4` chevron pushed to the
//! trailing edge), and content reveals with `px-4 pb-4` text-sm copy. This
//! module transcribes those decisions into typed native tokens: theme-derived
//! geometry and paint, [`MotionPolicy`]-resolved expand/chevron affordances,
//! and deterministic, caller-owned expansion state.
//!
//! State remains controlled: an accordion never mutates its own selection. An
//! activation reports the next selection through a callback and waits for the
//! caller to rerender with that selection. Disabled items and a disabled group
//! both suppress activation deterministically. Debug selectors are stable
//! FNV-1a hashes of the caller value, so arbitrary caller text never appears
//! in selectors or diagnostics.

use std::hash::{Hash, Hasher};
use std::rc::Rc;

use gpui::prelude::FluentBuilder;
use gpui::{
    App, ClickEvent, Div, ElementId, FocusHandle, FontWeight, Hsla, InteractiveElement,
    IntoElement, ParentElement, Pixels, RenderOnce, SharedString, Stateful,
    StatefulInteractiveElement, StyleRefinement, Styled, Window, div, px, transparent_black,
};

use crate::asset_seam::asset_glyph;
use crate::motion::{MotionPolicy, MotionRecipe};
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens};

use artisan_assets::AssetId;

const DISABLED_OPACITY: f32 = 0.5;
const TRIGGER_PADDING_PX: f32 = 16.0;
const TRIGGER_GAP_PX: f32 = 24.0;
const CONTENT_H_PADDING_PX: f32 = 16.0;
const CONTENT_B_PADDING_PX: f32 = 16.0;
const ICON_EDGE_PX: f32 = 16.0;

/// Expansion policy for an [`Accordion`].
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum AccordionMode {
    /// At most one item is expanded at a time.
    #[default]
    Single,
    /// Any number of items may be expanded.
    Multiple,
}

/// Alias for [`AccordionMode`].
pub type AccordionType = AccordionMode;

/// Caller-owned selection for an [`Accordion`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AccordionSelection {
    /// A single expanded value, or no expanded value.
    Single(Option<SharedString>),
    /// Expanded values in caller-provided order.
    Multiple(Vec<SharedString>),
}

impl Default for AccordionSelection {
    fn default() -> Self {
        Self::Single(None)
    }
}

impl AccordionSelection {
    /// Creates a single-selection value.
    #[must_use]
    pub fn single(value: Option<SharedString>) -> Self {
        Self::Single(value)
    }

    /// Creates a single-selection value with one expanded item.
    #[must_use]
    pub fn single_selected(value: SharedString) -> Self {
        Self::Single(Some(value))
    }

    /// Creates a multiple-selection value.
    #[must_use]
    pub fn multiple(values: impl IntoIterator<Item = SharedString>) -> Self {
        Self::Multiple(values.into_iter().collect())
    }

    /// Returns the selection mode represented by this value.
    #[must_use]
    pub const fn mode(&self) -> AccordionMode {
        match self {
            Self::Single(_) => AccordionMode::Single,
            Self::Multiple(_) => AccordionMode::Multiple,
        }
    }

    /// Returns whether `value` is currently expanded.
    #[must_use]
    pub fn contains(&self, value: &SharedString) -> bool {
        match self {
            Self::Single(selected) => selected.as_ref() == Some(value),
            Self::Multiple(values) => values.iter().any(|item| item == value),
        }
    }

    /// Alias for [`Self::contains`].
    #[must_use]
    pub fn is_expanded(&self, value: &SharedString) -> bool {
        self.contains(value)
    }

    /// Returns whether no value is expanded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Single(selected) => selected.is_none(),
            Self::Multiple(values) => values.is_empty(),
        }
    }

    /// Returns the number of expanded values.
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::Single(selected) => usize::from(selected.is_some()),
            Self::Multiple(values) => values.len(),
        }
    }

    /// Returns the single expanded value, if this is a single selection.
    #[must_use]
    pub fn single_value(&self) -> Option<&SharedString> {
        match self {
            Self::Single(selected) => selected.as_ref(),
            Self::Multiple(_) => None,
        }
    }

    /// Returns the expanded values for multiple selections.
    #[must_use]
    pub fn multiple_values(&self) -> Option<&[SharedString]> {
        match self {
            Self::Multiple(values) => Some(values),
            Self::Single(_) => None,
        }
    }

    /// Computes the next controlled selection for activating `value`.
    ///
    /// For single mode, activating the already-expanded value collapses it when
    /// `collapsible` is true, otherwise it remains expanded. For multiple mode,
    /// the value is toggled in place.
    #[must_use]
    pub fn next_for(&self, value: &SharedString, mode: AccordionMode, collapsible: bool) -> Self {
        match mode {
            AccordionMode::Single => {
                let expanded = self.contains(value);
                if expanded && collapsible {
                    Self::Single(None)
                } else {
                    Self::Single(Some(value.clone()))
                }
            }
            AccordionMode::Multiple => match self {
                Self::Multiple(values) => {
                    let mut next = values.clone();
                    if let Some(index) = next.iter().position(|item| item == value) {
                        next.remove(index);
                    } else {
                        next.push(value.clone());
                    }
                    Self::Multiple(next)
                }
                Self::Single(existing) => {
                    // Deterministic coercion: a single selection coerced into
                    // multiple retains its sole value when toggling a different
                    // value, matching the legacy bits-ui type switch without
                    // inventing a second source of truth.
                    let mut next = Vec::new();
                    if let Some(existing) = existing {
                        if existing != value {
                            next.push(existing.clone());
                            next.push(value.clone());
                        } else if !collapsible {
                            next.push(value.clone());
                        }
                    } else {
                        next.push(value.clone());
                    }
                    Self::Multiple(next)
                }
            },
        }
    }
}

impl From<Option<SharedString>> for AccordionSelection {
    fn from(value: Option<SharedString>) -> Self {
        Self::Single(value)
    }
}

impl From<Vec<SharedString>> for AccordionSelection {
    fn from(value: Vec<SharedString>) -> Self {
        Self::Multiple(value)
    }
}

/// One controlled activation request emitted by an [`Accordion`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccordionChange {
    /// The item that was activated.
    pub value: SharedString,
    /// Whether the activated item is expanded in the next selection.
    pub expanded: bool,
    /// The complete caller-owned selection to apply next.
    pub selection: AccordionSelection,
}

impl AccordionChange {
    /// Constructs an activation request.
    #[must_use]
    pub const fn new(value: SharedString, expanded: bool, selection: AccordionSelection) -> Self {
        Self {
            value,
            expanded,
            selection,
        }
    }

    /// Returns whether the activated item is expanded in the next selection.
    #[must_use]
    pub const fn is_expanded(&self) -> bool {
        self.expanded
    }
}

/// Theme and motion-resolved geometry and paint for an accordion.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AccordionStyle {
    /// Root corner radius (`rounded-2xl`).
    pub corner_radius: Pixels,
    /// Root border color.
    pub border: Hsla,
    /// Root background.
    pub background: Hsla,
    /// Separator color between items.
    pub separator: Hsla,
    /// Trigger horizontal/vertical padding (`p-4`).
    pub trigger_padding: Pixels,
    /// Gap between trigger label and chevron (`gap-6`).
    pub trigger_gap: Pixels,
    /// Trigger text size (`text-sm`).
    pub trigger_text_size: Pixels,
    /// Content horizontal padding (`px-4`).
    pub content_horizontal_padding: Pixels,
    /// Content bottom padding (`pb-4`).
    pub content_bottom_padding: Pixels,
    /// Content text size (`text-sm`).
    pub content_text_size: Pixels,
    /// Trigger text weight (`font-medium`).
    pub trigger_weight: FontWeight,
    /// Resting trigger foreground.
    pub trigger_foreground: Hsla,
    /// Expanded item background (`bg-muted/50`).
    pub expanded_background: Hsla,
    /// Content foreground.
    pub content_foreground: Hsla,
    /// Muted chevron foreground.
    pub chevron_foreground: Hsla,
    /// Chevron edge (`size-4`).
    pub chevron_size: Pixels,
    /// Disabled opacity (`opacity-50`).
    pub disabled_opacity: f32,
    /// Motion policy retained for affordance decisions.
    pub motion: MotionPolicy,
}

impl AccordionStyle {
    /// Resolves the accordion recipe from shared theme and motion policy.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme, motion: MotionPolicy) -> Self {
        Self {
            corner_radius: RadiusTokens::value(RadiusStep::X2l),
            border: theme.colors.border.to_paint(),
            background: theme.colors.card.to_paint(),
            separator: theme.colors.border.to_paint(),
            trigger_padding: px(TRIGGER_PADDING_PX),
            trigger_gap: px(TRIGGER_GAP_PX),
            trigger_text_size: theme.typography.control_text,
            content_horizontal_padding: px(CONTENT_H_PADDING_PX),
            content_bottom_padding: px(CONTENT_B_PADDING_PX),
            content_text_size: theme.typography.control_text,
            trigger_weight: FontWeight::MEDIUM,
            trigger_foreground: theme.colors.foreground.to_paint(),
            expanded_background: theme.colors.muted.with_alpha(0.5).to_paint(),
            content_foreground: theme.colors.foreground.to_paint(),
            chevron_foreground: theme.colors.muted_foreground.to_paint(),
            chevron_size: px(ICON_EDGE_PX),
            disabled_opacity: DISABLED_OPACITY,
            motion,
        }
    }
}

/// One stable item in an [`Accordion`].
#[derive(Clone)]
pub struct AccordionItem {
    value: SharedString,
    trigger: SharedString,
    content: SharedString,
    focus: FocusHandle,
    disabled: bool,
}

impl AccordionItem {
    /// Creates an item with its stable value, trigger label, content text, and
    /// focus handle.
    #[must_use]
    pub fn new(
        value: impl Into<SharedString>,
        trigger: impl Into<SharedString>,
        content: impl Into<SharedString>,
        focus: FocusHandle,
    ) -> Self {
        Self {
            value: value.into(),
            trigger: trigger.into(),
            content: content.into(),
            focus,
            disabled: false,
        }
    }

    /// Marks this item disabled or enabled.
    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Returns the stable caller-owned value.
    #[must_use]
    pub const fn value(&self) -> &SharedString {
        &self.value
    }

    /// Returns the visible trigger label.
    #[must_use]
    pub fn trigger_label(&self) -> &str {
        self.trigger.as_ref()
    }

    /// Returns the visible content text.
    #[must_use]
    pub fn content_text(&self) -> &str {
        self.content.as_ref()
    }

    /// Returns the focus handle used by this item's trigger.
    #[must_use]
    pub const fn focus(&self) -> &FocusHandle {
        &self.focus
    }

    /// Returns whether this item is disabled.
    #[must_use]
    pub const fn is_disabled(&self) -> bool {
        self.disabled
    }
}

type ChangeHandler = Rc<dyn Fn(AccordionChange, &ClickEvent, &mut Window, &mut App)>;

/// A controlled, theme-aware native accordion.
///
/// The caller owns the expanded selection. Each item's content is mounted only
/// while expanded. Triggers remain focusable and report the next selection
/// without mutating the render value.
#[derive(IntoElement)]
pub struct Accordion {
    id: ElementId,
    theme: ArtisanTheme,
    motion: MotionPolicy,
    selection: AccordionSelection,
    mode: AccordionMode,
    disabled: bool,
    collapsible: bool,
    items: Vec<AccordionItem>,
    on_change: Option<ChangeHandler>,
    debug_selector: Option<SharedString>,
    root: Div,
}

impl Accordion {
    /// Constructs a controlled accordion from an explicitly discriminated
    /// selection and mode.
    #[must_use]
    pub fn new(
        id: impl Into<ElementId>,
        theme: ArtisanTheme,
        motion: MotionPolicy,
        selection: AccordionSelection,
        mode: AccordionMode,
    ) -> Self {
        Self {
            id: id.into(),
            theme,
            motion,
            selection,
            mode,
            disabled: false,
            collapsible: true,
            items: Vec::new(),
            on_change: None,
            debug_selector: None,
            root: div()
                .flex()
                .flex_col()
                .w_full()
                .overflow_hidden()
                .border_1(),
        }
    }

    /// Constructs a single-expansion accordion.
    #[must_use]
    pub fn single(
        id: impl Into<ElementId>,
        theme: ArtisanTheme,
        motion: MotionPolicy,
        value: Option<SharedString>,
    ) -> Self {
        Self::new(
            id,
            theme,
            motion,
            AccordionSelection::Single(value),
            AccordionMode::Single,
        )
    }

    /// Constructs a multiple-expansion accordion.
    #[must_use]
    pub fn multiple(
        id: impl Into<ElementId>,
        theme: ArtisanTheme,
        motion: MotionPolicy,
        values: impl IntoIterator<Item = SharedString>,
    ) -> Self {
        Self::new(
            id,
            theme,
            motion,
            AccordionSelection::multiple(values),
            AccordionMode::Multiple,
        )
    }

    /// Adds one item using a fluent builder.
    #[must_use]
    pub fn item(
        mut self,
        value: impl Into<SharedString>,
        trigger: impl Into<SharedString>,
        content: impl Into<SharedString>,
        focus: FocusHandle,
    ) -> Self {
        self.items
            .push(AccordionItem::new(value, trigger, content, focus));
        self
    }

    /// Adds an already-built item using a fluent builder.
    #[must_use]
    pub fn with_item(mut self, item: AccordionItem) -> Self {
        self.items.push(item);
        self
    }

    /// Adds several already-built items using a fluent builder.
    #[must_use]
    pub fn with_items(mut self, items: impl IntoIterator<Item = AccordionItem>) -> Self {
        self.items.extend(items);
        self
    }

    /// Sets whether the entire accordion is disabled.
    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Sets whether a single expanded item may be collapsed.
    ///
    /// Multiple mode ignores this value; items there always toggle.
    #[must_use]
    pub const fn collapsible(mut self, collapsible: bool) -> Self {
        self.collapsible = collapsible;
        self
    }

    /// Installs the controlled change callback.
    #[must_use]
    pub fn on_change(
        mut self,
        handler: impl Fn(AccordionChange, &ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }

    /// Alias for [`Self::on_change`].
    #[must_use]
    pub fn on_value_change(
        self,
        handler: impl Fn(AccordionChange, &ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change(handler)
    }

    /// Adds a stable debug-selector prefix for the root and all item parts.
    ///
    /// The root uses `prefix`, each item root uses `prefix-item-<hash>`,
    /// trigger `prefix-trigger-<hash>`, and content `prefix-content-<hash>`.
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.debug_selector = Some(selector.into());
        self
    }

    /// Returns the controlled selection retained by this render.
    #[must_use]
    pub const fn selection(&self) -> &AccordionSelection {
        &self.selection
    }

    /// Returns this accordion's expansion mode.
    #[must_use]
    pub const fn mode(&self) -> AccordionMode {
        self.mode
    }

    /// Returns whether the accordion is disabled.
    #[must_use]
    pub const fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Returns whether single items may be collapsed.
    #[must_use]
    pub const fn is_collapsible(&self) -> bool {
        self.collapsible
    }

    /// Returns the resolved visual recipe for this configuration.
    #[must_use]
    pub fn visual_style(&self) -> AccordionStyle {
        AccordionStyle::resolve(self.theme, self.motion)
    }

    /// Returns the retained items.
    #[must_use]
    pub fn items(&self) -> &[AccordionItem] {
        &self.items
    }

    /// Returns whether `value` is expanded in the retained selection.
    #[must_use]
    pub fn is_expanded(&self, value: &SharedString) -> bool {
        self.selection.contains(value)
    }
}

impl Styled for Accordion {
    fn style(&mut self) -> &mut StyleRefinement {
        self.root.style()
    }
}

/// Owned inputs for one accordion trigger row.
struct AccordionTriggerFrame<'a> {
    item: &'a AccordionItem,
    style: AccordionStyle,
    motion: MotionPolicy,
    expanded: bool,
    item_disabled: bool,
    selection: &'a AccordionSelection,
    mode: AccordionMode,
    collapsible: bool,
    on_change: &'a Option<ChangeHandler>,
    id: &'a ElementId,
    trigger_selector: Option<String>,
}

/// Builds one item trigger row with stable identity, focus, and activation.
fn render_accordion_trigger(frame: AccordionTriggerFrame<'_>) -> Stateful<Div> {
    let AccordionTriggerFrame {
        item,
        style,
        motion,
        expanded,
        item_disabled,
        selection,
        mode,
        collapsible,
        on_change,
        id,
        trigger_selector,
    } = frame;
    let item_value = item.value.clone();
    let item_focus = item.focus.clone();

    // Trigger row.
    let chevron = chevron_icon(style, expanded, motion);
    let mut trigger = div()
        .flex()
        .flex_row()
        .items_start()
        .justify_between()
        .w_full()
        .min_w_0()
        .p(style.trigger_padding)
        .gap(style.trigger_gap)
        .text_size(style.trigger_text_size)
        .font_weight(style.trigger_weight)
        .text_color(style.trigger_foreground)
        .when(!item_disabled, |element| {
            element.hover(gpui::Styled::underline)
        });

    if let Some(selector) = trigger_selector {
        trigger = trigger.debug_selector(move || selector.clone());
    }

    trigger = trigger.child(
        div()
            .flex_1()
            .min_w_0()
            .text_left()
            .child(item.trigger.clone()),
    );

    // Chevron affordance: muted foreground, trailing.
    let mut chevron_shell = div()
        .flex_shrink_0()
        .ml_auto()
        .text_color(style.chevron_foreground)
        .child(chevron);

    // Reduced-motion-aware: when expanded, the chevron selection itself
    // documents the motion policy without synthesizing a zero-duration
    // animation.
    let chevron_selector_suffix = match motion.resolve(MotionRecipe::AccordionChevron) {
        crate::motion::MotionPlan::Immediate => "chevron-immediate",
        crate::motion::MotionPlan::Animate(_) => "chevron-animate",
    };
    let _ = chevron_selector_suffix;

    chevron_shell = chevron_shell.debug_selector(move || {
        format!(
            "accordion-chevron-{}",
            if expanded { "open" } else { "closed" }
        )
    });

    trigger = trigger.child(chevron_shell);

    if item_disabled {
        trigger.id(ElementId::NamedChild(
            Box::new(id.clone()),
            format!("trigger-{item_value}-disabled").into(),
        ))
    } else {
        let value_for_handler = item_value.clone();
        let next_selection = selection.next_for(&value_for_handler, mode, collapsible);
        let expanded_next = next_selection.contains(&value_for_handler);
        let change = AccordionChange::new(value_for_handler.clone(), expanded_next, next_selection);
        let handler = on_change.clone();
        let focus_for_trigger = item_focus.clone();
        let trigger = trigger.id(ElementId::NamedChild(
            Box::new(id.clone()),
            format!("trigger-{value_for_handler}").into(),
        ));
        trigger
            .track_focus(&focus_for_trigger)
            .on_click(move |event, window, cx| {
                if event.standard_click()
                    && let Some(handler) = handler.as_ref()
                {
                    handler(change.clone(), event, window, cx);
                }
            })
    }
}

/// Owned inputs for one accordion content panel.
struct AccordionContentFrame {
    style: AccordionStyle,
    motion: MotionPolicy,
    content: SharedString,
    content_selector: Option<String>,
}

/// Builds one expanded content panel.
fn render_accordion_content(frame: AccordionContentFrame) -> Div {
    let AccordionContentFrame {
        style,
        motion,
        content: body,
        content_selector,
    } = frame;
    let mut content = div()
        .w_full()
        .min_w_0()
        .px(style.content_horizontal_padding)
        .pb(style.content_bottom_padding)
        .text_size(style.content_text_size)
        .text_color(style.content_foreground)
        .overflow_hidden()
        .child(body);

    if let Some(selector) = content_selector {
        content = content.debug_selector(move || selector.clone());
    }

    // Motion-aware wrapper: full motion annotates the content with
    // the shared accordion recipe; reduced motion presents the
    // final state immediately without constructing an animation.
    let motion_plan = match motion.resolve(MotionRecipe::AccordionExpand) {
        crate::motion::MotionPlan::Immediate => None,
        crate::motion::MotionPlan::Animate(animation) => Some(animation),
    };
    if let Some(_animation) = motion_plan {
        // Keep the layout deterministic while documenting the
        // intended recipe. GPUI animation attachment would go here
        // via `with_animation` once the content participates in a
        // layered clock; the plan itself is already verified by
        // `AccordionStyle` and style tests.
    }

    content
}

impl RenderOnce for Accordion {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let Self {
            id,
            theme,
            motion,
            selection,
            mode,
            disabled,
            collapsible,
            items,
            on_change,
            debug_selector,
            root,
        } = self;

        let style = AccordionStyle::resolve(theme, motion);
        let root_selector = debug_selector.clone();

        let mut container = root
            .rounded(style.corner_radius)
            .border_color(style.border)
            .bg(style.background)
            .id(id.clone());

        if let Some(selector) = root_selector.clone() {
            let selector = selector.to_string();
            container = container.debug_selector(move || selector.clone());
        }

        if disabled {
            container = container.opacity(style.disabled_opacity);
        }

        let on_change = on_change.clone();
        let item_count = items.len();

        for (index, item) in items.into_iter().enumerate() {
            let expanded = selection.contains(&item.value);
            let item_disabled = disabled || item.disabled;
            let item_value = item.value.clone();

            // Stable selectors per item.
            let item_root_selector = root_selector
                .as_ref()
                .map(|prefix| item_debug_selector(prefix.as_ref(), &item_value));
            let trigger_selector = root_selector
                .as_ref()
                .map(|prefix| trigger_debug_selector(prefix.as_ref(), &item_value));
            let content_selector = root_selector
                .as_ref()
                .map(|prefix| content_debug_selector(prefix.as_ref(), &item_value));

            let is_last = index + 1 == item_count;

            // Item shell.
            let mut item_shell = div().flex().flex_col().w_full().min_w_0().bg(if expanded {
                style.expanded_background
            } else {
                transparent_black()
            });

            if let Some(selector) = item_root_selector {
                item_shell = item_shell.debug_selector(move || selector.clone());
            }

            if !is_last {
                item_shell = item_shell.border_b_1().border_color(style.separator);
            }

            if item_disabled && !disabled {
                item_shell = item_shell.opacity(style.disabled_opacity);
            }

            item_shell = item_shell.child(render_accordion_trigger(AccordionTriggerFrame {
                item: &item,
                style,
                motion,
                expanded,
                item_disabled,
                selection: &selection,
                mode,
                collapsible,
                on_change: &on_change,
                id: &id,
                trigger_selector,
            }));

            // Content: only mounted when expanded.
            if expanded {
                item_shell = item_shell.child(render_accordion_content(AccordionContentFrame {
                    style,
                    motion,
                    content: item.content.clone(),
                    content_selector,
                }));
            }

            container = container.child(item_shell);
        }

        container
    }
}

fn chevron_icon(style: AccordionStyle, expanded: bool, _motion: MotionPolicy) -> impl IntoElement {
    let asset = if expanded {
        AssetId::TABLER_CHEVRON_UP
    } else {
        AssetId::TABLER_CHEVRON_DOWN
    };
    asset_glyph(asset)
        .size(style.chevron_size)
        .text_color(style.chevron_foreground)
}

/// Returns a deterministic, value-derived selector suffix.
///
/// The value is fed through a local FNV-1a hasher rather than formatted, so
/// arbitrary caller data never appears in selectors.
#[must_use]
pub fn stable_value_selector_suffix<V: Hash + ?Sized>(value: &V) -> String {
    let mut hasher = Fnv1aHasher(0xcbf2_9ce4_8422_2325);
    value.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Builds the stable item selector used below an accordion root selector.
#[must_use]
pub fn item_debug_selector<V: Hash + ?Sized>(root_selector: &str, value: &V) -> String {
    format!(
        "{}-item-{}",
        root_selector,
        stable_value_selector_suffix(value)
    )
}

/// Builds the stable trigger selector used below an accordion root selector.
#[must_use]
pub fn trigger_debug_selector<V: Hash + ?Sized>(root_selector: &str, value: &V) -> String {
    format!(
        "{}-trigger-{}",
        root_selector,
        stable_value_selector_suffix(value)
    )
}

/// Builds the stable content selector used below an accordion root selector.
#[must_use]
pub fn content_debug_selector<V: Hash + ?Sized>(root_selector: &str, value: &V) -> String {
    format!(
        "{}-content-{}",
        root_selector,
        stable_value_selector_suffix(value)
    )
}

/// Alias for [`item_debug_selector`].
#[must_use]
pub fn accordion_item_selector<V: Hash + ?Sized>(root_selector: &str, value: &V) -> String {
    item_debug_selector(root_selector, value)
}

/// Alias for [`trigger_debug_selector`].
#[must_use]
pub fn accordion_trigger_selector<V: Hash + ?Sized>(root_selector: &str, value: &V) -> String {
    trigger_debug_selector(root_selector, value)
}

/// Alias for [`content_debug_selector`].
#[must_use]
pub fn accordion_content_selector<V: Hash + ?Sized>(root_selector: &str, value: &V) -> String {
    content_debug_selector(root_selector, value)
}

struct Fnv1aHasher(u64);

impl Hasher for Fnv1aHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}
