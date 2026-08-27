//! Controlled segmented tabs for the reached model-selector surfaces.
//!
//! GPUI does not expose the browser's tab/ARIA tree or a browser-style
//! per-trigger focus model. This primitive therefore keeps one caller-owned
//! focus handle on the list and implements the reached roving behavior in the
//! list's key handler. The selected value is always a render input: navigation
//! may remember which trigger is being roved over for the lifetime of one
//! rendered tree, but it never changes the selected value or invents a second
//! source of truth.

use std::cell::Cell;
use std::rc::Rc;

use gpui::prelude::{FluentBuilder, Refineable};
use gpui::{
    App, ClickEvent, Div, ElementId, FocusHandle, FontWeight, Hsla, InteractiveElement,
    IntoElement, Modifiers, ParentElement, Pixels, RenderOnce, SharedString, Stateful,
    StatefulInteractiveElement, StyleRefinement, Styled, Window, div, px, transparent_black,
};

use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens};

/// The two reached tabs-list visual recipes.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum TabsVariant {
    /// A muted pill containing the active trigger.
    #[default]
    Default,
    /// A transparent list with an orientation-aligned active underline.
    Line,
}

/// The axis used by the list and by roving arrow-key navigation.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum TabsOrientation {
    /// Triggers are laid out left-to-right and left/right keys navigate.
    #[default]
    Horizontal,
    /// Triggers are laid out top-to-bottom and up/down keys navigate.
    Vertical,
}

/// One stable, ordered tab option.
///
/// Values are retained separately from visible labels so callers can use
/// stable model identifiers while changing presentation text. Values should
/// be unique within one list; they also form the deterministic child element
/// IDs and debug-selector suffixes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TabSpec {
    /// Stable value sent to the controlled change callback.
    pub value: SharedString,
    /// Visible one-line trigger label.
    pub label: SharedString,
    /// Whether this trigger is skipped and cannot be activated.
    pub disabled: bool,
}

impl TabSpec {
    /// Creates an enabled tab from its stable value and visible label.
    #[must_use]
    pub fn new(value: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            disabled: false,
        }
    }

    /// Sets whether this tab is disabled.
    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Returns the stable value as text.
    #[must_use]
    pub fn value(&self) -> &str {
        self.value.as_str()
    }

    /// Returns the visible label as text.
    #[must_use]
    pub fn label(&self) -> &str {
        self.label.as_str()
    }

    /// Returns whether this tab is disabled.
    #[must_use]
    pub const fn is_disabled(&self) -> bool {
        self.disabled
    }
}

/// Theme-resolved geometry and paint for one tabs configuration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TabsStyle {
    /// Horizontal list padding: 3 px on every side.
    pub list_padding: Pixels,
    /// List height for horizontal tabs; vertical lists size to their content.
    pub list_height: Option<Pixels>,
    /// Gap between list children.
    pub list_gap: Pixels,
    /// Outer list corner radius.
    pub list_corner_radius: Pixels,
    /// List background.
    pub list_background: Hsla,
    /// List's inherited muted foreground.
    pub list_foreground: Hsla,
    /// Trigger horizontal padding.
    pub trigger_horizontal_padding: Pixels,
    /// Trigger vertical padding.
    pub trigger_vertical_padding: Pixels,
    /// Gap between trigger content items.
    pub trigger_gap: Pixels,
    /// Base trigger corner radius.
    pub trigger_corner_radius: Pixels,
    /// Active default-trigger corner radius.
    pub active_corner_radius: Pixels,
    /// Trigger text size.
    pub trigger_text_size: Pixels,
    /// Trigger line height.
    pub trigger_line_height: Pixels,
    /// Trigger text weight.
    pub trigger_weight: FontWeight,
    /// Inactive trigger foreground.
    pub inactive_foreground: Hsla,
    /// Active or hovered trigger foreground.
    pub active_foreground: Hsla,
    /// Active default-trigger background; transparent for the line recipe.
    pub active_background: Hsla,
    /// Active line indicator color.
    pub indicator_color: Hsla,
    /// Active line indicator thickness.
    pub indicator_thickness: Pixels,
    /// Opacity used for disabled presentation.
    pub disabled_opacity: f32,
}

impl TabsStyle {
    /// Resolves the reached tabs recipe from shared theme tokens.
    #[must_use]
    pub fn resolve(
        theme: ArtisanTheme,
        variant: TabsVariant,
        orientation: TabsOrientation,
    ) -> Self {
        let list_corner_radius = match (variant, orientation) {
            (TabsVariant::Default, TabsOrientation::Horizontal) => {
                RadiusTokens::value(RadiusStep::X4l)
            }
            (TabsVariant::Default, TabsOrientation::Vertical) => {
                RadiusTokens::value(RadiusStep::X2l)
            }
            (TabsVariant::Line, _) => px(0.0),
        };
        let list_background = match variant {
            TabsVariant::Default => theme.colors.muted.to_paint(),
            TabsVariant::Line => transparent_black(),
        };
        let active_background = match variant {
            TabsVariant::Default => theme.colors.background.to_paint(),
            TabsVariant::Line => transparent_black(),
        };

        Self {
            list_padding: theme.density.tabs_list_padding,
            list_height: match orientation {
                TabsOrientation::Horizontal => Some(theme.density.tabs_list_height),
                TabsOrientation::Vertical => None,
            },
            list_gap: match variant {
                TabsVariant::Default => px(0.0),
                TabsVariant::Line => theme.spacing.steps(1.0),
            },
            list_corner_radius,
            list_background,
            list_foreground: theme.colors.muted_foreground.to_paint(),
            trigger_horizontal_padding: theme.spacing.steps(2.0),
            trigger_vertical_padding: theme.spacing.steps(1.0),
            trigger_gap: theme.spacing.steps(1.5),
            trigger_corner_radius: RadiusTokens::value(RadiusStep::Xl),
            active_corner_radius: px(12.0),
            trigger_text_size: theme.typography.control_text,
            trigger_line_height: theme.spacing.steps(5.0),
            trigger_weight: FontWeight::MEDIUM,
            inactive_foreground: theme.colors.foreground.with_alpha(0.6).to_paint(),
            active_foreground: theme.colors.foreground.to_paint(),
            active_background,
            indicator_color: theme.colors.primary.to_paint(),
            indicator_thickness: px(2.0),
            disabled_opacity: 0.5,
        }
    }
}

type ChangeHandler = Rc<dyn Fn(SharedString, &ClickEvent, &mut Window, &mut App)>;

/// A controlled native GPUI segmented-tabs component.
#[derive(IntoElement)]
pub struct Tabs {
    id: ElementId,
    focus: FocusHandle,
    theme: ArtisanTheme,
    selected: SharedString,
    specs: Vec<TabSpec>,
    variant: TabsVariant,
    orientation: TabsOrientation,
    disabled: bool,
    on_change: Option<ChangeHandler>,
    debug_prefix: Option<SharedString>,
    list: Div,
}

impl Tabs {
    /// Creates a controlled tabs list from a finite ordered collection.
    #[must_use]
    pub fn new(
        id: impl Into<ElementId>,
        focus: FocusHandle,
        theme: ArtisanTheme,
        selected: impl Into<SharedString>,
        tabs: impl IntoIterator<Item = TabSpec>,
    ) -> Self {
        Self {
            id: id.into(),
            focus,
            theme,
            selected: selected.into(),
            specs: tabs.into_iter().collect(),
            variant: TabsVariant::default(),
            orientation: TabsOrientation::default(),
            disabled: false,
            on_change: None,
            debug_prefix: None,
            list: div(),
        }
    }

    /// Selects the list visual variant.
    #[must_use]
    pub const fn variant(mut self, variant: TabsVariant) -> Self {
        self.variant = variant;
        self
    }

    /// Selects the list orientation and its arrow-key axis.
    #[must_use]
    pub const fn orientation(mut self, orientation: TabsOrientation) -> Self {
        self.orientation = orientation;
        self
    }

    /// Disables the complete list and suppresses every activation.
    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Installs the controlled change callback.
    #[must_use]
    pub fn on_change(
        mut self,
        handler: impl Fn(SharedString, &ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }

    /// Adds a stable prefix for list, trigger, and value debug selectors.
    ///
    /// With prefix model-tabs, generated selectors are
    /// model-tabs-list, model-tabs-trigger-<value>, and
    /// model-tabs-value-<value>.
    #[must_use]
    pub fn debug_selector(mut self, prefix: impl Into<SharedString>) -> Self {
        self.debug_prefix = Some(prefix.into());
        self
    }

    /// Returns the controlled value retained by this render.
    #[must_use]
    pub fn selected_value(&self) -> &str {
        self.selected.as_str()
    }

    /// Returns the ordered tab specifications retained by this render.
    #[must_use]
    pub fn tab_specs(&self) -> &[TabSpec] {
        &self.specs
    }

    /// Returns the resolved visual recipe for this configuration.
    #[must_use]
    pub fn visual_style(&self) -> TabsStyle {
        TabsStyle::resolve(self.theme, self.variant, self.orientation)
    }
}

impl Styled for Tabs {
    fn style(&mut self) -> &mut StyleRefinement {
        self.list.style()
    }
}

impl RenderOnce for Tabs {
    fn render(mut self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let style = self.visual_style();
        let orientation = self.orientation;
        let variant = self.variant;
        let disabled = self.disabled;
        let focus = self.focus.clone();
        let selected = self.selected.clone();
        let tabs = self.specs;
        let focused_index = Rc::new(Cell::new(initial_focus_index(&tabs, &selected)));
        let on_change = self.on_change;

        let mut list = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .gap(style.list_gap)
            .p(style.list_padding)
            .rounded(style.list_corner_radius)
            .bg(style.list_background)
            .text_color(style.list_foreground)
            .when(orientation == TabsOrientation::Horizontal, |element| {
                element.flex_row()
            })
            .when(orientation == TabsOrientation::Vertical, |element| {
                element.flex_col()
            })
            .when_some(style.list_height, Styled::h);

        let caller_style = self.list.style().clone();
        list.style().refine(&caller_style);

        let list_id = self.id.clone();
        let mut list = list.id(self.id);
        if let Some(prefix) = self.debug_prefix.as_ref() {
            let selector = format!("{prefix}-list");
            list = list.debug_selector(move || selector);
        }

        if !disabled {
            list = attach_keyboard_handler(
                list,
                &focus,
                orientation,
                tabs.clone(),
                selected.clone(),
                focused_index.clone(),
                on_change.clone(),
            );
        }

        let trigger_context = TriggerContext {
            tabs: &tabs,
            style,
            orientation,
            variant,
            disabled,
            selected: &selected,
            focused_index: &focused_index,
            on_change: on_change.as_ref(),
            debug_prefix: self.debug_prefix.as_ref(),
            list_id: &list_id,
        };

        for (index, tab) in tabs.iter().enumerate() {
            let trigger = render_trigger(tab, index, &trigger_context);
            list = list.child(trigger);
        }

        if disabled {
            list.opacity(style.disabled_opacity)
        } else {
            list
        }
    }
}

fn attach_keyboard_handler(
    list: Stateful<Div>,
    focus: &FocusHandle,
    orientation: TabsOrientation,
    tabs: Vec<TabSpec>,
    selected: SharedString,
    focused_index: Rc<Cell<Option<usize>>>,
    on_change: Option<ChangeHandler>,
) -> Stateful<Div> {
    list.track_focus(focus)
        .on_key_down(move |event, window, cx| {
            if event.keystroke.modifiers != Modifiers::none() {
                return;
            }

            let key = event.keystroke.key.as_str();
            let direction = match (orientation, key) {
                (TabsOrientation::Horizontal, "left" | "arrowleft")
                | (TabsOrientation::Vertical, "up" | "arrowup") => Some(false),
                (TabsOrientation::Horizontal, "right" | "arrowright")
                | (TabsOrientation::Vertical, "down" | "arrowdown") => Some(true),
                _ => None,
            };

            if let Some(forward) = direction {
                window.prevent_default();
                let next = adjacent_enabled_index(&tabs, focused_index.get(), forward);
                focused_index.set(next);
                emit_activation(
                    &tabs,
                    &selected,
                    next,
                    on_change.as_ref(),
                    &ClickEvent::default(),
                    window,
                    cx,
                );
                return;
            }

            if matches!(key, "home" | "start" | "end") {
                window.prevent_default();
                let next = if key == "end" {
                    last_enabled_index(&tabs)
                } else {
                    first_enabled_index(&tabs)
                };
                focused_index.set(next);
                emit_activation(
                    &tabs,
                    &selected,
                    next,
                    on_change.as_ref(),
                    &ClickEvent::default(),
                    window,
                    cx,
                );
                return;
            }

            if matches!(key, "enter" | "return" | "space") {
                window.prevent_default();
                emit_activation(
                    &tabs,
                    &selected,
                    focused_index.get(),
                    on_change.as_ref(),
                    &ClickEvent::default(),
                    window,
                    cx,
                );
            }
        })
}

fn tab_trigger_id(list_id: &ElementId, value: &SharedString) -> ElementId {
    ElementId::NamedChild(Box::new(list_id.clone()), format!("tab-{value}").into())
}

struct TriggerContext<'a> {
    tabs: &'a [TabSpec],
    style: TabsStyle,
    orientation: TabsOrientation,
    variant: TabsVariant,
    disabled: bool,
    selected: &'a SharedString,
    focused_index: &'a Rc<Cell<Option<usize>>>,
    on_change: Option<&'a ChangeHandler>,
    debug_prefix: Option<&'a SharedString>,
    list_id: &'a ElementId,
}

fn render_trigger(tab: &TabSpec, index: usize, context: &TriggerContext<'_>) -> Stateful<Div> {
    let style = context.style;
    let orientation = context.orientation;
    let active = tab.value.as_str() == context.selected.as_str();
    let trigger_disabled = context.disabled || tab.disabled;
    let mut trigger = div()
        .flex()
        .flex_row()
        .flex_1()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .gap(style.trigger_gap)
        .px(style.trigger_horizontal_padding)
        .py(style.trigger_vertical_padding)
        .rounded(if active {
            style.active_corner_radius
        } else {
            style.trigger_corner_radius
        })
        .border_1()
        .border_color(transparent_black())
        .bg(if active {
            style.active_background
        } else {
            transparent_black()
        })
        .text_color(if active {
            style.active_foreground
        } else {
            style.inactive_foreground
        })
        .text_size(style.trigger_text_size)
        .line_height(style.trigger_line_height)
        .font_weight(style.trigger_weight)
        .whitespace_nowrap()
        .when(orientation == TabsOrientation::Vertical, |element| {
            element.w_full().justify_start()
        })
        .when(tab.disabled && !context.disabled, |element| {
            element.opacity(style.disabled_opacity)
        })
        .id(tab_trigger_id(context.list_id, &tab.value));

    if let Some(prefix) = context.debug_prefix {
        let selector = format!("{prefix}-trigger-{}", tab.value);
        trigger = trigger.debug_selector(move || selector);
    }

    let mut value = div()
        .flex_shrink_0()
        .whitespace_nowrap()
        .child(tab.label.clone());
    if let Some(prefix) = context.debug_prefix {
        let selector = format!("{prefix}-value-{}", tab.value);
        value = value.debug_selector(move || selector);
    }
    trigger = trigger.child(value);

    if active && context.variant == TabsVariant::Line {
        trigger = trigger.relative().child(line_indicator(style, orientation));
    }

    if !trigger_disabled {
        let trigger_index = Rc::clone(context.focused_index);
        let trigger_selected = context.selected.clone();
        let trigger_tabs = context.tabs.to_vec();
        let trigger_on_change = context.on_change.cloned();
        trigger = trigger.on_click(move |event, window, cx| {
            trigger_index.set(Some(index));
            emit_activation(
                &trigger_tabs,
                &trigger_selected,
                Some(index),
                trigger_on_change.as_ref(),
                event,
                window,
                cx,
            );
        });
        trigger = trigger.hover(move |hovered| hovered.text_color(style.active_foreground));
    }

    trigger
}

/// Builds the line variant's orientation-aligned active indicator.
fn line_indicator(style: TabsStyle, orientation: TabsOrientation) -> Div {
    div()
        .absolute()
        .bg(style.indicator_color)
        .when(orientation == TabsOrientation::Horizontal, |element| {
            element
                .left(px(0.0))
                .right(px(0.0))
                .bottom(px(0.0))
                .h(style.indicator_thickness)
        })
        .when(orientation == TabsOrientation::Vertical, |element| {
            element
                .top(px(0.0))
                .bottom(px(0.0))
                .right(px(0.0))
                .w(style.indicator_thickness)
        })
}

fn first_enabled_index(tabs: &[TabSpec]) -> Option<usize> {
    tabs.iter().position(|tab| !tab.disabled)
}

fn last_enabled_index(tabs: &[TabSpec]) -> Option<usize> {
    tabs.iter().rposition(|tab| !tab.disabled)
}

fn initial_focus_index(tabs: &[TabSpec], selected: &SharedString) -> Option<usize> {
    tabs.iter()
        .position(|tab| !tab.disabled && tab.value.as_str() == selected.as_str())
        .or_else(|| first_enabled_index(tabs))
}

fn adjacent_enabled_index(
    tabs: &[TabSpec],
    current: Option<usize>,
    forward: bool,
) -> Option<usize> {
    let first = first_enabled_index(tabs)?;
    let last = last_enabled_index(tabs)?;
    let current = current
        .filter(|index| *index < tabs.len() && !tabs[*index].disabled)
        .unwrap_or(first);
    let len = tabs.len();

    for offset in 1..=len {
        let candidate = if forward {
            (current + offset) % len
        } else {
            (current + len - offset) % len
        };
        if !tabs[candidate].disabled {
            return Some(candidate);
        }
    }

    Some(if forward { first } else { last })
}

fn emit_activation(
    tabs: &[TabSpec],
    selected: &SharedString,
    index: Option<usize>,
    on_change: Option<&ChangeHandler>,
    event: &ClickEvent,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(index) = index else {
        return;
    };
    let Some(tab) = tabs.get(index) else {
        return;
    };
    if tab.disabled || tab.value.as_str() == selected.as_str() {
        return;
    }
    let Some(on_change) = on_change else {
        return;
    };
    on_change(tab.value.clone(), event, window, cx);
}
