//! Controlled native select/listbox primitives for Artisan settings surfaces.
//!
//! `SelectState` is independent from GPUI. It owns highlight, controlled-open
//! reconciliation, and printable typeahead; Select turns those transitions
//! into the existing Artisan theme, icon, focus, and deferred-overlay recipes.
//!
//! GPUI does not currently expose a platform accessibility tree. The render
//! recipe therefore keeps stable role, label, and selection metadata in public
//! semantic records without pretending those records are native accessibility
//! attributes.

#![forbid(unsafe_code)]

mod render;
mod state;

use self::render::selected_index;

pub use self::render::{item_debug_selector, stable_value_selector_suffix};
pub use self::state::{
    SelectAction, SelectEntry, SelectItem, SelectKey, SelectScrollEdges, SelectScrollState,
    SelectSize, SelectState, SelectValue,
};

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    App, BoxShadow, ClickEvent, ElementId, FocusHandle, Pixels, ScrollHandle, SharedString, Window,
    px, transparent_black,
};

use crate::motion::SELECT_TYPEAHEAD;
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens, SurfaceStep, ThemeMode};
/// Stable selector for an unprefixed select trigger.
pub const SELECT_TRIGGER_SELECTOR: &str = "artisan-select-trigger";
/// Stable selector for an unprefixed select content layer.
pub const SELECT_CONTENT_SELECTOR: &str = "artisan-select-content";
/// Stable selector for an unprefixed select viewport.
pub const SELECT_VIEWPORT_SELECTOR: &str = "artisan-select-viewport";

/// Printable typeahead remains active for this long between keystrokes.
pub const TYPEAHEAD_TIMEOUT: Duration = SELECT_TYPEAHEAD;

const DISABLED_OPACITY: f32 = 0.5;

/// Theme-resolved geometry and paint for a Select.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectStyle {
    /// Trigger height.
    pub trigger_height: Pixels,
    /// Trigger horizontal padding.
    pub trigger_horizontal_padding: Pixels,
    /// Gap between the label and selector glyph.
    pub trigger_gap: Pixels,
    /// Trigger pill radius.
    pub trigger_corner_radius: Pixels,
    /// Trigger resting background.
    pub trigger_background: gpui::Hsla,
    /// Trigger foreground.
    pub trigger_foreground: gpui::Hsla,
    /// Placeholder foreground.
    pub placeholder_foreground: gpui::Hsla,
    /// Trigger border.
    pub trigger_border: gpui::Hsla,
    /// Trigger focus border.
    pub focus_border: gpui::Hsla,
    /// Trigger focus ring paint.
    pub focus_ring: gpui::Hsla,
    /// Trigger focus ring spread.
    pub focus_ring_width: Pixels,
    /// Popover minimum width.
    pub content_min_width: Pixels,
    /// Popover and viewport maximum height.
    pub content_max_height: Pixels,
    /// Popover corner radius.
    pub content_corner_radius: Pixels,
    /// Popover background.
    pub content_background: gpui::Hsla,
    /// Popover foreground.
    pub content_foreground: gpui::Hsla,
    /// Popover shadow layers.
    pub content_shadow: [BoxShadow; 1],
    /// Item horizontal padding.
    pub item_horizontal_padding: Pixels,
    /// Item vertical padding.
    pub item_vertical_padding: Pixels,
    /// Gap between a check indicator and label.
    pub item_gap: Pixels,
    /// Item corner radius.
    pub item_corner_radius: Pixels,
    /// Resting item background.
    pub item_background: gpui::Hsla,
    /// Resting item foreground.
    pub item_foreground: gpui::Hsla,
    /// Highlighted item background.
    pub item_highlight_background: gpui::Hsla,
    /// Highlighted item foreground.
    pub item_highlight_foreground: gpui::Hsla,
    /// Group heading foreground.
    pub group_label_foreground: gpui::Hsla,
    /// Group heading text size.
    pub group_label_text_size: Pixels,
    /// Separator paint.
    pub separator_color: gpui::Hsla,
    /// Scroll-edge button height.
    pub scroll_button_height: Pixels,
    /// Shared control text size.
    pub text_size: Pixels,
    /// Disabled item opacity.
    pub disabled_opacity: f32,
}

impl SelectStyle {
    /// Resolves the select recipe from the shared theme and requested size.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme, size: SelectSize) -> Self {
        let trigger_height = match size {
            SelectSize::Default => theme.density.control_default,
            SelectSize::Small => theme.density.control_sm,
        };
        let trigger_background = match theme.mode {
            ThemeMode::Light => theme.surfaces.value(SurfaceStep::S100),
            ThemeMode::Dark => theme.surfaces.value(SurfaceStep::S900),
        }
        .to_paint();

        Self {
            trigger_height,
            trigger_horizontal_padding: px(12.0),
            trigger_gap: theme.spacing.steps(1.5),
            trigger_corner_radius: RadiusTokens::value(RadiusStep::X4l),
            trigger_background,
            trigger_foreground: theme.colors.foreground.to_paint(),
            placeholder_foreground: theme.colors.muted_foreground.to_paint(),
            trigger_border: theme.colors.input.to_paint(),
            focus_border: theme.colors.ring.to_paint(),
            focus_ring: theme.interaction.focus_ring_color.to_paint(),
            focus_ring_width: theme.interaction.focus_ring_width,
            content_min_width: theme.spacing.steps(36.0),
            content_max_height: theme.density.command_list_max_height,
            content_corner_radius: RadiusTokens::value(RadiusStep::X2l),
            content_background: theme.colors.popover.to_paint(),
            content_foreground: theme.colors.popover_foreground.to_paint(),
            content_shadow: theme
                .elevation
                .menu_shadow
                .map(super::theme::ShadowLayer::to_box_shadow),
            item_horizontal_padding: theme.spacing.steps(3.0),
            item_vertical_padding: theme.spacing.steps(2.0),
            item_gap: theme.spacing.steps(2.5),
            item_corner_radius: RadiusTokens::value(RadiusStep::Xl),
            item_background: transparent_black(),
            item_foreground: theme.colors.popover_foreground.to_paint(),
            item_highlight_background: theme.colors.accent.to_paint(),
            item_highlight_foreground: theme.colors.accent_foreground.to_paint(),
            group_label_foreground: theme.colors.muted_foreground.to_paint(),
            group_label_text_size: theme.typography.label_text,
            separator_color: theme.colors.border.with_alpha(0.5).to_paint(),
            scroll_button_height: theme.spacing.steps(6.0),
            text_size: theme.typography.control_text,
            disabled_opacity: DISABLED_OPACITY,
        }
    }

    /// Returns the resolved trigger height.
    #[must_use]
    pub const fn height(&self) -> Pixels {
        self.trigger_height
    }
}

/// Change notification for a committed select value.
pub type SelectChangeHandler<V> = Rc<dyn Fn(V, Option<&ClickEvent>, &mut Window, &mut App)>;

/// Change notification for controlled open updates.
pub type SelectOpenChangeHandler = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// Semantic trigger/listbox snapshot for inspectable metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectSemanticState {
    /// Fixed trigger role name.
    pub trigger_role: &'static str,
    /// Fixed listbox role name.
    pub listbox_role: &'static str,
    /// Fixed option role name.
    pub option_role: &'static str,
    /// Whether the listbox is currently expanded.
    pub expanded: bool,
    /// Whether the whole control is disabled.
    pub disabled: bool,
    /// Visible trigger text.
    pub label: SharedString,
    /// Whether the trigger shows the placeholder.
    pub placeholder: bool,
    /// Visible label of the committed value, if any.
    pub value_label: Option<SharedString>,
    /// Stable trigger selector.
    pub trigger_selector: String,
    /// Stable content selector.
    pub content_selector: String,
}

/// Semantic record for one rendered option row.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SelectItemSemanticState {
    /// Fixed option role name.
    pub role: &'static str,
    /// Visible option label.
    pub label: SharedString,
    /// Whether this row holds the committed value.
    pub selected: bool,
    /// Whether this row holds the roving highlight.
    pub highlighted: bool,
    /// Whether this row can be activated.
    pub disabled: bool,
    /// Stable per-value selector.
    pub selector: String,
}

/// A controlled native select/listbox view.
///
/// The caller owns `open` and the committed value; interaction state flows
/// through an optional shared [`SelectState`]. Rendering reconciles the
/// shared state with the controlled props every frame, so owner-applied
/// updates always win while in-progress keyboard motion survives renders
/// that change nothing.
#[derive(gpui::IntoElement)]
pub struct Select<V: SelectValue = SharedString> {
    id: ElementId,
    focus: FocusHandle,
    theme: ArtisanTheme,
    selected: Option<V>,
    open: bool,
    placeholder: SharedString,
    entries: Vec<SelectEntry<V>>,
    size: SelectSize,
    disabled: bool,
    scroll_state: SelectScrollState,
    scroll_handle: ScrollHandle,
    on_change: Option<SelectChangeHandler<V>>,
    on_open_change: Option<SelectOpenChangeHandler>,
    debug_selector: Option<SharedString>,
    interaction_state: Option<Rc<RefCell<SelectState>>>,
}

impl<V: SelectValue> Select<V> {
    /// Creates a select with an explicit committed value.
    pub fn new(
        id: impl Into<ElementId>,
        focus: FocusHandle,
        theme: ArtisanTheme,
        selected: Option<V>,
        entries: Vec<SelectEntry<V>>,
    ) -> Self {
        Self {
            id: id.into(),
            focus,
            theme,
            selected,
            open: false,
            placeholder: SharedString::from(String::new()),
            entries,
            size: SelectSize::default(),
            disabled: false,
            scroll_state: SelectScrollState::default(),
            scroll_handle: ScrollHandle::new(),
            on_change: None,
            on_open_change: None,
            debug_selector: None,
            interaction_state: None,
        }
    }

    /// Sets the controlled open state.
    #[must_use]
    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    /// Sets the placeholder shown without a committed value.
    #[must_use]
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Adds a stable selector; trigger, content, and viewport derive
    /// `{selector}-trigger`, `{selector}-content`, and `{selector}-viewport`.
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.debug_selector = Some(selector.into());
        self
    }

    /// Selects the trigger size.
    #[must_use]
    pub fn size(mut self, size: SelectSize) -> Self {
        self.size = size;
        self
    }

    /// Sets the whole-control disabled state.
    #[must_use]
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Sets the caller-owned scroll-edge state.
    #[must_use]
    pub fn scroll_state(mut self, scroll_state: SelectScrollState) -> Self {
        self.scroll_state = scroll_state;
        self
    }

    /// Shares deterministic interaction state across renders.
    #[must_use]
    pub fn with_interaction_state(mut self, state: Rc<RefCell<SelectState>>) -> Self {
        self.interaction_state = Some(state);
        self
    }

    /// Installs the committed-value notification.
    #[must_use]
    pub fn on_change(
        mut self,
        handler: impl Fn(V, Option<&ClickEvent>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }

    /// Installs the controlled open-update notification.
    #[must_use]
    pub fn on_open_change(
        mut self,
        handler: impl Fn(bool, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_open_change = Some(Rc::new(handler));
        self
    }

    /// Returns the committed entries.
    #[must_use]
    pub fn entries(&self) -> &[SelectEntry<V>] {
        &self.entries
    }

    /// Returns the committed value's visible label, if a row holds it.
    #[must_use]
    pub fn current_label(&self) -> Option<&str> {
        selected_index(&self.entries, self.selected.as_ref())
            .and_then(|index| self.entries.get(index))
            .and_then(|entry| entry.as_item())
            .map(SelectItem::label)
    }

    /// Returns the trigger text: committed label or placeholder.
    #[must_use]
    pub fn display_label(&self) -> String {
        self.current_label()
            .unwrap_or_else(|| self.placeholder.as_ref())
            .to_owned()
    }

    /// Resolves the trigger recipe for the requested size.
    #[must_use]
    pub fn visual_style(&self) -> SelectStyle {
        SelectStyle::resolve(self.theme, self.size)
    }

    /// Returns the caller-owned scroll-edge state.
    #[must_use]
    pub const fn scroll_state_value(&self) -> SelectScrollState {
        self.scroll_state
    }

    /// Returns the semantic trigger/listbox snapshot.
    #[must_use]
    pub fn semantic_state(&self) -> SelectSemanticState {
        let value_label = self
            .current_label()
            .map(|label| SharedString::from(label.to_owned()));
        let root = self.selector_root();
        SelectSemanticState {
            trigger_role: "combobox",
            listbox_role: "listbox",
            option_role: "option",
            expanded: self.open && !self.disabled,
            disabled: self.disabled,
            label: SharedString::from(self.display_label()),
            placeholder: value_label.is_none(),
            value_label,
            trigger_selector: format!("{root}-trigger"),
            content_selector: format!("{root}-content"),
        }
    }

    /// Returns semantic item records in visual item order.
    #[must_use]
    pub fn item_semantics(&self, state: &SelectState) -> Vec<SelectItemSemanticState> {
        let root = self.selector_root();
        self.entries
            .iter()
            .filter_map(|entry| match entry {
                SelectEntry::Item(item) => Some(SelectItemSemanticState {
                    role: "option",
                    label: item.label.clone(),
                    selected: self
                        .selected
                        .as_ref()
                        .is_some_and(|selected| item.value() == selected),
                    highlighted: state.highlighted_index().is_some_and(|highlighted| {
                        self.entries.get(highlighted).is_some_and(|row| {
                            matches!(row, SelectEntry::Item(row) if row.value() == item.value())
                        })
                    }),
                    disabled: self.disabled || item.is_disabled(),
                    selector: item_debug_selector(&root, item.value()),
                }),
                SelectEntry::Group(_) | SelectEntry::Separator => None,
            })
            .collect()
    }

    fn selector_root(&self) -> String {
        self.debug_selector
            .as_ref()
            .map_or_else(|| "artisan-select".to_owned(), ToString::to_string)
    }
}
