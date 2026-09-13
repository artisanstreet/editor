//! Reusable GPUI model-selector popover for the native composer.
//!
//! The selector mirrors the Electron model picker at the presentation
//! boundary: the full static catalog remains readable when disconnected, and
//! every mutation is an explicit event for the application owner. Runtime
//! harness readiness is deliberately outside this picker; the run boundary
//! checks it when a policy is used. Persistence and backend authority stay
//! outside this entity.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::{cell::RefCell, ops::Range, rc::Rc, sync::Arc, time::Duration};

use artisan_assets::AssetId;
use artisan_ui::{
    gradient::{hover_fill_gradient, vertical_gradient},
    icon::{IconSize, IconStyle, IconTint, icon},
    motion::MotionCurve,
    theme::{ArtisanTheme, SurfaceStep, ThemeMode},
};
use gpui::{
    Anchor, Animation, AnimationExt as _, AnyElement, App, Bounds, ClickEvent, Context, Div,
    ElementId, EventEmitter, FocusHandle, Focusable, FontWeight, HighlightStyle, Hsla, ImageSource,
    InteractiveElement as _, KeyDownEvent, MouseDownEvent, ParentElement as _, Pixels, Point,
    Render, RenderImage, ScrollHandle, ScrollWheelEvent, SharedString, Size, Stateful,
    StatefulInteractiveElement as _, Styled as _, StyledText, Task, Window, anchored, canvas,
    deferred, div, img, point, prelude::FluentBuilder as _, prelude::IntoElement, px, rgb,
    rgb_to_hsla,
};

use crate::engine_section_indicator_policy::{
    EngineSectionIndicatorMeasurement, EngineSectionIndicatorPolicy,
};
use crate::native_composer_material::{
    GlassStrength, card_shadows, glass_blur_radius, glass_card_shadows, glass_foreground_base,
    glass_highlight_layer, glass_material_layer,
};
use crate::native_model_catalog::{
    NativeModelCatalog, NativeModelDefinition, NativeModelPolicy, NativeModelView,
    NativeOptionValue, NativePolicyValidationError, NativeThinkingCapability,
};
use crate::speed_presentation::SpeedGradient;

#[path = "native_picker_motion.rs"]
mod native_picker_motion;
pub(crate) use self::native_picker_motion::{
    HoverRect, PickerMenuMotion, PickerMenuPhase, PickerScrollState, SlidingHoverState,
};

// Phase-1 split submodules (see native_model_selector/).

#[path = "native_model_selector/state.rs"]
mod state;

#[path = "native_model_selector/interaction.rs"]
mod interaction;

#[path = "native_model_selector/render.rs"]
mod render;

#[cfg(test)]
#[path = "native_model_selector/tests.rs"]
mod tests;

// Re-exports keep the public surface identical to the pre-split module.
pub(crate) use render::{
    animate_picker_menu, engine_accent, engine_asset, render_picker_hover_pill,
};
pub use state::model_display_label;

/// Stable selector painted on the compact composer trigger.
pub const NATIVE_MODEL_SELECTOR_TRIGGER_SELECTOR: &str = "artisan-native-model-selector-trigger";
/// Stable selector painted on the open selector panel.
pub const NATIVE_MODEL_SELECTOR_MENU_SELECTOR: &str = "artisan-native-model-selector-menu";
/// Prefix for model-row debug selectors.
pub const NATIVE_MODEL_SELECTOR_ROW_SELECTOR_PREFIX: &str = "artisan-native-model-selector-row";
/// Prefix for engine-tab debug selectors.
pub const NATIVE_MODEL_SELECTOR_ENGINE_SELECTOR_PREFIX: &str =
    "artisan-native-model-selector-engine";

const MENU_WIDTH_PX: f32 = 480.0;
const MENU_MAX_HEIGHT_PX: f32 = 320.0;
const MENU_VIEWPORT_INSET_X_PX: f32 = 32.0;
const MENU_VIEWPORT_INSET_Y_PX: f32 = 24.0;
const MENU_GAP_PX: f32 = 8.0;
const DROPDOWN_GAP_PX: f32 = 4.0;
const DROPDOWN_MIN_WIDTH_PX: f32 = 144.0;
const MODEL_PANEL_HEIGHT_PX: f32 = 192.0;
const MODEL_PREVIEW_WIDTH_PX: f32 = 224.0;
const MODEL_ROW_HEIGHT_PX: f32 = 48.0;
const COMPACT_CONTROL_HEIGHT_PX: f32 = 32.0;
const POLICY_CONTROL_HEIGHT_PX: f32 = 24.0;
pub(crate) const PICKER_MENU_MOTION_DURATION_MS: u64 = 100;
const PICKER_HOVER_MOTION_DURATION_MS: u64 = 250;
const PICKER_TOOLTIP_SHOW_DELAY_MS: u64 = 500;
const OPTION_TOOLTIP_WIDTH_PX: f32 = 320.0;
const OPTION_TOOLTIP_GAP_PX: f32 = 8.0;

/// The policy payload emitted after a model or policy control is committed.
pub type SelectPolicy = NativeModelPolicy;

/// The explicit favorite intent emitted by the selector.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetFavorite {
    /// Exact catalog model ID.
    pub model_id: String,
    /// Desired final favorite state.
    pub favorite: bool,
}

/// Events consumed by the application owner.
#[expect(
    clippy::large_enum_variant,
    reason = "the selection event carries the complete owned policy to the application owner; boxing would add an allocation per model selection for a short-lived event"
)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeModelSelectorEvent {
    /// Refresh host capabilities when the menu opens.
    RefreshCatalog,
    /// Request that the owner persist and apply a complete model policy.
    SelectPolicy(SelectPolicy),
    /// Request that the owner persist one favorite mutation.
    SetFavorite(SetFavorite),
    /// Retry the retained failed request or reload unavailable catalog data.
    Retry,
}

/// Owner-controlled persistence/status feedback.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NativeModelSelectorStatus {
    /// A policy or favorite request is currently being persisted.
    pub saving: bool,
    /// Last owner-reported persistence error.
    pub error: Option<String>,
    /// Whether the owner has supplied an authoritative policy for this view.
    pub authoritative: bool,
}

/// Keyboard gestures understood by the pure selector state machine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeModelSelectorKey {
    /// Move to the next selectable model.
    ArrowDown,
    /// Move to the previous selectable model.
    ArrowUp,
    /// Move to the first selectable model.
    Home,
    /// Move to the last selectable model.
    End,
    /// Commit the highlighted model.
    Enter,
    /// Commit the highlighted model.
    Space,
    /// Close without committing a preview.
    Escape,
    /// Close without committing a preview and return focus to the trigger.
    Tab,
    /// Append one character to the model filter.
    Character(char),
    /// Remove one character from the model filter.
    Backspace,
}

/// The dynamic policy axis shown in the preview pane.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativePolicyAxis {
    /// Select a model variant in the same routed family.
    Variant,
    /// Select a reasoning/thinking option.
    Thinking,
    /// Select a speed option.
    Speed,
    /// Select a context-window option.
    ContextWindow,
    /// Select a harness permission option.
    Permission,
}

/// Paint treatment for one token of the full model label.
///
/// The trigger paints the name with its inherited foreground, context/effort/
/// variant detail muted, and an accelerated tier with a static left-to-right
/// glyph gradient. Roles carry no motion: reduced motion never changes a
/// colour.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeModelLabelRole {
    /// The model name; paints with the inherited foreground.
    Name,
    /// Context/effort/variant detail; paints muted.
    Detail,
    /// An accelerated speed tier; paints the gradient across its glyphs.
    Gradient(SpeedGradient),
}

/// One ordered token of the full model label.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeModelLabelToken {
    /// Exact token text.
    pub text: String,
    /// Paint treatment for this token.
    pub role: NativeModelLabelRole,
}

/// Ordered tokens of the full model label
/// (`<name> <context> <effort> <variant> <speed>`).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NativeModelLabel {
    tokens: Vec<NativeModelLabelToken>,
}

/// Pure model-selector interaction state, independent of GPUI.
pub struct NativeModelSelectorState {
    snapshot: NativeModelCatalog,
    policy: Option<NativeModelPolicy>,
    status: NativeModelSelectorStatus,
    open: bool,
    active_engine: String,
    query: String,
    previewed_model_id: Option<String>,
    highlighted_model_id: Option<String>,
    open_axis: Option<NativePolicyAxis>,
    local_error: Option<String>,
    model_groups_cache: RefCell<Option<ModelGroupsCache>>,
    collapsed_groups: std::collections::HashSet<(String, String)>,
}

/// Catalog projections survive animation frames; their inputs change only on interaction.
struct ModelGroupsCache {
    engine: String,
    query: String,
    selected_model_id: Option<String>,
    groups: Rc<[crate::native_model_catalog::NativeModelGroupView]>,
}

#[derive(Clone, Copy, Debug)]
struct EngineIndicatorTransition {
    from_left: f64,
    from_width: f64,
    to_left: f64,
    to_width: f64,
    generation: u64,
}

/// One policy-option tooltip waiting to be shown or already mounted.
///
/// The row bounds are measured in window coordinates by a canvas probe. The
/// tooltip is then rendered as a deferred anchored element, so the dropdown's
/// overflow mask cannot clip a long paragraph.
#[derive(Clone, Debug)]
struct PickerTooltipTarget {
    key: String,
    advisory: Option<String>,
    description: Option<String>,
    row_bounds: Option<Bounds<Pixels>>,
    visible: bool,
}

/// A GPUI entity that paints the model trigger and bounded selector popover.
pub struct NativeModelSelector {
    state: NativeModelSelectorState,
    theme: ArtisanTheme,
    trigger_focus: FocusHandle,
    menu_focus: FocusHandle,
    menu_scroll: ScrollHandle,
    virtual_model_scroll: gpui::UniformListScrollHandle,
    axis_menu_scroll: ScrollHandle,
    trigger_origin: Rc<RefCell<Option<Point<Pixels>>>>,
    menu_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
    trigger_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
    engine_indicator: Rc<RefCell<EngineSectionIndicatorPolicy>>,
    engine_surface_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
    engine_indicator_transition: Rc<RefCell<Option<EngineIndicatorTransition>>>,
    engine_indicator_animation_generation: Rc<RefCell<u64>>,
    menu_motion: Rc<RefCell<PickerMenuMotion>>,
    menu_motion_task: Option<Task<()>>,
    axis_menu_motion: Rc<RefCell<PickerMenuMotion>>,
    axis_menu_motion_task: Option<Task<()>>,
    axis_menu_motion_axis: Option<NativePolicyAxis>,
    axis_menu_pending_axis: Option<NativePolicyAxis>,
    model_hover: Rc<RefCell<SlidingHoverState>>,
    model_hover_surface_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
    axis_hover: Rc<RefCell<SlidingHoverState>>,
    axis_hover_surface_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
    model_scroll: PickerScrollState,
    model_scroll_frame_scheduled: bool,
    axis_scroll: PickerScrollState,
    axis_scroll_frame_scheduled: bool,
    axis_trigger_bounds: Rc<RefCell<[Option<Bounds<Pixels>>; 5]>>,
    axis_menu_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
    option_tooltip: Rc<RefCell<Option<PickerTooltipTarget>>>,
    option_tooltip_task: Option<Task<()>>,
    option_tooltip_generation: u64,
    highlighted_axis_option: Option<String>,
}

#[derive(Clone, Debug)]
struct SelectorOption {
    id: String,
    label: String,
    description: Option<String>,
    advisory: Option<String>,
    group: Option<String>,
    selected: bool,
    disabled: bool,
}
