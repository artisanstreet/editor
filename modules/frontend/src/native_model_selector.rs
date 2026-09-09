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

use std::{cell::RefCell, rc::Rc, sync::Arc, time::Duration};

use artisan_assets::AssetId;
use artisan_ui::{
    gradient::{hover_fill_gradient, vertical_gradient},
    icon::{IconSize, IconStyle, IconTint, icon},
    motion::MotionCurve,
    theme::{ArtisanTheme, SurfaceStep, ThemeMode},
};
use gpui::{
    Anchor, Animation, AnimationExt as _, AnyElement, App, AppContext as _, Bounds, ClickEvent,
    Context, Div, ElementId, EventEmitter, FocusHandle, Focusable, FontWeight, HighlightStyle,
    ImageSource, InteractiveElement as _, KeyDownEvent, MouseDownEvent, ParentElement as _, Pixels,
    Point, Render, RenderImage, ScrollHandle, ScrollWheelEvent, SharedString, Size, Stateful,
    StatefulInteractiveElement as _, Styled as _, StyledText, Task, Window, anchored, canvas,
    deferred, div, img, point, prelude::FluentBuilder as _, prelude::IntoElement, px,
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

#[path = "native_picker_motion.rs"]
mod native_picker_motion;
pub(crate) use self::native_picker_motion::{HoverRect, SlidingHoverState};
use self::native_picker_motion::{PickerMenuMotion, PickerMenuPhase, PickerScrollState};

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
const PICKER_MENU_MOTION_DURATION_MS: u64 = 100;
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeModelSelectorEvent {
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
}

impl NativeModelSelectorState {
    /// Creates closed selector state over a complete catalog snapshot.
    #[must_use]
    pub fn new(snapshot: NativeModelCatalog, policy: Option<NativeModelPolicy>) -> Self {
        let policy = policy
            .and_then(|policy| rebase_selection_policy(&snapshot, &policy))
            .or_else(|| default_picker_policy(&snapshot));
        let active_engine = policy
            .as_ref()
            .map(|policy| policy.engine_id.clone())
            .filter(|engine| snapshot.manifest.harness(engine).is_some())
            .or_else(|| {
                snapshot
                    .manifest
                    .harnesses
                    .first()
                    .map(|harness| harness.id.clone())
            })
            .unwrap_or_default();
        let mut state = Self {
            snapshot,
            policy,
            status: NativeModelSelectorStatus::default(),
            open: false,
            active_engine,
            query: String::new(),
            previewed_model_id: None,
            highlighted_model_id: None,
            open_axis: None,
            local_error: None,
        };
        state.refresh_preview_and_highlight();
        state
    }

    /// Replaces the complete static-plus-runtime snapshot.
    ///
    /// A current policy is rebased onto the new revision and keeps only
    /// compatible option values. Runtime harness readiness is not consulted
    /// while the picker is open.
    pub fn set_snapshot(&mut self, snapshot: NativeModelCatalog) {
        let policy = self
            .policy
            .as_ref()
            .and_then(|policy| rebase_selection_policy(&snapshot, policy));
        let active_engine = if snapshot.manifest.harness(&self.active_engine).is_some() {
            self.active_engine.clone()
        } else {
            policy
                .as_ref()
                .map(|policy| policy.engine_id.clone())
                .or_else(|| {
                    snapshot
                        .manifest
                        .harnesses
                        .first()
                        .map(|harness| harness.id.clone())
                })
                .unwrap_or_default()
        };
        self.snapshot = snapshot;
        self.policy = policy;
        self.active_engine = active_engine;
        self.open_axis = None;
        self.local_error = None;
        self.refresh_preview_and_highlight();
    }

    /// Replaces the owner-authoritative policy.
    ///
    /// Invalid identities are rejected and leave the current state intact.
    /// Compatible policies from an older runtime revision are rebased onto
    /// this snapshot; the method never invents a successful persistence result.
    pub fn set_policy(&mut self, policy: Option<NativeModelPolicy>) -> bool {
        let accepted = match policy {
            None => {
                self.policy = default_picker_policy(&self.snapshot);
                true
            }
            Some(policy) => {
                let Some(policy) = rebase_selection_policy(&self.snapshot, &policy) else {
                    return false;
                };
                self.policy = Some(policy);
                true
            }
        };
        if accepted {
            self.status.authoritative = true;
            self.local_error = None;
            self.refresh_preview_and_highlight();
        }
        accepted
    }

    /// Replaces owner-controlled saving/error/authority feedback.
    pub fn set_status(&mut self, status: NativeModelSelectorStatus) {
        self.status = status;
    }

    /// Returns the complete catalog snapshot.
    #[must_use]
    pub const fn snapshot(&self) -> &NativeModelCatalog {
        &self.snapshot
    }

    /// Returns the currently displayed policy, if one exists.
    #[must_use]
    pub const fn policy(&self) -> Option<&NativeModelPolicy> {
        self.policy.as_ref()
    }

    /// Returns owner-controlled status.
    #[must_use]
    pub const fn status(&self) -> &NativeModelSelectorStatus {
        &self.status
    }

    /// Returns whether the popover is open.
    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.open
    }

    /// Returns the currently active engine tab.
    #[must_use]
    pub fn active_engine(&self) -> &str {
        &self.active_engine
    }

    /// Returns the exact current search/filter query.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Returns the selected catalog model ID.
    #[must_use]
    pub fn selected_model_id(&self) -> Option<&str> {
        self.policy.as_ref().map(|policy| policy.model_id.as_str())
    }

    /// Returns the model currently shown in the preview pane.
    #[must_use]
    pub fn previewed_model_id(&self) -> Option<&str> {
        self.previewed_model_id.as_deref()
    }

    /// Returns the highlighted selectable model ID.
    #[must_use]
    pub fn highlighted_model_id(&self) -> Option<&str> {
        self.highlighted_model_id.as_deref()
    }

    /// Returns the preview model's policy-shaped defaults, including while
    /// disconnected. The result is not a runnable admission.
    #[must_use]
    pub fn preview_policy(&self) -> Option<NativeModelPolicy> {
        let model_id = self.previewed_model_id.as_deref()?;
        if self.selected_model_id() == Some(model_id) {
            return self.policy.clone();
        }
        self.snapshot.preview_policy_for_model(model_id).ok()
    }

    /// Returns the compact trigger label without inventing a model name.
    #[must_use]
    pub fn trigger_label(&self) -> String {
        self.selected_model_id()
            .and_then(|model_id| self.snapshot.manifest.model(model_id))
            .map_or_else(|| "No model".to_owned(), |model| model.name.clone())
    }

    /// Returns the compact trigger's exact policy-axis summary.
    #[must_use]
    pub fn trigger_summary(&self) -> String {
        let Some(policy) = &self.policy else {
            return String::new();
        };
        let model = self.snapshot.manifest.model(&policy.model_id);
        let mut values = Vec::new();
        if let Some(value) = &policy.reasoning_effort {
            let has_selectable_thinking = model.map_or(true, |model| {
                matches!(
                    &model.capabilities.thinking,
                    NativeThinkingCapability::Supported { .. }
                )
            });
            if has_selectable_thinking {
                values.push(humanize_variant(&value.id));
            }
        }
        if let Some(selection) = &policy.native_selection
            && let Some(variant) = selection.variant_id.as_deref()
            && variant != "default"
        {
            values.push(humanize_variant(variant));
        }
        if let Some(value) = &policy.speed
            && !model.is_some_and(|model| {
                model
                    .capabilities
                    .speed_options
                    .iter()
                    .any(|option| option.id == value.id && option.default)
            })
        {
            let label = model
                .and_then(|model| {
                    model
                        .capabilities
                        .speed_options
                        .iter()
                        .find(|option| option.id == value.id)
                        .map(|option| option.label.clone())
                })
                .unwrap_or_else(|| humanize_variant(&value.id));
            values.push(label);
        }
        values.join(" \u{b7} ")
    }

    /// Opens/closes the popover without emitting a policy event.
    pub fn press_trigger(&mut self) {
        if self.open {
            self.dismiss();
        } else {
            self.open = true;
            self.local_error = None;
            self.refresh_preview_and_highlight();
        }
    }

    /// Dismisses the popover and restores preview to the selected policy.
    pub fn dismiss(&mut self) {
        self.open = false;
        self.open_axis = None;
        self.previewed_model_id = self.selected_model_id().map(str::to_owned);
        self.highlighted_model_id = None;
    }

    /// Switches the active engine tab.
    pub fn set_active_engine(&mut self, engine_id: impl Into<String>) {
        let engine_id = engine_id.into();
        if self.snapshot.manifest.harness(&engine_id).is_none() {
            return;
        }
        self.active_engine = engine_id;
        self.query.clear();
        self.open_axis = None;
        self.refresh_preview_and_highlight();
    }

    /// Replaces the exact filter query.
    pub fn set_query(&mut self, query: impl Into<String>) {
        self.query = query.into();
        self.refresh_preview_and_highlight();
    }

    /// Previews a model on pointer/focus movement without selecting it.
    pub fn preview_model(&mut self, model_id: &str) {
        if let Some(model) = self.snapshot.manifest.model(model_id)
            && model.harness == self.active_engine
        {
            self.previewed_model_id = Some(model_id.to_owned());
            self.highlighted_model_id = model.disabled.is_none().then(|| model_id.to_owned());
        }
    }

    /// Returns filtered and grouped rows for the current engine.
    #[must_use]
    pub fn model_groups(&self) -> Vec<crate::native_model_catalog::NativeModelGroupView> {
        self.snapshot.route_groups_for_engine(
            &self.active_engine,
            &self.query,
            self.selected_model_id(),
        )
    }

    /// Returns the source-order index of the highlighted visible row.
    #[must_use]
    pub fn highlighted_index(&self) -> Option<usize> {
        let highlighted = self.highlighted_model_id.as_deref()?;
        self.visible_models()
            .iter()
            .position(|model| model.id == highlighted)
    }

    /// Returns whether one policy axis menu is open.
    #[must_use]
    pub fn is_axis_open(&self, axis: NativePolicyAxis) -> bool {
        self.open_axis == Some(axis)
    }

    /// Toggles one preview policy axis menu.
    pub fn toggle_axis(&mut self, axis: NativePolicyAxis) {
        self.open_axis = (self.open_axis != Some(axis)).then_some(axis);
        self.local_error = None;
    }

    /// Selects the highlighted model and queues an explicit policy event.
    pub fn activate_highlighted(&mut self) -> Option<NativeModelSelectorEvent> {
        let model_id = self.highlighted_model_id.clone()?;
        self.select_model(&model_id)
    }

    /// Selects a model by exact catalog ID without requiring runtime readiness.
    pub fn select_model(&mut self, model_id: &str) -> Option<NativeModelSelectorEvent> {
        match self.select_model_policy(model_id) {
            Ok(policy) => Some(NativeModelSelectorEvent::SelectPolicy(policy)),
            Err(error) => {
                self.local_error = Some(error.to_string());
                None
            }
        }
    }

    /// Applies one exact option ID/native value to the preview policy.
    pub fn choose_option(
        &mut self,
        axis: NativePolicyAxis,
        option_id: &str,
    ) -> Result<Option<NativeModelSelectorEvent>, NativePolicyValidationError> {
        if axis == NativePolicyAxis::Variant {
            return Ok(Some(NativeModelSelectorEvent::SelectPolicy(
                self.select_model_policy(option_id)?,
            )));
        }
        let model_id = self
            .previewed_model_id
            .clone()
            .ok_or_else(|| NativePolicyValidationError::UnknownModel(String::new()))?;
        let mut policy = if self.selected_model_id() == Some(model_id.as_str()) {
            self.policy
                .clone()
                .ok_or_else(|| NativePolicyValidationError::UnknownModel(model_id.clone()))?
        } else {
            self.snapshot.selection_policy_for_model(&model_id)?
        };
        match axis {
            NativePolicyAxis::Thinking => {
                let model =
                    self.snapshot.manifest.model(&model_id).ok_or_else(|| {
                        NativePolicyValidationError::UnknownModel(model_id.clone())
                    })?;
                let NativeThinkingCapability::Supported { options, .. } =
                    &model.capabilities.thinking
                else {
                    return Err(NativePolicyValidationError::InvalidThinkingOption {
                        model_id,
                        option_id: option_id.to_owned(),
                    });
                };
                let option = options
                    .iter()
                    .find(|option| option.id == option_id)
                    .ok_or_else(|| NativePolicyValidationError::InvalidThinkingOption {
                        model_id: model_id.clone(),
                        option_id: option_id.to_owned(),
                    })?;
                policy.reasoning_effort = Some(NativeOptionValue {
                    id: option.id.clone(),
                    native_value: option.native_value.clone(),
                });
            }
            NativePolicyAxis::Speed => {
                let model =
                    self.snapshot.manifest.model(&model_id).ok_or_else(|| {
                        NativePolicyValidationError::UnknownModel(model_id.clone())
                    })?;
                let option = model
                    .capabilities
                    .speed_options
                    .iter()
                    .find(|option| option.id == option_id)
                    .ok_or_else(|| NativePolicyValidationError::InvalidSpeedOption {
                        model_id: model_id.clone(),
                        option_id: option_id.to_owned(),
                    })?;
                if option.disabled.is_some() {
                    return Err(NativePolicyValidationError::InvalidSpeedOption {
                        model_id: model_id.clone(),
                        option_id: option_id.to_owned(),
                    });
                }
                policy.speed = Some(NativeOptionValue {
                    id: option.id.clone(),
                    native_value: option.native_value.clone(),
                });
            }
            NativePolicyAxis::ContextWindow => {
                let model =
                    self.snapshot.manifest.model(&model_id).ok_or_else(|| {
                        NativePolicyValidationError::UnknownModel(model_id.clone())
                    })?;
                let capability = model.capabilities.context_window.as_ref().ok_or_else(|| {
                    NativePolicyValidationError::InvalidContextOption {
                        model_id: model_id.clone(),
                        option_id: option_id.to_owned(),
                    }
                })?;
                let option = capability
                    .options
                    .iter()
                    .find(|option| option.id == option_id)
                    .ok_or_else(|| NativePolicyValidationError::InvalidContextOption {
                        model_id: model_id.clone(),
                        option_id: option_id.to_owned(),
                    })?;
                policy.context_window = Some(crate::native_model_catalog::NativeContextSelection {
                    id: option.id.clone(),
                    native_suffix: option.native_suffix.clone(),
                    native_config: option.native_config.clone(),
                });
            }
            NativePolicyAxis::Permission => {
                let model =
                    self.snapshot.manifest.model(&model_id).ok_or_else(|| {
                        NativePolicyValidationError::UnknownModel(model_id.clone())
                    })?;
                let harness = self
                    .snapshot
                    .manifest
                    .harness(&model.harness)
                    .ok_or_else(|| {
                        NativePolicyValidationError::UnknownHarness(model.harness.clone())
                    })?;
                let option = harness
                    .permissions
                    .options
                    .iter()
                    .find(|option| option.id == option_id)
                    .ok_or_else(|| NativePolicyValidationError::InvalidPermissionOption {
                        engine_id: model.harness.clone(),
                        option_id: option_id.to_owned(),
                    })?;
                policy.permission = Some(NativeOptionValue {
                    id: option.id.clone(),
                    native_value: option.native_value.clone(),
                });
            }
            NativePolicyAxis::Variant => unreachable!("handled before policy construction"),
        }
        self.snapshot.validate_selection_policy(&policy)?;
        self.policy = Some(policy.clone());
        self.local_error = None;
        self.open_axis = None;
        Ok(Some(NativeModelSelectorEvent::SelectPolicy(policy)))
    }

    /// Emits a favorite intent without mutating the authoritative favorite
    /// list. The owner must send a new snapshot after persistence.
    pub fn toggle_favorite(&mut self, model_id: &str) -> Option<NativeModelSelectorEvent> {
        let model = self.snapshot.manifest.model(model_id)?;
        if model.disabled.is_some() {
            return None;
        }
        Some(NativeModelSelectorEvent::SetFavorite(SetFavorite {
            model_id: model_id.to_owned(),
            favorite: !self.snapshot.is_favorite(model_id),
        }))
    }

    /// Handles one keyboard gesture while the popover is open.
    pub fn handle_key(&mut self, key: NativeModelSelectorKey) -> Option<NativeModelSelectorEvent> {
        if !self.open {
            return None;
        }
        match key {
            NativeModelSelectorKey::ArrowDown => self.move_highlight(1),
            NativeModelSelectorKey::ArrowUp => self.move_highlight(-1),
            NativeModelSelectorKey::Home => self.move_to_edge(false),
            NativeModelSelectorKey::End => self.move_to_edge(true),
            NativeModelSelectorKey::Enter | NativeModelSelectorKey::Space => {
                self.activate_highlighted()
            }
            NativeModelSelectorKey::Escape | NativeModelSelectorKey::Tab => {
                self.dismiss();
                None
            }
            NativeModelSelectorKey::Character(character) if !character.is_control() => {
                self.query.push(character);
                self.refresh_preview_and_highlight();
                None
            }
            NativeModelSelectorKey::Backspace => {
                self.query.pop();
                self.refresh_preview_and_highlight();
                None
            }
            NativeModelSelectorKey::Character(_) => None,
        }
    }

    fn visible_models(&self) -> Vec<NativeModelView> {
        self.snapshot
            .models_for_engine(&self.active_engine, &self.query, self.selected_model_id())
    }

    fn refresh_preview_and_highlight(&mut self) {
        let rows = self.visible_models();
        let selected_in_view = self
            .selected_model_id()
            .filter(|id| rows.iter().any(|row| row.id == *id))
            .map(str::to_owned);
        self.previewed_model_id =
            selected_in_view.or_else(|| rows.first().map(|row| row.id.clone()));
        self.highlighted_model_id = rows
            .iter()
            .find(|row| !self.model_definition_disabled(&row.id))
            .map(|row| row.id.clone());
    }

    fn move_highlight(&mut self, direction: isize) -> Option<NativeModelSelectorEvent> {
        let rows = self
            .visible_models()
            .into_iter()
            .filter(|row| !self.model_definition_disabled(&row.id))
            .collect::<Vec<_>>();
        if rows.is_empty() {
            return None;
        }
        let current = self
            .highlighted_model_id
            .as_deref()
            .and_then(|id| rows.iter().position(|row| row.id == id));
        let index = match (current, direction.cmp(&0)) {
            (None, _) => 0,
            (Some(index), std::cmp::Ordering::Less) => {
                if index == 0 {
                    rows.len() - 1
                } else {
                    index - 1
                }
            }
            (Some(index), std::cmp::Ordering::Greater) => (index + 1) % rows.len(),
            (Some(index), std::cmp::Ordering::Equal) => index,
        };
        self.highlighted_model_id = Some(rows[index].id.clone());
        self.previewed_model_id = Some(rows[index].id.clone());
        None
    }

    fn move_to_edge(&mut self, last: bool) -> Option<NativeModelSelectorEvent> {
        let rows = self
            .visible_models()
            .into_iter()
            .filter(|row| !self.model_definition_disabled(&row.id))
            .collect::<Vec<_>>();
        let row = if last { rows.last() } else { rows.first() }?;
        self.highlighted_model_id = Some(row.id.clone());
        self.previewed_model_id = Some(row.id.clone());
        None
    }

    fn set_local_error(&mut self, error: Option<String>) {
        self.local_error = error;
    }

    fn select_model_policy(
        &mut self,
        model_id: &str,
    ) -> Result<NativeModelPolicy, NativePolicyValidationError> {
        self.preview_model(model_id);
        let policy = self.snapshot.selection_policy_for_model(model_id)?;
        self.policy = Some(policy.clone());
        self.open = false;
        self.open_axis = None;
        self.previewed_model_id = Some(model_id.to_owned());
        self.highlighted_model_id = None;
        self.local_error = None;
        Ok(policy)
    }

    fn model_definition_disabled(&self, model_id: &str) -> bool {
        self.snapshot
            .manifest
            .model(model_id)
            .is_some_and(|model| model.disabled.is_some())
    }

    fn model_definition_disabled_reason(&self, model_id: &str) -> Option<String> {
        self.snapshot
            .manifest
            .model(model_id)
            .and_then(|model| model.disabled.as_ref())
            .map(|disabled| disabled.reason.clone())
    }
}

/// Reconciles an owner policy for picker use without requiring a configured
/// runtime harness. Each option is retained only when the catalog still
/// accepts its exact ID/native value; the catalog supplies fresh identity and
/// defaults for everything else.
fn default_picker_policy(snapshot: &NativeModelCatalog) -> Option<NativeModelPolicy> {
    snapshot
        .default_model_id
        .as_deref()
        .and_then(|id| snapshot.selection_policy_for_model(id).ok())
        .or_else(|| {
            snapshot
                .manifest
                .models
                .iter()
                .find_map(|model| snapshot.selection_policy_for_model(&model.id).ok())
        })
}

fn rebase_selection_policy(
    snapshot: &NativeModelCatalog,
    policy: &NativeModelPolicy,
) -> Option<NativeModelPolicy> {
    let mut rebased = snapshot.selection_policy_for_model(&policy.model_id).ok()?;
    if rebased.engine_id != policy.engine_id
        || rebased.native_model_id != policy.native_model_id
        || rebased.native_selection != policy.native_selection
    {
        return None;
    }

    if policy.reasoning_effort.is_some() {
        let mut candidate = rebased.clone();
        candidate.reasoning_effort = policy.reasoning_effort.clone();
        if snapshot.validate_selection_policy(&candidate).is_ok() {
            rebased.reasoning_effort = candidate.reasoning_effort;
        }
    }
    if policy.speed.is_some() {
        let mut candidate = rebased.clone();
        candidate.speed = policy.speed.clone();
        if snapshot.validate_selection_policy(&candidate).is_ok() {
            rebased.speed = candidate.speed;
        }
    }
    if policy.context_window.is_some() {
        let mut candidate = rebased.clone();
        candidate.context_window = policy.context_window.clone();
        if snapshot.validate_selection_policy(&candidate).is_ok() {
            rebased.context_window = candidate.context_window;
        }
    }
    if policy.permission.is_some() {
        let mut candidate = rebased.clone();
        candidate.permission = policy.permission.clone();
        if snapshot.validate_selection_policy(&candidate).is_ok() {
            rebased.permission = candidate.permission;
        }
    }
    Some(rebased)
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

impl EventEmitter<NativeModelSelectorEvent> for NativeModelSelector {}

impl Focusable for NativeModelSelector {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.trigger_focus.clone()
    }
}

impl NativeModelSelector {
    /// Builds a selector entity over a complete catalog snapshot.
    pub fn new(
        snapshot: NativeModelCatalog,
        policy: Option<NativeModelPolicy>,
        mode: ThemeMode,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            state: NativeModelSelectorState::new(snapshot, policy),
            theme: ArtisanTheme::for_mode(mode),
            trigger_focus: cx.focus_handle().tab_index(1).tab_stop(true),
            menu_focus: cx.focus_handle(),
            menu_scroll: ScrollHandle::new(),
            axis_menu_scroll: ScrollHandle::new(),
            trigger_origin: Rc::new(RefCell::new(None)),
            menu_bounds: Rc::new(RefCell::new(None)),
            trigger_bounds: Rc::new(RefCell::new(None)),
            engine_indicator: Rc::new(RefCell::new(EngineSectionIndicatorPolicy::new())),
            engine_surface_bounds: Rc::new(RefCell::new(None)),
            engine_indicator_transition: Rc::new(RefCell::new(None)),
            engine_indicator_animation_generation: Rc::new(RefCell::new(0)),
            menu_motion: Rc::new(RefCell::new(PickerMenuMotion::default())),
            menu_motion_task: None,
            axis_menu_motion: Rc::new(RefCell::new(PickerMenuMotion::default())),
            axis_menu_motion_task: None,
            axis_menu_motion_axis: None,
            axis_menu_pending_axis: None,
            model_hover: Rc::new(RefCell::new(SlidingHoverState::default())),
            model_hover_surface_bounds: Rc::new(RefCell::new(None)),
            axis_hover: Rc::new(RefCell::new(SlidingHoverState::default())),
            axis_hover_surface_bounds: Rc::new(RefCell::new(None)),
            model_scroll: PickerScrollState::default(),
            model_scroll_frame_scheduled: false,
            axis_scroll: PickerScrollState::default(),
            axis_scroll_frame_scheduled: false,
            axis_trigger_bounds: Rc::new(RefCell::new([None; 5])),
            axis_menu_bounds: Rc::new(RefCell::new(None)),
            option_tooltip: Rc::new(RefCell::new(None)),
            option_tooltip_task: None,
            option_tooltip_generation: 0,
            highlighted_axis_option: None,
        }
    }

    /// Returns read-only pure interaction state.
    #[must_use]
    pub const fn state(&self) -> &NativeModelSelectorState {
        &self.state
    }

    /// Replaces the complete static-plus-runtime catalog snapshot.
    pub fn set_snapshot(&mut self, snapshot: NativeModelCatalog, cx: &mut Context<Self>) {
        let closing_axis = self.axis_menu_motion_axis.or(self.state.open_axis);
        self.state.set_snapshot(snapshot);
        self.axis_menu_pending_axis = None;
        self.clear_option_tooltip();
        if let Some(axis) = closing_axis {
            self.begin_axis_menu_close(axis, cx);
        }
        cx.notify();
    }

    /// Replaces the owner-authoritative policy.
    pub fn set_policy(&mut self, policy: Option<SelectPolicy>, cx: &mut Context<Self>) {
        if !self.state.set_policy(policy) {
            self.state.set_local_error(Some(
                "The supplied model policy is not compatible with the catalog.".to_owned(),
            ));
        }
        cx.notify();
    }

    /// Supplies owner-controlled save/error/authority state.
    pub fn set_status(&mut self, status: NativeModelSelectorStatus, cx: &mut Context<Self>) {
        self.state.set_status(status);
        cx.notify();
    }

    fn menu_is_interactive(&self) -> bool {
        self.state.is_open() && self.menu_motion.borrow().phase() != PickerMenuPhase::Closing
    }

    fn axis_is_interactive(&self, axis: NativePolicyAxis) -> bool {
        self.menu_is_interactive()
            && self.state.is_axis_open(axis)
            && self.axis_menu_motion_axis == Some(axis)
            && self.axis_menu_motion.borrow().phase() != PickerMenuPhase::Closing
    }

    fn clear_option_tooltip(&mut self) {
        self.option_tooltip_generation = self.option_tooltip_generation.wrapping_add(1);
        self.option_tooltip.borrow_mut().take();
        self.option_tooltip_task = None;
    }

    fn clear_option_tooltip_for(&mut self, key: &str) {
        let matches = self
            .option_tooltip
            .borrow()
            .as_ref()
            .is_some_and(|target| target.key == key);
        if matches {
            self.clear_option_tooltip();
        }
    }

    fn begin_option_tooltip(
        &mut self,
        axis: NativePolicyAxis,
        option: &SelectorOption,
        cx: &mut Context<Self>,
    ) {
        let key = option_tooltip_key(axis, &option.id);
        if option_tooltip_text(option.advisory.as_deref(), option.description.as_deref()).is_none()
        {
            self.clear_option_tooltip();
            return;
        }

        self.option_tooltip_generation = self.option_tooltip_generation.wrapping_add(1);
        let generation = self.option_tooltip_generation;
        *self.option_tooltip.borrow_mut() = Some(PickerTooltipTarget {
            key: key.clone(),
            advisory: option.advisory.clone(),
            description: option.description.clone(),
            row_bounds: None,
            visible: false,
        });
        self.option_tooltip_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(PICKER_TOOLTIP_SHOW_DELAY_MS))
                .await;
            let _ = this.update(cx, |selector, cx| {
                if selector.option_tooltip_generation == generation
                    && selector.axis_is_interactive(axis)
                    && selector
                        .option_tooltip
                        .borrow()
                        .as_ref()
                        .is_some_and(|target| target.key == key)
                {
                    if let Some(target) = selector.option_tooltip.borrow_mut().as_mut() {
                        target.visible = true;
                    }
                    selector.option_tooltip_task = None;
                    cx.notify();
                }
            });
        }));
    }

    fn schedule_menu_motion_settle(
        &mut self,
        generation: u64,
        opening: bool,
        cx: &mut Context<Self>,
    ) {
        self.menu_motion_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(PICKER_MENU_MOTION_DURATION_MS))
                .await;
            let _ = this.update(cx, |selector, cx| {
                let settled = if opening {
                    selector.menu_motion.borrow_mut().finish_open(generation)
                } else {
                    selector.menu_motion.borrow_mut().finish_close(generation)
                };
                if settled {
                    selector.menu_motion_task = None;
                    if !opening {
                        selector.menu_bounds.borrow_mut().take();
                        selector.axis_menu_bounds.borrow_mut().take();
                        selector.model_hover.borrow_mut().clear();
                        selector.axis_hover.borrow_mut().clear();
                    }
                    cx.notify();
                }
            });
        }));
    }

    fn schedule_axis_menu_motion_settle(
        &mut self,
        generation: u64,
        opening: bool,
        cx: &mut Context<Self>,
    ) {
        self.axis_menu_motion_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(PICKER_MENU_MOTION_DURATION_MS))
                .await;
            let _ = this.update(cx, |selector, cx| {
                let settled = if opening {
                    selector
                        .axis_menu_motion
                        .borrow_mut()
                        .finish_open(generation)
                } else {
                    selector
                        .axis_menu_motion
                        .borrow_mut()
                        .finish_close(generation)
                };
                if settled {
                    selector.axis_menu_motion_task = None;
                    if !opening {
                        selector.axis_menu_bounds.borrow_mut().take();
                        selector.axis_hover_surface_bounds.borrow_mut().take();
                        selector.axis_hover.borrow_mut().clear();
                        selector.clear_option_tooltip();
                        if let Some(axis) = selector.axis_menu_pending_axis.take() {
                            if selector.state.is_open()
                                && selector.menu_motion.borrow().phase() != PickerMenuPhase::Closing
                            {
                                selector.state.open_axis = Some(axis);
                                selector.begin_axis_menu_open(axis, cx);
                            } else {
                                selector.axis_menu_motion_axis = None;
                            }
                        } else {
                            selector.axis_menu_motion_axis = None;
                        }
                    }
                    cx.notify();
                }
            });
        }));
    }

    fn begin_menu_open(&mut self, cx: &mut Context<Self>) {
        if self.axis_menu_motion.borrow().phase() == PickerMenuPhase::Closing {
            self.axis_menu_motion.borrow_mut().hide();
            self.axis_menu_motion_axis = None;
            self.axis_menu_pending_axis = None;
            self.axis_menu_motion_task = None;
            self.axis_menu_bounds.borrow_mut().take();
            self.axis_hover_surface_bounds.borrow_mut().take();
            self.axis_hover.borrow_mut().clear();
        }
        let generation = self.menu_motion.borrow_mut().begin_open();
        if cx.reduce_motion() {
            self.menu_motion.borrow_mut().finish_open(generation);
            self.menu_motion_task = None;
        } else {
            self.schedule_menu_motion_settle(generation, true, cx);
        }
    }

    fn begin_axis_menu_open(&mut self, axis: NativePolicyAxis, cx: &mut Context<Self>) {
        self.axis_menu_motion_axis = Some(axis);
        self.axis_menu_pending_axis = None;
        let generation = self.axis_menu_motion.borrow_mut().begin_open();
        if cx.reduce_motion() {
            self.axis_menu_motion.borrow_mut().finish_open(generation);
            self.axis_menu_motion_task = None;
        } else {
            self.schedule_axis_menu_motion_settle(generation, true, cx);
        }
    }

    fn begin_axis_menu_close(&mut self, axis: NativePolicyAxis, cx: &mut Context<Self>) {
        self.axis_menu_motion_axis = Some(axis);
        self.clear_option_tooltip();
        self.highlighted_axis_option = None;
        self.axis_menu_bounds.borrow_mut().take();
        self.axis_hover_surface_bounds.borrow_mut().take();
        self.axis_hover.borrow_mut().clear();
        self.axis_scroll.cancel_to(
            f32::from(self.axis_menu_scroll.offset().y),
            f32::from(self.axis_menu_scroll.max_offset().y),
        );
        let generation = self.axis_menu_motion.borrow_mut().begin_close();
        if cx.reduce_motion() {
            self.axis_menu_motion.borrow_mut().finish_close(generation);
            self.axis_menu_motion_task = None;
            if let Some(next_axis) = self.axis_menu_pending_axis.take() {
                if self.state.is_open()
                    && self.menu_motion.borrow().phase() != PickerMenuPhase::Closing
                {
                    self.state.open_axis = Some(next_axis);
                    self.begin_axis_menu_open(next_axis, cx);
                } else {
                    self.axis_menu_motion_axis = None;
                }
            } else {
                self.axis_menu_motion_axis = None;
            }
        } else {
            self.schedule_axis_menu_motion_settle(generation, false, cx);
        }
    }

    fn begin_menu_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let closing_axis = self.axis_menu_motion_axis.or(self.state.open_axis);
        self.state.dismiss();
        self.axis_menu_pending_axis = None;
        self.clear_option_tooltip();
        self.highlighted_axis_option = None;
        self.axis_menu_bounds.borrow_mut().take();
        self.axis_hover_surface_bounds.borrow_mut().take();
        self.model_hover.borrow_mut().clear();
        self.axis_hover.borrow_mut().clear();
        self.model_scroll.cancel_to(
            f32::from(self.menu_scroll.offset().y),
            f32::from(self.menu_scroll.max_offset().y),
        );
        self.axis_scroll.cancel_to(
            f32::from(self.axis_menu_scroll.offset().y),
            f32::from(self.axis_menu_scroll.max_offset().y),
        );
        if let Some(axis) = closing_axis {
            self.begin_axis_menu_close(axis, cx);
        }
        let generation = self.menu_motion.borrow_mut().begin_close();
        if cx.reduce_motion() {
            self.menu_motion.borrow_mut().finish_close(generation);
            self.menu_motion_task = None;
        } else {
            self.schedule_menu_motion_settle(generation, false, cx);
        }
        window.focus(&self.trigger_focus, cx);
    }

    fn handle_model_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_picker_scroll(event, window, cx, true);
    }

    fn handle_axis_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_picker_scroll(event, window, cx, false);
    }

    fn handle_picker_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
        model_list: bool,
    ) {
        // The content wrapper is the first bubble listener inside the GPUI
        // scroll container. Stopping propagation there prevents its default
        // immediate offset update from being applied a second time.
        cx.stop_propagation();
        if !self.menu_is_interactive() || (!model_list && self.state.open_axis.is_none()) {
            return;
        }

        let delta = event.delta.pixel_delta(window.line_height()).y;
        let delta = f32::from(delta);
        if delta.abs() <= f32::EPSILON {
            return;
        }
        if !model_list {
            self.clear_option_tooltip();
        }

        let handle = if model_list {
            self.menu_scroll.clone()
        } else {
            // Keep policy-menu scrolling independent so a future long option
            // list cannot fight model-list inertia.
            self.axis_menu_scroll.clone()
        };
        let offset = handle.offset();
        let current = f32::from(offset.y);
        let maximum = f32::from(handle.max_offset().y).max(0.0);

        if event.delta.precise() || cx.reduce_motion() {
            let next = (current + delta).clamp(-maximum, 0.0);
            handle.set_offset(point(offset.x, px(next)));
            if model_list {
                self.model_scroll.cancel_to(next, maximum);
            } else {
                self.axis_scroll.cancel_to(next, maximum);
            }
            cx.notify();
            return;
        }

        if model_list {
            self.model_scroll.push(current, delta, maximum);
            self.schedule_model_scroll_frame(window, cx);
        } else {
            self.axis_scroll.push(current, delta, maximum);
            self.schedule_axis_scroll_frame(window, cx);
        }
    }

    fn schedule_model_scroll_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.model_scroll.active() || self.model_scroll_frame_scheduled {
            return;
        }
        self.model_scroll_frame_scheduled = true;
        cx.on_next_frame(window, |selector, window, cx| {
            selector.advance_model_scroll(window, cx);
        });
    }

    fn schedule_axis_scroll_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.axis_scroll.active() || self.axis_scroll_frame_scheduled {
            return;
        }
        self.axis_scroll_frame_scheduled = true;
        cx.on_next_frame(window, |selector, window, cx| {
            selector.advance_axis_scroll(window, cx);
        });
    }

    fn advance_model_scroll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.model_scroll_frame_scheduled = false;
        let offset = self.menu_scroll.offset();
        let maximum = f32::from(self.menu_scroll.max_offset().y).max(0.0);
        if let Some(next) = self.model_scroll.step(f32::from(offset.y), maximum) {
            self.menu_scroll.set_offset(point(offset.x, px(next)));
            cx.notify();
        }
        self.schedule_model_scroll_frame(window, cx);
    }

    fn advance_axis_scroll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.axis_scroll_frame_scheduled = false;
        let offset = self.axis_menu_scroll.offset();
        let maximum = f32::from(self.axis_menu_scroll.max_offset().y).max(0.0);
        if let Some(next) = self.axis_scroll.step(f32::from(offset.y), maximum) {
            self.axis_menu_scroll.set_offset(point(offset.x, px(next)));
            cx.notify();
        }
        self.schedule_axis_scroll_frame(window, cx);
    }

    fn handle_trigger_click(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.is_open() {
            self.begin_menu_close(window, cx);
        } else {
            self.state.press_trigger();
            self.model_hover.borrow_mut().clear();
            self.axis_hover.borrow_mut().clear();
            self.begin_menu_open(cx);
            self.sync_focus_after_transition(window, cx);
        }
        cx.notify();
    }

    fn handle_menu_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = selector_key_from_event(event) else {
            return;
        };
        if !self.menu_is_interactive() {
            cx.stop_propagation();
            return;
        }
        if let Some(axis) = self.state.open_axis {
            let options = self
                .axis_options(axis, &self.preview_view())
                .into_iter()
                .filter(|option| !option.disabled)
                .collect::<Vec<_>>();
            let current = options
                .iter()
                .position(|option| Some(&option.id) == self.highlighted_axis_option.as_ref())
                .or_else(|| options.iter().position(|option| option.selected))
                .unwrap_or(0);
            let next = match key {
                NativeModelSelectorKey::ArrowDown if !options.is_empty() => {
                    Some((current + 1) % options.len())
                }
                NativeModelSelectorKey::ArrowUp if !options.is_empty() => {
                    Some((current + options.len() - 1) % options.len())
                }
                NativeModelSelectorKey::Home if !options.is_empty() => Some(0),
                NativeModelSelectorKey::End if !options.is_empty() => Some(options.len() - 1),
                NativeModelSelectorKey::Enter | NativeModelSelectorKey::Space => {
                    if let Some(option) = options.get(current) {
                        self.choose_axis(axis, option.id.clone(), window, cx);
                    }
                    None
                }
                NativeModelSelectorKey::Escape | NativeModelSelectorKey::Tab => {
                    self.state.open_axis = None;
                    self.begin_axis_menu_close(axis, cx);
                    None
                }
                _ => None,
            };
            if let Some(index) = next {
                self.highlighted_axis_option = Some(options[index].id.clone());
                self.axis_hover
                    .borrow_mut()
                    .set_active(options[index].id.clone());
                self.begin_option_tooltip(axis, &options[index], cx);
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        let was_open = self.state.is_open();
        let emitted = self.state.handle_key(key);
        if let Some(event) = emitted {
            cx.emit(event);
        }
        if was_open && !self.state.is_open() {
            self.begin_menu_close(window, cx);
        } else if self.state.is_open() {
            if matches!(
                key,
                NativeModelSelectorKey::ArrowDown
                    | NativeModelSelectorKey::ArrowUp
                    | NativeModelSelectorKey::Home
                    | NativeModelSelectorKey::End
            ) {
                if let Some(model_id) = self.state.highlighted_model_id().map(str::to_owned) {
                    self.model_hover.borrow_mut().set_active(model_id);
                }
            }
            self.reveal_highlight();
        }
        cx.notify();
    }

    fn handle_outside_press(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.state.is_open()
            || (self.state.open_axis.is_some()
                && self
                    .axis_menu_bounds
                    .borrow()
                    .as_ref()
                    .is_some_and(|bounds| bounds.contains(&event.position)))
            || self
                .menu_bounds
                .borrow()
                .as_ref()
                .is_some_and(|bounds| bounds.contains(&event.position))
            || self
                .trigger_bounds
                .borrow()
                .as_ref()
                .is_some_and(|bounds| bounds.contains(&event.position))
        {
            return;
        }
        self.begin_menu_close(window, cx);
        cx.notify();
    }

    fn choose_model(&mut self, model_id: String, window: &mut Window, cx: &mut Context<Self>) {
        if !self.menu_is_interactive() {
            return;
        }
        let closing_axis = self.axis_menu_motion_axis.or(self.state.open_axis);
        let emitted = self.state.select_model(&model_id);
        if let Some(event) = emitted {
            cx.emit(event);
        }
        if let Some(axis) = closing_axis.filter(|_| self.state.open_axis.is_none()) {
            self.begin_axis_menu_close(axis, cx);
        }
        if self.state.is_open() {
            self.sync_focus_after_transition(window, cx);
        } else {
            self.begin_menu_close(window, cx);
        }
        cx.notify();
    }

    fn toggle_favorite(&mut self, model_id: String, cx: &mut Context<Self>) {
        if !self.menu_is_interactive() {
            return;
        }
        if let Some(event) = self.state.toggle_favorite(&model_id) {
            cx.emit(event);
        }
        cx.notify();
    }

    fn choose_axis(
        &mut self,
        axis: NativePolicyAxis,
        option_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.axis_is_interactive(axis) {
            return;
        }
        let axis_was_open = self.state.is_axis_open(axis);
        match self.state.choose_option(axis, &option_id) {
            Ok(Some(event)) => cx.emit(event),
            Ok(None) => {}
            Err(error) => self.state.set_local_error(Some(error.to_string())),
        }
        if axis_was_open && !self.state.is_axis_open(axis) {
            self.begin_axis_menu_close(axis, cx);
        }
        if !self.state.is_open() {
            self.begin_menu_close(window, cx);
        }
        cx.notify();
    }

    fn toggle_axis(
        &mut self,
        axis: NativePolicyAxis,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.menu_is_interactive() {
            return;
        }
        self.highlighted_axis_option = None;
        self.axis_menu_bounds.borrow_mut().take();
        self.axis_hover_surface_bounds.borrow_mut().take();
        self.clear_option_tooltip();
        self.axis_hover.borrow_mut().clear();
        self.axis_menu_scroll.set_offset(point(px(0.0), px(0.0)));
        self.axis_scroll.cancel_to(0.0, 0.0);
        if let Some(open_axis) = self.state.open_axis {
            if open_axis == axis {
                self.state.open_axis = None;
                self.axis_menu_pending_axis = None;
                self.begin_axis_menu_close(axis, cx);
            } else {
                self.state.open_axis = None;
                self.axis_menu_pending_axis = Some(axis);
                self.begin_axis_menu_close(open_axis, cx);
            }
        } else {
            self.state.toggle_axis(axis);
            self.begin_axis_menu_open(axis, cx);
        }
        cx.notify();
    }

    fn switch_engine(&mut self, engine_id: String, cx: &mut Context<Self>) {
        if !self.menu_is_interactive() {
            return;
        }
        let closing_axis = self.axis_menu_motion_axis.or(self.state.open_axis);
        self.state.set_active_engine(engine_id);
        self.axis_menu_pending_axis = None;
        self.clear_option_tooltip();
        if let Some(axis) = closing_axis {
            self.begin_axis_menu_close(axis, cx);
        }
        self.model_hover.borrow_mut().clear();
        self.menu_scroll.set_offset(point(px(0.0), px(0.0)));
        self.model_scroll.cancel_to(0.0, 0.0);
        cx.notify();
    }

    fn sync_focus_after_transition(&mut self, window: &mut Window, cx: &mut App) {
        if self.state.is_open() {
            window.focus(&self.menu_focus, cx);
            self.reveal_highlight();
        } else {
            window.focus(&self.trigger_focus, cx);
        }
    }

    fn reveal_highlight(&mut self) {
        if let Some(index) = self.state.highlighted_index() {
            self.menu_scroll.scroll_to_item(index);
        }
    }

    fn render_trigger(&self, cx: &Context<Self>) -> Stateful<Div> {
        let foreground = self.theme.colors.foreground.to_paint();
        let muted = self.theme.colors.muted_foreground.to_paint();
        let engine = self.state.policy().map_or_else(
            || self.state.active_engine().to_owned(),
            |policy| policy.engine_id.clone(),
        );
        let summary = self.state.trigger_summary();
        let mut trigger = div()
            .id("artisan-native-model-selector-trigger")
            .track_focus(&self.trigger_focus)
            .debug_selector(|| NATIVE_MODEL_SELECTOR_TRIGGER_SELECTOR.to_owned())
            .role(gpui::Role::Button)
            .aria_label("Select model")
            .on_click(cx.listener(Self::handle_trigger_click))
            .flex()
            .items_center()
            .gap(px(8.0))
            .h(px(COMPACT_CONTROL_HEIGHT_PX))
            .max_w(px(360.0))
            .px(px(8.0))
            .rounded(px(10.0))
            .hover(move |style| style.bg(hover_fill_gradient(self.theme)))
            .focus_visible(move |style| style.shadow(source_focus_ring(self.theme)))
            .text_color(foreground);
        trigger = trigger.child(
            icon(IconStyle::resolve(
                self.theme,
                engine_asset(&engine),
                IconSize::Default,
                IconTint::Muted,
            ))
            .size(px(16.0))
            .flex_shrink_0(),
        );
        trigger = trigger.child(
            div()
                .flex()
                .items_center()
                .gap(px(4.0))
                .flex_1()
                .min_w(px(0.0))
                .overflow_hidden()
                .child(
                    div()
                        .truncate()
                        .text_size(px(14.0))
                        .line_height(px(20.0))
                        .child(self.state.trigger_label()),
                )
                .when(!summary.is_empty(), |body| {
                    body.child(
                        div()
                            .truncate()
                            .text_size(px(14.0))
                            .line_height(px(20.0))
                            .text_color(muted)
                            .child(summary),
                    )
                }),
        );
        trigger.child(
            icon(IconStyle::resolve(
                self.theme,
                AssetId::TABLER_SELECTOR,
                IconSize::Compact,
                IconTint::Muted,
            ))
            .size(px(14.0))
            .flex_shrink_0(),
        )
    }

    fn render_menu(&self, viewport: Size<Pixels>, cx: &Context<Self>) -> Option<AnyElement> {
        if self.menu_motion.borrow().phase() == PickerMenuPhase::Hidden {
            self.menu_bounds.borrow_mut().take();
            return None;
        }
        let bounds = Rc::clone(&self.menu_bounds);
        let bounds_probe = canvas(
            |_, _, _| {},
            move |painted, (), _, _| {
                *bounds.borrow_mut() = Some(painted);
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let origin = self.trigger_origin.borrow().as_ref().copied()?;
        let mut panel = div()
            .id("artisan-native-model-selector-menu")
            .on_mouse_down_out(cx.listener(Self::handle_outside_press))
            .occlude()
            .relative()
            .child(bounds_probe)
            .track_focus(&self.menu_focus)
            .debug_selector(|| NATIVE_MODEL_SELECTOR_MENU_SELECTOR.to_owned())
            .on_key_down(cx.listener(Self::handle_menu_key))
            .flex()
            .flex_col()
            .overflow_hidden()
            .w(menu_width_for_viewport(viewport))
            .max_h(menu_max_height_for_viewport(viewport))
            .p(px(8.0))
            .gap(px(8.0))
            .rounded(px(22.0))
            .backdrop_blur(glass_blur_radius(GlassStrength::Strong))
            .bg(glass_foreground_base(self.theme))
            .text_color(self.theme.colors.foreground.to_paint())
            .shadow(source_menu_shadows(self.theme));
        panel = panel.child(glass_material_layer(GlassStrength::Strong, px(22.0)));
        panel = panel.child(glass_highlight_layer(GlassStrength::Strong, px(22.0)));
        panel = panel.child(self.render_engine_tabs(cx));
        panel = panel.child(
            div()
                .flex()
                .flex_row()
                .h(px(MODEL_PANEL_HEIGHT_PX))
                .flex_shrink_0()
                .gap(px(8.0))
                .child(self.render_model_list(cx))
                .child(self.render_preview(viewport, cx)),
        );
        let motion = *self.menu_motion.borrow();
        Some(
            anchored()
                .anchor(Anchor::BottomLeft)
                .position(origin)
                .offset(point(px(0.0), px(-MENU_GAP_PX)))
                .child(animate_picker_menu(
                    panel,
                    self.menu_motion.clone(),
                    motion,
                    "main",
                ))
                .into_any_element(),
        )
    }

    fn render_engine_tabs(&self, cx: &Context<Self>) -> Stateful<Div> {
        let surface_bounds = Rc::clone(&self.engine_surface_bounds);
        let surface_probe = canvas(
            |_, _, _| {},
            move |bounds, (), window, cx| {
                let changed = {
                    let mut surface = surface_bounds.borrow_mut();
                    if *surface == Some(bounds) {
                        false
                    } else {
                        *surface = Some(bounds);
                        true
                    }
                };
                if changed {
                    window.defer(cx, |window, _| window.refresh());
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let active_engine = self.state.active_engine().to_owned();
        let transition = *self.engine_indicator_transition.borrow();
        let indicator = {
            let indicator = self.engine_indicator.borrow();
            indicator.indicator_visible().then(|| {
                render_engine_light(
                    self.theme,
                    px(indicator.indicator_left() as f32),
                    px(indicator.indicator_width() as f32),
                    transition,
                )
            })
        };
        let mut tabs = div()
            .id("artisan-model-engine-tabs")
            .relative()
            .flex()
            .flex_row()
            .w_full()
            .h(px(40.0))
            .gap(px(4.0))
            .p(px(4.0))
            .rounded(px(10.0))
            .overflow_x_scroll()
            .overflow_y_hidden()
            .scrollbar_width(px(0.0))
            .bg(source_control_gradient(self.theme))
            .shadow(source_card_shadows(self.theme))
            .child(surface_probe);
        if let Some(indicator) = indicator {
            tabs = tabs.child(indicator);
        }
        for harness in &self.state.snapshot().manifest.harnesses {
            let engine_id = harness.id.clone();
            let measured_engine_id = engine_id.clone();
            let indicator_policy = Rc::clone(&self.engine_indicator);
            let surface_bounds = Rc::clone(&self.engine_surface_bounds);
            let indicator_transition = Rc::clone(&self.engine_indicator_transition);
            let animation_generation = Rc::clone(&self.engine_indicator_animation_generation);
            let active_engine = active_engine.clone();
            let tab_probe = canvas(
                |_, _, _| {},
                move |bounds, (), window, cx| {
                    if measured_engine_id != active_engine {
                        return;
                    }
                    let Some(surface) = *surface_bounds.borrow() else {
                        return;
                    };
                    let measurement = EngineSectionIndicatorMeasurement::new(
                        f64::from(f32::from(surface.left())),
                        f64::from(f32::from(bounds.left())),
                        f64::from(f32::from(bounds.size.width)),
                    );
                    let changed = {
                        let mut indicator = indicator_policy.borrow_mut();
                        let previous_visible = indicator.indicator_visible();
                        let previous_left = indicator.indicator_left();
                        let previous_width = indicator.indicator_width();
                        let engine_changed =
                            indicator.lit_engine() != Some(measured_engine_id.as_str());
                        let next_left = measurement.tab_left - measurement.surface_left;
                        let geometry_changed =
                            previous_left != next_left || previous_width != measurement.tab_width;
                        if !previous_visible || engine_changed || geometry_changed {
                            indicator.measure(measured_engine_id.clone(), Some(measurement));
                            if previous_visible && engine_changed {
                                let mut generation = animation_generation.borrow_mut();
                                *generation = generation.saturating_add(1);
                                *indicator_transition.borrow_mut() =
                                    Some(EngineIndicatorTransition {
                                        from_left: previous_left,
                                        from_width: previous_width,
                                        to_left: next_left,
                                        to_width: measurement.tab_width,
                                        generation: *generation,
                                    });
                            } else {
                                // Resize remeasurement follows the source's
                                // instant geometry correction rather than
                                // replaying a tab-change animation.
                                *indicator_transition.borrow_mut() = None;
                            }
                            true
                        } else {
                            false
                        }
                    };
                    if changed {
                        window.defer(cx, |window, _| window.refresh());
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full();
            let selector = format!(
                "{NATIVE_MODEL_SELECTOR_ENGINE_SELECTOR_PREFIX}-{}",
                harness.id
            );
            let mut tab = div()
                .id(format!(
                    "{NATIVE_MODEL_SELECTOR_ENGINE_SELECTOR_PREFIX}-{}",
                    harness.id
                ))
                .debug_selector(move || selector.clone())
                .on_click(cx.listener(move |view: &mut Self, _: &ClickEvent, _, cx| {
                    view.switch_engine(engine_id.clone(), cx);
                }))
                .role(gpui::Role::Button)
                .aria_label(harness.label.clone())
                .flex()
                .items_center()
                .justify_center()
                .size(px(32.0))
                .flex_shrink_0()
                .rounded(px(14.0))
                .text_color(self.theme.colors.foreground.to_paint())
                .child(tab_probe);
            tab = tab.child(
                icon(IconStyle::resolve(
                    self.theme,
                    engine_asset(&harness.id),
                    IconSize::Compact,
                    IconTint::Inherit,
                ))
                .size(px(16.0))
                .flex_shrink_0(),
            );
            tabs = tabs.child(tab);
        }
        tabs
    }

    fn render_model_list(&self, cx: &Context<Self>) -> Stateful<Div> {
        let groups = self.state.model_groups();
        let visible_ids = groups
            .iter()
            .flat_map(|group| group.models.iter().map(|model| model.id.clone()))
            .collect::<Vec<_>>();
        self.model_hover.borrow_mut().clear_if_missing(&visible_ids);
        let model_surface_bounds = Rc::clone(&self.model_hover_surface_bounds);
        let surface_probe = canvas(
            |_, _, _| {},
            move |bounds, (), window, cx| {
                let changed = {
                    let mut surface = model_surface_bounds.borrow_mut();
                    if *surface == Some(bounds) {
                        false
                    } else {
                        *surface = Some(bounds);
                        true
                    }
                };
                if changed {
                    window.defer(cx, |window, _| window.refresh());
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let mut list = div()
            .id("artisan-native-model-selector-model-list")
            .relative()
            .flex()
            .flex_col()
            .w_full()
            .min_w(px(0.0))
            .h(px(MODEL_PANEL_HEIGHT_PX))
            .min_h(px(0.0))
            .overflow_y_scroll()
            .scrollbar_width(px(0.0))
            .track_scroll(&self.menu_scroll)
            .child(surface_probe)
            .child(render_picker_hover_pill(
                self.theme,
                Rc::clone(&self.model_hover),
                "model",
                px(14.0),
                cx.reduce_motion(),
            ))
            .gap(px(3.0));
        if groups.is_empty() {
            return list;
        }
        let show_group_headers =
            groups.len() > 1 || groups.first().is_some_and(|group| group.id != "default");
        let mut content = div()
            .relative()
            .flex()
            .flex_col()
            .w_full()
            .min_w(px(0.0))
            .flex_shrink_0()
            .gap(px(3.0))
            .on_scroll_wheel(cx.listener(Self::handle_model_scroll_wheel));
        for group in groups {
            let mut section = div().flex().flex_col().gap(px(2.0));
            if show_group_headers {
                let header = div()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .px(px(10.0))
                    .py(px(4.0))
                    .rounded(px(10.0))
                    .text_size(px(12.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(self.theme.colors.muted_foreground.to_paint())
                    .child(
                        icon(IconStyle::resolve(
                            self.theme,
                            AssetId::TABLER_CHEVRON_RIGHT,
                            IconSize::Compact,
                            IconTint::Muted,
                        ))
                        .size(px(14.0)),
                    )
                    .child(group.label);
                section = section.child(header);
            }
            for model in group.models {
                section = section.child(self.render_model_row(model, cx));
            }
            content = content.child(section);
        }
        list = list.child(content);
        let scroll = self.menu_scroll.clone();
        let top = self
            .theme
            .surfaces
            .value(SurfaceStep::S800)
            .with_alpha(0.14);
        let bottom = self
            .theme
            .surfaces
            .value(SurfaceStep::S800)
            .with_alpha(0.14);
        let fade = canvas(
            |_, _, _| {},
            move |bounds, (), window, _| {
                let offset = f32::from(scroll.offset().y);
                let maximum = f32::from(scroll.max_offset().y);
                let above = (-offset).clamp(0.0, 24.0);
                let below = (maximum + offset).clamp(0.0, 24.0);
                if above > 0.0 {
                    window.paint_quad(gpui::fill(
                        Bounds::new(bounds.origin, gpui::size(bounds.size.width, px(above))),
                        vertical_gradient(top, top.with_alpha(0.0)),
                    ));
                }
                if below > 0.0 {
                    window.paint_quad(gpui::fill(
                        Bounds::new(
                            point(bounds.left(), bounds.bottom() - px(below)),
                            gpui::size(bounds.size.width, px(below)),
                        ),
                        vertical_gradient(bottom.with_alpha(0.0), bottom),
                    ));
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        div()
            .id("artisan-model-list-frame")
            .relative()
            .flex_1()
            .min_w(px(0.0))
            .h(px(MODEL_PANEL_HEIGHT_PX))
            .child(list)
            .child(fade)
    }

    fn render_model_row(&self, model: NativeModelView, cx: &Context<Self>) -> Stateful<Div> {
        let definition_disabled = self.state.model_definition_disabled(&model.id);
        let disabled_reason = self.state.model_definition_disabled_reason(&model.id);
        let model_id = model.id.clone();
        let hover_model_id = model.id.clone();
        let favorite_id = model.id.clone();
        let measured_model_id = model.id.clone();
        let model_hover = Rc::clone(&self.model_hover);
        let model_surface_bounds = Rc::clone(&self.model_hover_surface_bounds);
        let row_probe = canvas(
            |_, _, _| {},
            move |bounds, (), window, cx| {
                let Some(surface) = *model_surface_bounds.borrow() else {
                    return;
                };
                let rect = HoverRect {
                    left: f32::from(bounds.left() - surface.left()),
                    top: f32::from(bounds.top() - surface.top()),
                    width: f32::from(bounds.size.width),
                    height: f32::from(bounds.size.height),
                };
                if model_hover.borrow_mut().measure(&measured_model_id, rect) {
                    window.defer(cx, |window, _| window.refresh());
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let mut row = div()
            .id(format!(
                "{NATIVE_MODEL_SELECTOR_ROW_SELECTOR_PREFIX}-{}",
                model.id
            ))
            .debug_selector({
                let selector = format!("{NATIVE_MODEL_SELECTOR_ROW_SELECTOR_PREFIX}-{}", model.id);
                move || selector.clone()
            })
            .relative()
            .flex()
            .items_center()
            .gap(px(8.0))
            .h(px(MODEL_ROW_HEIGHT_PX))
            .px(px(10.0))
            .rounded(px(14.0))
            .when(definition_disabled, |row| row.opacity(0.58));
        row = row.on_hover(cx.listener(move |view: &mut Self, hovered: &bool, _, cx| {
            if *hovered && view.menu_is_interactive() {
                view.state.preview_model(&hover_model_id);
                view.model_hover
                    .borrow_mut()
                    .set_active(hover_model_id.clone());
                cx.notify();
            }
        }));
        if !definition_disabled {
            row = row.on_click(
                cx.listener(move |view: &mut Self, _: &ClickEvent, window, cx| {
                    view.choose_model(model_id.clone(), window, cx);
                }),
            );
        }
        row = row.child(row_probe);
        row = row.child(
            icon(IconStyle::resolve(
                self.theme,
                provider_asset(&model.provider_id, &model.engine_id),
                IconSize::Default,
                IconTint::Inherit,
            ))
            .size(px(20.0))
            .flex_shrink_0(),
        );
        let mut text = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .overflow_hidden()
            .child(
                div()
                    .truncate()
                    .text_size(px(14.0))
                    .line_height(px(20.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(model.name.clone()),
            )
            .child(
                div()
                    .truncate()
                    .text_size(px(12.0))
                    .line_height(px(16.0))
                    .text_color(self.theme.colors.muted_foreground.to_paint())
                    .child(model.lab.clone()),
            );
        if let Some(reason) = disabled_reason {
            text = text.child(
                div()
                    .truncate()
                    .text_size(px(10.0))
                    .line_height(px(12.0))
                    .text_color(self.theme.colors.muted_foreground.to_paint())
                    .child(reason),
            );
        }
        row = row.child(text);
        let mut favorite = div()
            .id(format!(
                "artisan-native-model-selector-favorite-{}",
                model.id
            ))
            .flex()
            .items_center()
            .justify_center()
            .size(px(28.0))
            .rounded_full()
            .role(gpui::Role::Button)
            .aria_label(if model.favorite {
                format!("Remove {} from favorites", model.name)
            } else {
                format!("Add {} to favorites", model.name)
            })
            .hover(|style| style.bg(hover_fill_gradient(self.theme)))
            .text_color(if model.favorite {
                self.theme.colors.favorite.to_paint()
            } else {
                self.theme.colors.muted_foreground.to_paint()
            })
            .child(
                icon(IconStyle::resolve(
                    self.theme,
                    if model.favorite {
                        AssetId::TABLER_STAR_FILLED
                    } else {
                        AssetId::TABLER_STAR
                    },
                    IconSize::Compact,
                    IconTint::Inherit,
                ))
                .size(px(16.0)),
            );
        if !definition_disabled {
            favorite =
                favorite.on_click(cx.listener(move |view: &mut Self, _: &ClickEvent, _, cx| {
                    view.toggle_favorite(favorite_id.clone(), cx);
                    cx.stop_propagation();
                }));
        }
        row = row.child(favorite);
        row
    }

    fn render_preview(&self, viewport: Size<Pixels>, cx: &Context<Self>) -> Stateful<Div> {
        let mut preview = div()
            .id("artisan-native-model-selector-preview")
            .flex()
            .flex_col()
            .justify_between()
            .h(px(MODEL_PANEL_HEIGHT_PX))
            .w(px(MODEL_PREVIEW_WIDTH_PX))
            .flex_shrink_0()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_y_scroll()
            .scrollbar_width(px(0.0))
            .gap(px(8.0))
            .p(px(10.0));
        let Some(model_id) = self.state.previewed_model_id() else {
            return preview;
        };
        let Some(model) = self.state.snapshot().manifest.model(model_id).cloned() else {
            return preview;
        };
        let view = self.preview_view();
        let context = model
            .capabilities
            .context_window_tokens
            .map(format_context_tokens);
        let mut summary = div().flex().flex_col().gap(px(4.0)).child(
            div()
                .flex()
                .items_baseline()
                .justify_between()
                .gap(px(8.0))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(14.0))
                        .line_height(px(20.0))
                        .child(model.name.clone()),
                )
                .when_some(context, |title, context| {
                    title.child(
                        div()
                            .flex_shrink_0()
                            .text_size(px(10.0))
                            .line_height(px(14.0))
                            .text_color(self.theme.colors.muted_foreground.to_paint())
                            .child(context),
                    )
                }),
        );
        if let Some(description) = model.description.clone() {
            summary = summary.child(
                div()
                    .text_size(px(12.0))
                    .line_height(px(16.0))
                    .text_color(self.theme.colors.muted_foreground.to_paint())
                    .child(description),
            );
        }
        preview = preview.child(summary);
        if self.state.preview_policy().is_some() {
            preview = preview.child(self.render_policy_controls(&view, viewport, cx));
        }
        preview
    }

    fn render_policy_controls(
        &self,
        model: &NativeModelView,
        viewport: Size<Pixels>,
        cx: &Context<Self>,
    ) -> Div {
        let mut controls = div().flex().flex_col().gap(px(6.0));
        for (axis, label) in [
            (NativePolicyAxis::Variant, "Variant"),
            (NativePolicyAxis::Thinking, "Thinking"),
            (NativePolicyAxis::Speed, "Speed"),
            (NativePolicyAxis::ContextWindow, "Context"),
            (NativePolicyAxis::Permission, "Permission"),
        ] {
            let options = self.axis_options(axis, model);
            let should_show = match axis {
                NativePolicyAxis::Thinking | NativePolicyAxis::ContextWindow => !options.is_empty(),
                NativePolicyAxis::Speed
                | NativePolicyAxis::Permission
                | NativePolicyAxis::Variant => options.len() > 1,
            };
            if !should_show {
                continue;
            }
            let value = options
                .iter()
                .find(|option| option.selected)
                .map(|option| option.label.clone())
                .unwrap_or_else(|| options[0].label.clone());
            controls = controls.child(self.render_axis_control(
                axis,
                label,
                value,
                self.state.model_definition_disabled(&model.id),
                viewport,
                cx,
            ));
        }
        controls
    }

    fn render_axis_control(
        &self,
        axis: NativePolicyAxis,
        label: &str,
        value: String,
        disabled: bool,
        viewport: Size<Pixels>,
        cx: &Context<Self>,
    ) -> Div {
        let axis_bounds = Rc::clone(&self.axis_trigger_bounds);
        let probe = canvas(
            |_, _, _| {},
            move |bounds, (), _, _| {
                axis_bounds.borrow_mut()[axis as usize] = Some(bounds);
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let axis_open = self.state.is_axis_open(axis);
        let mut control = div()
            .relative()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .child(probe);
        let mut trigger = div()
            .id(format!("artisan-native-model-selector-axis-{axis:?}"))
            .debug_selector(move || format!("artisan-model-policy-{axis:?}"))
            .role(gpui::Role::Button)
            .aria_label(format!("{label}: {value}"))
            .flex()
            .items_center()
            .gap(px(7.0))
            .h(px(POLICY_CONTROL_HEIGHT_PX))
            .px(px(8.0))
            .rounded(px(8.0))
            .bg(source_control_gradient(self.theme))
            .shadow(source_card_shadows(self.theme))
            .focus_visible(move |style| {
                let mut shadows = source_card_shadows(self.theme);
                shadows.extend(source_focus_ring(self.theme));
                style
                    .border_1()
                    .border_color(self.theme.colors.ring.to_paint())
                    .shadow(shadows)
            })
            .when(disabled, |trigger| trigger.opacity(0.58))
            .when(axis_open, |trigger| {
                let mut shadows = source_card_shadows(self.theme);
                shadows.extend(source_focus_ring(self.theme));
                trigger.shadow(shadows)
            })
            .child(
                icon(IconStyle::resolve(
                    self.theme,
                    axis_icon(axis),
                    IconSize::Compact,
                    IconTint::Muted,
                ))
                .size(px(14.0))
                .flex_shrink_0(),
            )
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w(px(0.0))
                    .justify_center()
                    .truncate()
                    .text_size(px(12.0))
                    .text_color(self.theme.colors.foreground.to_paint())
                    .child(value),
            )
            .child(
                icon(IconStyle::resolve(
                    self.theme,
                    AssetId::TABLER_SELECTOR,
                    IconSize::Compact,
                    IconTint::Muted,
                ))
                .size(px(14.0)),
            );
        if !disabled {
            trigger = trigger.on_click(cx.listener(
                move |view: &mut Self, _: &ClickEvent, window, cx| {
                    view.toggle_axis(axis, window, cx);
                },
            ));
        }
        control = control.child(trigger);
        let axis_popup_visible = self.axis_menu_motion_axis == Some(axis)
            && self.axis_menu_motion.borrow().phase() != PickerMenuPhase::Hidden;
        if axis_popup_visible
            && let Some(trigger_bounds) = self.axis_trigger_bounds.borrow()[axis as usize]
        {
            let popup_bounds = Rc::clone(&self.axis_menu_bounds);
            let axis_surface_bounds = Rc::clone(&self.axis_hover_surface_bounds);
            let popup_probe = canvas(
                |_, _, _| {},
                move |bounds, (), window, cx| {
                    let popup_changed = {
                        let mut popup = popup_bounds.borrow_mut();
                        if *popup == Some(bounds) {
                            false
                        } else {
                            *popup = Some(bounds);
                            true
                        }
                    };
                    let surface_changed = {
                        let mut surface = axis_surface_bounds.borrow_mut();
                        if *surface == Some(bounds) {
                            false
                        } else {
                            *surface = Some(bounds);
                            true
                        }
                    };
                    if popup_changed || surface_changed {
                        window.defer(cx, |window, _| window.refresh());
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full();
            let mut options = div()
                .id(format!(
                    "artisan-native-model-selector-axis-options-{axis:?}"
                ))
                .debug_selector(|| "artisan-model-policy-options".to_owned())
                .relative()
                .occlude()
                .child(popup_probe)
                .w(trigger_bounds.size.width)
                .min_w(px(DROPDOWN_MIN_WIDTH_PX))
                .flex()
                .flex_col()
                .p(px(4.0))
                .max_h(dropdown_max_height_for_viewport(viewport, trigger_bounds))
                .overflow_hidden()
                .rounded(px(18.0))
                .backdrop_blur(glass_blur_radius(GlassStrength::Strong))
                .bg(glass_foreground_base(self.theme))
                .shadow(glass_card_shadows());
            options = options.child(glass_material_layer(GlassStrength::Strong, px(18.0)));
            options = options.child(glass_highlight_layer(GlassStrength::Strong, px(18.0)));
            options = options.child(render_picker_hover_pill(
                self.theme,
                Rc::clone(&self.axis_hover),
                "axis",
                px(14.0),
                cx.reduce_motion(),
            ));
            let dropdown_max_height = dropdown_max_height_for_viewport(viewport, trigger_bounds);
            let viewport_max_height = px((f32::from(dropdown_max_height) - 8.0).max(0.0));
            let mut viewport = div()
                .id(format!(
                    "artisan-native-model-selector-axis-viewport-{axis:?}"
                ))
                .relative()
                .w_full()
                .min_w(px(0.0))
                .min_h(px(0.0))
                .flex_shrink_0()
                .max_h(viewport_max_height)
                .overflow_y_scroll()
                .scrollbar_width(px(0.0))
                .track_scroll(&self.axis_menu_scroll);
            let mut content = div()
                .relative()
                .flex()
                .flex_col()
                .w_full()
                .min_w(px(0.0))
                .flex_shrink_0()
                .id("artisan-model-policy-hover-surface")
                .on_hover(cx.listener(|view: &mut Self, hovered: &bool, _, cx| {
                    if !*hovered {
                        view.axis_hover.borrow_mut().clear();
                        view.clear_option_tooltip();
                        cx.notify();
                    }
                }))
                .on_scroll_wheel(cx.listener(Self::handle_axis_scroll_wheel));
            let mut current_thinking_group: Option<String> = None;
            let axis_options = self.axis_options(axis, &self.preview_view());
            let visible_option_ids = axis_options
                .iter()
                .map(|option| option.id.clone())
                .collect::<Vec<_>>();
            self.axis_hover
                .borrow_mut()
                .clear_if_missing(&visible_option_ids);
            for option in axis_options {
                let group = option.group.clone();
                if axis == NativePolicyAxis::Thinking && group != current_thinking_group {
                    if current_thinking_group.is_some() {
                        content = content.child(
                            div().mx(px(8.0)).my(px(4.0)).h(px(1.0)).bg(self
                                .theme
                                .colors
                                .border
                                .with_alpha(self.theme.colors.border.a * 0.4)
                                .to_paint()),
                        );
                    }
                    if let Some(group) = group.as_deref() {
                        content = content.child(
                            div()
                                .px(px(12.0))
                                .pt(px(6.0))
                                .pb(px(4.0))
                                .text_size(px(10.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(
                                    self.theme
                                        .colors
                                        .muted_foreground
                                        .with_alpha(0.75)
                                        .to_paint(),
                                )
                                .child(group.to_owned()),
                        );
                    }
                    current_thinking_group = group;
                }
                content = content.child(self.render_axis_option(axis, option, cx));
            }
            viewport = viewport.child(content);
            options = options.child(viewport);
            control = control.child(
                deferred(
                    anchored()
                        .anchor(Anchor::BottomLeft)
                        .position(trigger_bounds.origin)
                        .offset(point(px(0.0), px(-DROPDOWN_GAP_PX)))
                        .child(animate_picker_menu(
                            options,
                            self.axis_menu_motion.clone(),
                            *self.axis_menu_motion.borrow(),
                            "axis-options",
                        )),
                )
                .with_priority(2),
            );
        }
        control
    }

    fn render_axis_option(
        &self,
        axis: NativePolicyAxis,
        option: SelectorOption,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let option_id = option.id.clone();
        let option_disabled = option.disabled;
        let option_label = option.label.clone();
        let option_selector = format!("artisan-model-policy-option-{axis:?}-{}", option.id);
        let option_description = option.description.clone();
        let option_advisory = option.advisory.clone();
        let measured_option_id = option.id.clone();
        let measured_tooltip_key = option_tooltip_key(axis, &option.id);
        let axis_hover = Rc::clone(&self.axis_hover);
        let axis_surface_bounds = Rc::clone(&self.axis_hover_surface_bounds);
        let option_tooltip = Rc::clone(&self.option_tooltip);
        let row_probe = canvas(
            |_, _, _| {},
            move |bounds, (), window, cx| {
                let Some(surface) = *axis_surface_bounds.borrow() else {
                    return;
                };
                let rect = HoverRect {
                    left: f32::from(bounds.left() - surface.left()),
                    top: f32::from(bounds.top() - surface.top()),
                    width: f32::from(bounds.size.width),
                    height: f32::from(bounds.size.height),
                };
                if axis_hover.borrow_mut().measure(&measured_option_id, rect) {
                    window.defer(cx, |window, _| window.refresh());
                }
                let tooltip_changed = {
                    let mut tooltip = option_tooltip.borrow_mut();
                    tooltip.as_mut().is_some_and(|target| {
                        if target.key != measured_tooltip_key {
                            return false;
                        }
                        if target.row_bounds == Some(bounds) {
                            false
                        } else {
                            target.row_bounds = Some(bounds);
                            true
                        }
                    })
                };
                if tooltip_changed {
                    window.defer(cx, |window, _| window.refresh());
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let tooltip_description =
            option_tooltip_text(option_advisory.as_deref(), option_description.as_deref());
        let tooltip_key = option_tooltip_key(axis, &option.id);
        let tooltip_option = option.clone();
        let mut row = div()
            .id(format!(
                "artisan-native-model-selector-option-{axis:?}-{}",
                option.id
            ))
            .debug_selector(move || option_selector.clone())
            .role(gpui::Role::Button)
            .aria_label(format!("{axis:?}: {option_label}"))
            .relative()
            .flex()
            .flex_row()
            .items_center()
            .w_full()
            .min_w(px(0.0))
            .gap(px(10.0))
            .pl(px(12.0))
            .pr(px(32.0))
            .py(px(8.0))
            .rounded(px(14.0))
            .text_size(px(14.0))
            .line_height(px(20.0))
            .when(option.disabled, |row| row.opacity(0.5))
            .child(row_probe)
            .child(div().flex_1().min_w(px(0.0)).truncate().child(option_label));
        row = row.on_hover(cx.listener(move |view: &mut Self, hovered: &bool, _, cx| {
            if *hovered {
                if !view.axis_is_interactive(axis) {
                    return;
                }
                if !option_disabled {
                    view.axis_hover
                        .borrow_mut()
                        .set_active(tooltip_option.id.clone());
                }
                view.begin_option_tooltip(axis, &tooltip_option, cx);
            } else {
                view.clear_option_tooltip_for(&tooltip_key);
            }
            cx.notify();
        }));
        if let Some(tooltip_description) = tooltip_description {
            row = row.aria_description(tooltip_description);
        }
        if !option_disabled {
            row = row.on_click(
                cx.listener(move |view: &mut Self, _: &ClickEvent, window, cx| {
                    view.choose_axis(axis, option_id.clone(), window, cx);
                }),
            );
        }
        if option.selected {
            row = row.child(
                div()
                    .absolute()
                    .right(px(8.0))
                    .top_0()
                    .bottom(px(0.0))
                    .w(px(14.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        icon(IconStyle::resolve(
                            self.theme,
                            AssetId::TABLER_CHECK,
                            IconSize::Compact,
                            IconTint::Muted,
                        ))
                        .size(px(14.0)),
                    ),
            );
        }
        row
    }

    fn axis_options(&self, axis: NativePolicyAxis, model: &NativeModelView) -> Vec<SelectorOption> {
        let policy = self.state.preview_policy();
        let model_definition_disabled = self.state.model_definition_disabled(&model.id);
        match axis {
            NativePolicyAxis::Variant => self
                .state
                .snapshot()
                .models_for_engine(&model.engine_id, "", Some(model.id.as_str()))
                .into_iter()
                .filter(|candidate| same_model_family(&model, candidate))
                .map(|candidate| SelectorOption {
                    id: candidate.id.clone(),
                    label: candidate
                        .variant_label
                        .as_deref()
                        .map(humanize_variant)
                        .unwrap_or_else(|| candidate.name.clone()),
                    description: None,
                    advisory: self.state.model_definition_disabled_reason(&candidate.id),
                    group: None,
                    selected: candidate.id == model.id,
                    disabled: self.state.model_definition_disabled(&candidate.id),
                })
                .collect(),
            NativePolicyAxis::Thinking => match &model.capabilities.thinking {
                NativeThinkingCapability::Supported { options, .. } => options
                    .iter()
                    .map(|option| SelectorOption {
                        id: option.id.clone(),
                        label: humanize_variant(&option.id),
                        description: option.description.clone(),
                        advisory: option.advisory.clone(),
                        group: thinking_group_label(&option.presentation_group).map(str::to_owned),
                        selected: policy
                            .as_ref()
                            .and_then(|policy| policy.reasoning_effort.as_ref())
                            .is_some_and(|value| value.id == option.id),
                        disabled: model_definition_disabled,
                    })
                    .collect(),
                NativeThinkingCapability::Unavailable | NativeThinkingCapability::Native { .. } => {
                    Vec::new()
                }
            },
            NativePolicyAxis::Speed => model
                .capabilities
                .speed_options
                .iter()
                .filter(|option| option.disabled.is_none())
                .map(|option| SelectorOption {
                    id: option.id.clone(),
                    label: option.label.clone(),
                    description: (!option.description.is_empty())
                        .then(|| option.description.clone()),
                    advisory: None,
                    group: None,
                    selected: policy
                        .as_ref()
                        .and_then(|policy| policy.speed.as_ref())
                        .is_some_and(|value| value.id == option.id),
                    disabled: model_definition_disabled,
                })
                .collect(),
            NativePolicyAxis::ContextWindow => model
                .capabilities
                .context_window
                .as_ref()
                .map(|capability| {
                    capability
                        .options
                        .iter()
                        .map(|option| SelectorOption {
                            id: option.id.clone(),
                            label: option.label.clone(),
                            description: option.description.clone(),
                            advisory: option.advisory.clone(),
                            group: None,
                            selected: policy
                                .as_ref()
                                .and_then(|policy| policy.context_window.as_ref())
                                .is_some_and(|value| value.id == option.id),
                            disabled: model_definition_disabled,
                        })
                        .collect()
                })
                .unwrap_or_default(),
            NativePolicyAxis::Permission => self
                .state
                .snapshot()
                .manifest
                .harness(&model.engine_id)
                .map(|harness| {
                    harness
                        .permissions
                        .options
                        .iter()
                        .map(|option| SelectorOption {
                            id: option.id.clone(),
                            label: option.label.clone(),
                            description: (!option.description.is_empty())
                                .then(|| option.description.clone()),
                            advisory: None,
                            group: None,
                            selected: policy
                                .as_ref()
                                .and_then(|policy| policy.permission.as_ref())
                                .is_some_and(|value| value.id == option.id),
                            disabled: model_definition_disabled,
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    fn preview_view(&self) -> NativeModelView {
        let Some(model_id) = self.state.previewed_model_id() else {
            return fallback_model_view_from_state(&self.state);
        };
        let Some(model) = self.state.snapshot().manifest.model(model_id) else {
            return fallback_model_view_from_state(&self.state);
        };
        self.state
            .snapshot()
            .models_for_engine(&model.harness, "", Some(model_id))
            .into_iter()
            .find(|row| row.id == model_id)
            .unwrap_or_else(|| fallback_model_view(model))
    }
}

impl Render for NativeModelSelector {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let viewport = window.viewport_size();
        let menu = self.render_menu(viewport, cx);
        let option_tooltip = self.render_option_tooltip(viewport);
        let trigger = self.render_trigger(cx);
        let origin = Rc::clone(&self.trigger_origin);
        let trigger_bounds = Rc::clone(&self.trigger_bounds);
        let scroll = self.menu_scroll.clone();
        let probe = canvas(
            move |_, _, _| {},
            move |bounds, (), window, cx| {
                *trigger_bounds.borrow_mut() = Some(bounds);
                let moved = *origin.borrow_mut() != Some(bounds.origin);
                *origin.borrow_mut() = Some(bounds.origin);
                if moved {
                    let scroll = scroll.clone();
                    window.defer(cx, move |window, _| {
                        scroll.scroll_to_item(0);
                        window.refresh();
                    });
                }
            },
        )
        .absolute()
        .size_full();
        div()
            .id("artisan-native-model-selector-root")
            .tab_group()
            .flex()
            .flex_col()
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .child(probe)
                    .children(menu.map(deferred))
                    .child(trigger),
            )
            .children(option_tooltip)
    }
}

impl NativeModelSelector {
    fn render_option_tooltip(&self, viewport: Size<Pixels>) -> Option<AnyElement> {
        let target = self.option_tooltip.borrow().clone()?;
        let row_bounds = target.row_bounds?;
        if !target.visible {
            return None;
        }

        let viewport_width = f32::from(viewport.width).max(1.0);
        let right_space = viewport_width - f32::from(row_bounds.right()) - OPTION_TOOLTIP_GAP_PX;
        let left_space = f32::from(row_bounds.left()) - OPTION_TOOLTIP_GAP_PX;
        let show_right = right_space >= OPTION_TOOLTIP_WIDTH_PX || right_space >= left_space;
        let available_width = if show_right { right_space } else { left_space };
        let width = OPTION_TOOLTIP_WIDTH_PX.min(available_width.max(1.0));
        let (anchor, position, offset) = if show_right {
            (
                Anchor::LeftCenter,
                point(row_bounds.right(), row_bounds.center().y),
                point(px(OPTION_TOOLTIP_GAP_PX), px(0.0)),
            )
        } else {
            (
                Anchor::RightCenter,
                point(row_bounds.left(), row_bounds.center().y),
                point(px(-OPTION_TOOLTIP_GAP_PX), px(0.0)),
            )
        };
        Some(
            deferred(
                anchored()
                    .anchor(anchor)
                    .position(position)
                    .offset(offset)
                    .child(render_option_tooltip_surface(
                        self.theme,
                        width,
                        target.advisory,
                        target.description,
                    )),
            )
            .with_priority(3)
            .into_any_element(),
        )
    }
}

fn render_option_tooltip_surface(
    theme: ArtisanTheme,
    width: f32,
    advisory: Option<String>,
    description: Option<String>,
) -> AnyElement {
    let advisory = advisory.filter(|text| !text.is_empty());
    let description = description.filter(|text| !text.is_empty());
    let mut text = String::new();
    let mut advisory_end = 0;
    if let Some(advisory) = advisory {
        text.push_str(&advisory);
        advisory_end = text.len();
        if description.is_some() {
            text.push(' ');
        }
    }
    if let Some(description) = description {
        text.push_str(&description);
    }
    let body = if advisory_end == 0 {
        StyledText::new(SharedString::from(text))
    } else {
        StyledText::new(SharedString::from(text)).with_highlights([(
            0..advisory_end,
            HighlightStyle {
                color: Some(theme.colors.destructive.to_paint()),
                font_weight: Some(FontWeight::MEDIUM),
                ..Default::default()
            },
        )])
    };
    div()
        .id("artisan-native-model-selector-option-tooltip")
        .debug_selector(|| "artisan-native-model-selector-option-tooltip".to_owned())
        .w(px(width))
        .max_w(px(OPTION_TOOLTIP_WIDTH_PX))
        .px(px(12.0))
        .py(px(8.0))
        .rounded(px(18.0))
        .backdrop_blur(glass_blur_radius(GlassStrength::Strong))
        .bg(glass_foreground_base(theme))
        .shadow(glass_card_shadows())
        .text_size(px(12.0))
        .line_height(px(16.0))
        .whitespace_normal()
        .text_color(theme.colors.muted_foreground.to_paint())
        .relative()
        .overflow_hidden()
        .child(glass_material_layer(GlassStrength::Strong, px(18.0)))
        .child(glass_highlight_layer(GlassStrength::Strong, px(18.0)))
        .child(body)
        .into_any_element()
}

pub(crate) fn render_picker_hover_pill(
    theme: ArtisanTheme,
    hover: Rc<RefCell<SlidingHoverState>>,
    surface: &'static str,
    corner_radius: Pixels,
    reduce_motion: bool,
) -> AnyElement {
    let (mut rect, visible, transition) = {
        let hover = hover.borrow();
        (hover.visual_rect(), hover.visible(), hover.transition())
    };
    if reduce_motion {
        if let Some(transition) = transition {
            rect = transition.to;
            hover
                .borrow_mut()
                .apply_progress(transition.generation, 1.0);
        }
    }
    let pill = div()
        .id(format!("artisan-native-model-picker-hover-{surface}"))
        .absolute()
        .left(px(rect.left))
        .top(px(rect.top))
        .w(px(rect.width))
        .h(px(rect.height))
        .rounded(corner_radius)
        .bg(hover_fill_gradient(theme))
        .shadow(source_hover_highlight_shadow(theme))
        .opacity(if visible { 1.0 } else { 0.0 });
    if !reduce_motion {
        if let Some(transition) = transition {
            let motion = Rc::clone(&hover);
            let from = transition.from;
            let to = transition.to;
            let generation = transition.generation;
            let animation_id = ElementId::Name(
                format!("artisan-native-model-picker-hover-{surface}-{generation}").into(),
            );
            return pill
                .with_animation(
                    animation_id,
                    Animation::new(Duration::from_millis(PICKER_HOVER_MOTION_DURATION_MS))
                        .with_easing(engine_light_smooth_out),
                    move |pill, progress| {
                        let rect = from.lerp(to, progress);
                        motion.borrow_mut().apply_progress(generation, progress);
                        pill.left(px(rect.left))
                            .top(px(rect.top))
                            .w(px(rect.width))
                            .h(px(rect.height))
                    },
                )
                .into_any_element();
        }
    }
    pill.into_any_element()
}

fn animate_picker_menu(
    panel: Stateful<Div>,
    motion: Rc<RefCell<PickerMenuMotion>>,
    snapshot: PickerMenuMotion,
    surface: &'static str,
) -> AnyElement {
    let Some((from_opacity, from_offset, to_opacity, to_offset, generation)) =
        snapshot.transition()
    else {
        return panel.into_any_element();
    };
    let phase = snapshot.phase();
    let animation_id = ElementId::Name(
        format!(
            "artisan-native-model-selector-menu-{surface}-{}-{generation}",
            match phase {
                PickerMenuPhase::Opening => "opening",
                PickerMenuPhase::Closing => "closing",
                PickerMenuPhase::Hidden | PickerMenuPhase::Open => "settled",
            }
        )
        .into(),
    );
    panel
        .top(px(from_offset))
        .opacity(from_opacity)
        .with_animation(
            animation_id,
            Animation::new(Duration::from_millis(PICKER_MENU_MOTION_DURATION_MS))
                .with_easing(engine_light_smooth_out),
            move |panel, progress| {
                motion.borrow_mut().apply_progress(generation, progress);
                panel
                    .top(px(from_offset + (to_offset - from_offset) * progress))
                    .opacity(from_opacity + (to_opacity - from_opacity) * progress)
            },
        )
        .into_any_element()
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

fn option_tooltip_text(advisory: Option<&str>, description: Option<&str>) -> Option<String> {
    let mut text = String::new();
    if let Some(advisory) = advisory.filter(|advisory| !advisory.is_empty()) {
        text.push_str(advisory);
    }
    if let Some(description) = description.filter(|description| !description.is_empty()) {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(description);
    }
    (!text.is_empty()).then_some(text)
}

fn option_tooltip_key(axis: NativePolicyAxis, option_id: &str) -> String {
    format!("{axis:?}:{option_id}")
}

fn selector_key_from_event(event: &KeyDownEvent) -> Option<NativeModelSelectorKey> {
    let key = event.keystroke.key.as_str();
    let modified = event.keystroke.modifiers.modified();
    match key {
        "down" if !modified => Some(NativeModelSelectorKey::ArrowDown),
        "up" if !modified => Some(NativeModelSelectorKey::ArrowUp),
        "home" if !modified => Some(NativeModelSelectorKey::Home),
        "end" if !modified => Some(NativeModelSelectorKey::End),
        "enter" if !modified => Some(NativeModelSelectorKey::Enter),
        "space" if !modified => Some(NativeModelSelectorKey::Space),
        "escape" => Some(NativeModelSelectorKey::Escape),
        "tab" => Some(NativeModelSelectorKey::Tab),
        _ => None,
    }
}

fn menu_width_for_viewport(viewport: Size<Pixels>) -> Pixels {
    let available = f32::from(viewport.width) - MENU_VIEWPORT_INSET_X_PX;
    px(MENU_WIDTH_PX.min(available.max(0.0)))
}

fn menu_max_height_for_viewport(viewport: Size<Pixels>) -> Pixels {
    let available = f32::from(viewport.height) - MENU_VIEWPORT_INSET_Y_PX;
    px(MENU_MAX_HEIGHT_PX.min(available.max(0.0)))
}

fn dropdown_max_height_for_viewport(
    viewport: Size<Pixels>,
    trigger_bounds: Bounds<Pixels>,
) -> Pixels {
    let viewport_height = f32::from(viewport.height);
    let available_above = f32::from(trigger_bounds.top()) - DROPDOWN_GAP_PX;
    let available_below = viewport_height - f32::from(trigger_bounds.bottom()) - DROPDOWN_GAP_PX;
    px(available_above.max(available_below).max(0.0))
}

fn source_control_gradient(theme: ArtisanTheme) -> gpui::Background {
    let (top, bottom) = match theme.mode {
        ThemeMode::Light => (SurfaceStep::S225, SurfaceStep::S200),
        ThemeMode::Dark => (SurfaceStep::S800, SurfaceStep::S925),
    };
    vertical_gradient(theme.surfaces.value(top), theme.surfaces.value(bottom))
}

fn source_card_shadows(theme: ArtisanTheme) -> Vec<gpui::BoxShadow> {
    card_shadows(theme)
}

fn source_menu_shadows(_theme: ArtisanTheme) -> Vec<gpui::BoxShadow> {
    glass_card_shadows()
}

fn source_hover_highlight_shadow(theme: ArtisanTheme) -> Vec<gpui::BoxShadow> {
    vec![gpui::BoxShadow {
        color: theme.colors.foreground.with_alpha(0.13).to_paint(),
        offset: point(px(0.0), px(0.0)),
        blur_radius: px(0.0),
        spread_radius: px(0.5),
        inset: true,
    }]
}

fn source_focus_ring(theme: ArtisanTheme) -> Vec<gpui::BoxShadow> {
    vec![gpui::BoxShadow {
        color: theme.interaction.focus_ring_color.to_paint(),
        offset: point(px(0.0), px(0.0)),
        blur_radius: px(0.0),
        spread_radius: theme.interaction.focus_ring_width,
        inset: false,
    }]
}

fn render_engine_light(
    theme: ArtisanTheme,
    tab_left: Pixels,
    tab_width: Pixels,
    transition: Option<EngineIndicatorTransition>,
) -> AnyElement {
    let target_left = transition
        .map(|transition| centered_engine_light_left(transition.to_left, transition.to_width))
        .unwrap_or_else(|| {
            centered_engine_light_left(
                f64::from(f32::from(tab_left)),
                f64::from(f32::from(tab_width)),
            )
        });
    let image = engine_light_image(theme);
    let image = img(ImageSource::Render(image))
        .absolute()
        .top(px(-2.0))
        .left(px(target_left))
        .w(px(ENGINE_LIGHT_WIDTH_PX))
        .h(px(ENGINE_LIGHT_HEIGHT_PX));
    let Some(transition) = transition else {
        return image.into_any_element();
    };
    let from_left = centered_engine_light_left(transition.from_left, transition.from_width);
    let animation_id = ElementId::Name(
        format!(
            "artisan-native-model-engine-light-{}",
            transition.generation
        )
        .into(),
    );
    image
        .left(px(from_left))
        .with_animation(
            animation_id,
            Animation::new(std::time::Duration::from_millis(250))
                .with_easing(engine_light_smooth_out),
            move |image, progress| {
                image.left(px(from_left + ((target_left - from_left) * progress)))
            },
        )
        .into_any_element()
}

fn centered_engine_light_left(tab_left: f64, tab_width: f64) -> f32 {
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    let left = tab_left + ((tab_width - f64::from(ENGINE_LIGHT_WIDTH_PX)) / 2.0).max(0.0);
    left as f32
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn engine_light_smooth_out(progress: f32) -> f32 {
    MotionCurve::SmoothOut.sample(f64::from(progress)) as f32
}

const ENGINE_LIGHT_WIDTH_PX: f32 = 32.0;
const ENGINE_LIGHT_HEIGHT_PX: f32 = 24.0;
const ENGINE_LIGHT_RASTER_SCALE: u32 = 4;
const ENGINE_LIGHT_RASTER_WIDTH: u32 = 128;
const ENGINE_LIGHT_RASTER_HEIGHT: u32 = 96;

thread_local! {
    static ENGINE_LIGHT_IMAGES: RefCell<[Option<Arc<RenderImage>>; 2]> = RefCell::new([None, None]);
}

fn engine_light_image(theme: ArtisanTheme) -> Arc<RenderImage> {
    ENGINE_LIGHT_IMAGES.with(|images| {
        let mut images = images.borrow_mut();
        let slot = match theme.mode {
            ThemeMode::Light => 0,
            ThemeMode::Dark => 1,
        };
        images[slot]
            .get_or_insert_with(|| Arc::new(RenderImage::new(vec![engine_light_frame(theme)])))
            .clone()
    })
}

#[allow(clippy::cast_precision_loss)]
fn engine_light_frame(theme: ArtisanTheme) -> image::Frame {
    let foreground = theme.colors.foreground.to_srgb();
    let red = color_channel_to_byte(foreground.r);
    let green = color_channel_to_byte(foreground.g);
    let blue = color_channel_to_byte(foreground.b);
    // CSS filters run before the mask. Bake the 2px Gaussian into the cached
    // alpha image, with transparent padding for the filter's edge samples.
    let padding = 6 * ENGINE_LIGHT_RASTER_SCALE;
    let gradient = image::GrayImage::from_fn(
        ENGINE_LIGHT_RASTER_WIDTH + 2 * padding,
        ENGINE_LIGHT_RASTER_HEIGHT + 2 * padding,
        |x, y| {
            let inside = x >= padding
                && x < padding + ENGINE_LIGHT_RASTER_WIDTH
                && y >= padding
                && y < padding + ENGINE_LIGHT_RASTER_HEIGHT;
            let alpha = if inside {
                interpolate_profile(
                    (y - padding) as f32 / ENGINE_LIGHT_RASTER_HEIGHT as f32,
                    &[(0.0, 0.32), (0.26, 0.10), (0.52, 0.02), (0.74, 0.0)],
                )
            } else {
                0.0
            };
            image::Luma([color_channel_to_byte(alpha)])
        },
    );
    let blurred = image::imageops::blur(&gradient, 2.0 * ENGINE_LIGHT_RASTER_SCALE as f32);
    let buffer = image::ImageBuffer::from_fn(
        ENGINE_LIGHT_RASTER_WIDTH,
        ENGINE_LIGHT_RASTER_HEIGHT,
        |x, y| {
            let nx = (x as f32 + 0.5) / ENGINE_LIGHT_RASTER_WIDTH as f32;
            let ny = (y as f32 + 0.5) / ENGINE_LIGHT_RASTER_HEIGHT as f32;
            let distance = (((nx - 0.5) / 0.48).powi(2) + ((ny - 0.35) / 0.70).powi(2)).sqrt();
            let mask = interpolate_profile(
                distance,
                &[(0.0, 1.0), (0.42, 0.5), (0.68, 0.1), (0.88, 0.0)],
            );
            let alpha = f32::from(blurred.get_pixel(x + padding, y + padding)[0]) / 255.0;
            // RenderImage consumes BGRA, matching Image::to_image_data.
            image::Rgba([blue, green, red, color_channel_to_byte(alpha * mask)])
        },
    );
    image::Frame::new(buffer)
}

fn interpolate_profile(value: f32, stops: &[(f32, f32)]) -> f32 {
    let Some(&(first_position, first_alpha)) = stops.first() else {
        return 0.0;
    };
    if value <= first_position {
        return first_alpha;
    }
    for window in stops.windows(2) {
        let [(start_position, start_alpha), (end_position, end_alpha)] = window else {
            unreachable!("a two-item profile window is guaranteed by windows(2)");
        };
        if value <= *end_position {
            let span = *end_position - *start_position;
            if span <= f32::EPSILON {
                return *end_alpha;
            }
            let progress = (value - *start_position) / span;
            return start_alpha + ((*end_alpha - *start_alpha) * progress);
        }
    }
    stops.last().map_or(0.0, |(_, alpha)| *alpha)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn color_channel_to_byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn axis_icon(axis: NativePolicyAxis) -> AssetId {
    match axis {
        NativePolicyAxis::Variant | NativePolicyAxis::Thinking => AssetId::TABLER_BRAIN,
        NativePolicyAxis::Speed => AssetId::TABLER_BOLT_FILLED,
        NativePolicyAxis::ContextWindow => AssetId::TABLER_ARROWS_HORIZONTAL,
        NativePolicyAxis::Permission => AssetId::TABLER_LOCK,
    }
}

fn thinking_group_label(group: &str) -> Option<&'static str> {
    match group {
        "base" => Some("Efforts"),
        "special" => Some("Special Efforts"),
        _ => None,
    }
}

fn format_context_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 && tokens % 1_000_000 == 0 {
        format!("{}M", tokens / 1_000_000)
    } else if tokens >= 1_000 && tokens % 1_000 == 0 {
        format!("{}K", tokens / 1_000)
    } else {
        tokens.to_string()
    }
}

fn humanize_variant(value: &str) -> String {
    match value {
        "xhigh" => "Extra High".to_owned(),
        "minimal" => "Minimal".to_owned(),
        "medium" => "Medium".to_owned(),
        "high" => "High".to_owned(),
        "light" => "Light".to_owned(),
        "low" => "Low".to_owned(),
        "max" => "Max".to_owned(),
        "ultra" => "Ultra".to_owned(),
        "default" => "Default".to_owned(),
        other => {
            let mut chars = other.chars();
            chars.next().map_or_else(
                || other.to_owned(),
                |first| first.to_uppercase().collect::<String>() + chars.as_str(),
            )
        }
    }
}

fn same_model_family(left: &NativeModelView, right: &NativeModelView) -> bool {
    left.engine_id == right.engine_id
        && left.native_model_id == right.native_model_id
        && left
            .native_selection
            .as_ref()
            .map(|selection| selection.provider_route_id.as_str())
            == right
                .native_selection
                .as_ref()
                .map(|selection| selection.provider_route_id.as_str())
}

fn fallback_model_view(model: &NativeModelDefinition) -> NativeModelView {
    NativeModelView {
        id: model.id.clone(),
        name: model.name.clone(),
        lab: model.provider.clone(),
        provider_id: model.provider.clone(),
        engine_id: model.harness.clone(),
        description: model.description.clone(),
        native_model_id: model.native_model_id.clone(),
        native_selection: model.native_selection.clone(),
        route_label: None,
        variant_label: None,
        capabilities: model.capabilities.clone(),
        selected: false,
        favorite: false,
        available: false,
        unavailable_reason: None,
        source_index: 0,
    }
}

fn fallback_model_view_from_state(state: &NativeModelSelectorState) -> NativeModelView {
    state.snapshot().manifest.models.first().map_or_else(
        || NativeModelView {
            id: String::new(),
            name: "No model".to_owned(),
            lab: String::new(),
            provider_id: String::new(),
            engine_id: state.active_engine().to_owned(),
            description: None,
            native_model_id: String::new(),
            native_selection: None,
            route_label: None,
            variant_label: None,
            capabilities: crate::native_model_catalog::NativeModelCapabilities {
                context_window_tokens: None,
                context_window: None,
                image_input: false,
                local_tools: false,
                mcp: false,
                output_tokens: None,
                reasoning_display: None,
                speed_options: Vec::new(),
                thinking: NativeThinkingCapability::Unavailable,
                web_search: false,
            },
            selected: false,
            favorite: false,
            available: false,
            unavailable_reason: None,
            source_index: 0,
        },
        fallback_model_view,
    )
}

fn engine_asset(engine_id: &str) -> AssetId {
    match engine_id {
        "codex" => AssetId::SVGL_OPENAI,
        "claude" => AssetId::SVGL_CLAUDE_AI,
        "cursor" => AssetId::SVGL_CURSOR,
        "grok" => AssetId::SVGL_GROK,
        "opencode2" => AssetId::BRANDS_OPENCODE,
        "hermes" => AssetId::BRANDS_HERMES,
        _ => AssetId::TABLER_QUESTION_MARK,
    }
}

fn provider_asset(provider_id: &str, engine_id: &str) -> AssetId {
    match provider_id {
        "openai" => AssetId::SVGL_OPENAI,
        "anthropic" => AssetId::SVGL_CLAUDE_AI,
        "xai" => AssetId::SVGL_GROK,
        "cursor" => AssetId::SVGL_CURSOR,
        "opencode" => AssetId::BRANDS_OPENCODE,
        _ => engine_asset(engine_id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn clicks_in_engine_tabs_and_preview_do_not_dismiss_the_menu(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| {
            NativeModelSelector::new(
                NativeModelCatalog::offline().unwrap(),
                None,
                ThemeMode::Dark,
                cx,
            )
        });
        cx.simulate_resize(gpui::size(px(1000.0), px(800.0)));
        cx.run_until_parked();
        let trigger = cx
            .debug_bounds(NATIVE_MODEL_SELECTOR_TRIGGER_SELECTOR)
            .unwrap();
        cx.simulate_click(trigger.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("artisan-native-model-selector-search")
                .is_none()
        );
        assert!(cx.debug_bounds("artisan-model-selector-retry").is_none());
        let tab = cx
            .debug_bounds("artisan-native-model-selector-engine-claude")
            .unwrap();
        cx.simulate_click(tab.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, app| {
            assert!(view.read(app).state.is_open());
            assert_eq!(view.read(app).state.active_engine(), "claude");
            let picker = view.read(app);
            let surface = picker.engine_surface_bounds.borrow().unwrap();
            let expected_left = f64::from(f32::from(tab.left() - surface.left()));
            assert_eq!(
                picker.engine_indicator.borrow().indicator_left(),
                expected_left
            );
            assert_eq!(
                picker.engine_indicator_transition.borrow().unwrap().to_left,
                expected_left,
                "light animation coordinates must stay relative to the engine strip"
            );
        });
        let menu = cx
            .debug_bounds(NATIVE_MODEL_SELECTOR_MENU_SELECTOR)
            .unwrap();
        cx.simulate_click(
            point(menu.right() - px(15.0), menu.center().y),
            gpui::Modifiers::none(),
        );
        cx.run_until_parked();
        cx.update(|_, app| assert!(view.read(app).state.is_open()));
        cx.simulate_click(
            point(menu.right() + px(15.0), menu.center().y),
            gpui::Modifiers::none(),
        );
        cx.run_until_parked();
        cx.update(|_, app| assert!(!view.read(app).state.is_open()));
    }

    #[gpui::test]
    fn wheel_events_accumulate_once_and_precise_pixels_cancel_inertia(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|app| app.set_reduce_motion(true));
        let (view, cx) = cx.add_window_view(|_, cx| {
            NativeModelSelector::new(
                NativeModelCatalog::offline().unwrap(),
                None,
                ThemeMode::Dark,
                cx,
            )
        });
        cx.simulate_resize(gpui::size(px(1000.0), px(800.0)));
        cx.run_until_parked();
        let trigger = cx
            .debug_bounds(NATIVE_MODEL_SELECTOR_TRIGGER_SELECTOR)
            .unwrap();
        cx.simulate_click(trigger.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        let row = cx
            .debug_bounds("artisan-native-model-selector-row-codex-sol")
            .unwrap();
        let expected = cx.update(|window, app| {
            app.set_reduce_motion(false);
            let picker = view.read(app);
            let maximum = f32::from(picker.menu_scroll.max_offset().y);
            assert!(maximum > 0.0, "the fixture must scroll");
            (-3.0 * f32::from(window.line_height())).clamp(-maximum, 0.0)
        });
        cx.simulate_event(ScrollWheelEvent {
            position: row.center(),
            delta: gpui::ScrollDelta::Lines(point(0.0, -3.0)),
            modifiers: gpui::Modifiers::none(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        cx.update(|_, app| assert_eq!(view.read(app).model_scroll.target(), expected));
        let expected_pixel = cx.update(|_, app| {
            let picker = view.read(app);
            (f32::from(picker.menu_scroll.offset().y) - 7.0)
                .clamp(-f32::from(picker.menu_scroll.max_offset().y), 0.0)
        });
        cx.simulate_event(ScrollWheelEvent {
            position: row.center(),
            delta: gpui::ScrollDelta::Pixels(point(px(0.0), px(-7.0))),
            modifiers: gpui::Modifiers::none(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        cx.update(|_, app| {
            let picker = view.read(app);
            assert_eq!(f32::from(picker.menu_scroll.offset().y), expected_pixel);
            assert!(!picker.model_scroll.active());
        });
    }
    #[gpui::test]
    fn option_hover_slides_across_rows_and_clears_on_surface_departure(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|app| app.set_reduce_motion(true));
        let (view, cx) = cx.add_window_view(|_, cx| {
            NativeModelSelector::new(
                NativeModelCatalog::offline().unwrap(),
                None,
                ThemeMode::Dark,
                cx,
            )
        });
        cx.simulate_resize(gpui::size(px(1000.0), px(800.0)));
        cx.run_until_parked();
        let trigger = cx
            .debug_bounds(NATIVE_MODEL_SELECTOR_TRIGGER_SELECTOR)
            .unwrap();
        cx.simulate_click(trigger.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        let thinking = cx.debug_bounds("artisan-model-policy-Thinking").unwrap();
        cx.simulate_click(thinking.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, app| app.set_reduce_motion(false));
        let light = cx
            .debug_bounds("artisan-model-policy-option-Thinking-light")
            .unwrap();
        let medium = cx
            .debug_bounds("artisan-model-policy-option-Thinking-medium")
            .unwrap();
        cx.simulate_mouse_move(light.center(), None, gpui::Modifiers::none());
        cx.run_until_parked();
        cx.simulate_mouse_move(medium.center(), None, gpui::Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, app| {
            let picker = view.read(app);
            let hover = picker.axis_hover.borrow();
            assert_eq!(hover.active_id(), Some("medium"));
            assert!(
                hover.transition().is_some(),
                "sibling row departure must not reset the slide"
            );
        });
        cx.simulate_mouse_move(point(px(950.0), px(750.0)), None, gpui::Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, app| assert!(!view.read(app).axis_hover.borrow().visible()));
    }
    fn state_with_offline_catalog() -> NativeModelSelectorState {
        NativeModelSelectorState::new(
            NativeModelCatalog::offline().expect("the real bundled catalog must decode"),
            None,
        )
    }

    #[test]
    fn keyboard_commit_selects_policy_and_preserves_native_values() {
        let mut state = state_with_offline_catalog();
        state.press_trigger();
        assert_eq!(state.highlighted_model_id(), Some("codex-sol"));
        state.handle_key(NativeModelSelectorKey::ArrowDown);
        let event = state
            .handle_key(NativeModelSelectorKey::Enter)
            .expect("enter commits the highlighted row");
        let NativeModelSelectorEvent::SelectPolicy(policy) = event else {
            panic!("expected a policy event");
        };
        assert_eq!(policy.model_id, "codex-terra");
        assert_eq!(policy.native_model_id, "gpt-5.6-terra");
        assert_eq!(
            policy
                .reasoning_effort
                .as_ref()
                .map(|value| value.native_value.as_str()),
            Some("high")
        );
        assert!(!state.is_open());
    }

    #[test]
    fn escape_and_tab_cancel_preview_without_emitting_policy() {
        let mut state = state_with_offline_catalog();
        state.press_trigger();
        state.handle_key(NativeModelSelectorKey::ArrowDown);
        state.handle_key(NativeModelSelectorKey::Escape);
        assert!(!state.is_open());

        state.press_trigger();
        state.handle_key(NativeModelSelectorKey::ArrowDown);
        state.handle_key(NativeModelSelectorKey::Tab);
        assert!(!state.is_open());
    }

    #[test]
    fn favorite_event_is_explicit_and_does_not_fake_authoritative_state() {
        let mut state = state_with_offline_catalog();
        let event = state
            .toggle_favorite("codex-sol")
            .expect("offline model emits favorite intent");
        assert_eq!(
            event,
            NativeModelSelectorEvent::SetFavorite(SetFavorite {
                model_id: "codex-sol".to_owned(),
                favorite: true,
            })
        );
        assert!(!state.snapshot().is_favorite("codex-sol"));
    }

    #[test]
    fn offline_model_can_be_selected_without_runtime_readiness() {
        let mut state = state_with_offline_catalog();
        assert!(!state.snapshot().selectability("codex-sol").is_available());
        state.press_trigger();
        state.preview_model("codex-sol");
        assert_eq!(state.previewed_model_id(), Some("codex-sol"));
        assert!(matches!(
            state.select_model("codex-sol"),
            Some(NativeModelSelectorEvent::SelectPolicy(policy)) if policy.model_id == "codex-sol"
        ));
        assert!(!state.is_open());
    }

    #[test]
    fn invalid_option_is_rejected_without_runtime_readiness() {
        let mut state = state_with_offline_catalog();
        state.press_trigger();
        state.preview_model("codex-sol");
        assert!(matches!(
            state.choose_option(NativePolicyAxis::Thinking, "not-in-the-catalog"),
            Err(NativePolicyValidationError::InvalidThinkingOption { .. })
        ));
        assert!(matches!(
            state.choose_option(NativePolicyAxis::Variant, "not-in-the-catalog"),
            Err(NativePolicyValidationError::UnknownModel(model_id)) if model_id == "not-in-the-catalog"
        ));
    }

    #[test]
    fn xhigh_thinking_value_uses_source_label() {
        let wire_id = "xhigh";
        assert_eq!(humanize_variant(wire_id), "Extra High");
    }

    #[test]
    fn offline_rows_keep_runtime_readiness_out_of_picker_selection() {
        let mut state = state_with_offline_catalog();
        let row = state
            .model_groups()
            .into_iter()
            .flat_map(|group| group.models)
            .find(|model| model.id == "codex-sol")
            .expect("catalog model remains readable while offline");
        assert!(!row.available);
        assert!(!state.model_definition_disabled("codex-sol"));
        assert!(state.select_model("codex-sol").is_some());
        assert!(state.local_error.is_none());
    }

    #[gpui::test]
    fn pointer_click_selects_offline_model_without_runtime_configuration(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = cx.add_window_view(|_, cx| {
            NativeModelSelector::new(
                NativeModelCatalog::offline().expect("real catalog"),
                None,
                ThemeMode::Dark,
                cx,
            )
        });
        cx.simulate_resize(gpui::size(px(1000.0), px(800.0)));
        cx.run_until_parked();
        let trigger = cx
            .debug_bounds(NATIVE_MODEL_SELECTOR_TRIGGER_SELECTOR)
            .unwrap();
        cx.simulate_click(trigger.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        let row = cx
            .debug_bounds("artisan-native-model-selector-row-codex-sol")
            .expect("offline model row is painted");
        cx.simulate_click(row.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, app| {
            let selector = view.read(app);
            assert!(!selector.state.is_open());
            assert_eq!(selector.state.selected_model_id(), Some("codex-sol"));
        });
    }
    #[gpui::test]
    fn policy_popup_floats_and_accepts_clicks_outside_parent_bounds(cx: &mut gpui::TestAppContext) {
        cx.update(|app| app.set_reduce_motion(true));
        let (view, cx) = cx.add_window_view(|_, cx| {
            NativeModelSelector::new(
                NativeModelCatalog::offline().unwrap(),
                None,
                ThemeMode::Dark,
                cx,
            )
        });
        cx.simulate_resize(gpui::size(px(1000.0), px(800.0)));
        cx.run_until_parked();
        let trigger = cx
            .debug_bounds(NATIVE_MODEL_SELECTOR_TRIGGER_SELECTOR)
            .unwrap();
        cx.simulate_click(trigger.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        let menu = cx
            .debug_bounds(NATIVE_MODEL_SELECTOR_MENU_SELECTOR)
            .unwrap();
        assert_eq!(menu.size.width, px(480.0));
        assert!(menu.size.height <= px(264.0));
        let control = cx.debug_bounds("artisan-model-policy-Thinking").unwrap();
        assert_eq!(control.size.height, px(24.0));
        cx.simulate_click(control.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        assert_eq!(
            cx.debug_bounds(NATIVE_MODEL_SELECTOR_MENU_SELECTOR)
                .unwrap(),
            menu
        );
        let popup = cx.debug_bounds("artisan-model-policy-options").unwrap();
        cx.update(|_, app| {
            assert_eq!(
                f32::from(view.read(app).axis_menu_scroll.max_offset().y),
                0.0,
                "fitting policy options must not gain scroll extent from visual layers"
            );
        });
        let ids = cx.update(|_, app| {
            view.read(app)
                .axis_options(NativePolicyAxis::Thinking, &view.read(app).preview_view())
                .into_iter()
                .filter(|option| !option.disabled)
                .map(|option| option.id)
                .collect::<Vec<_>>()
        });
        let (id, bounds) = ids
            .into_iter()
            .filter_map(|id| {
                let bounds = cx.debug_bounds(Box::leak(
                    format!("artisan-model-policy-option-Thinking-{id}").into_boxed_str(),
                ))?;
                (popup.contains(&bounds.center()) && !menu.contains(&bounds.center()))
                    .then_some((id, bounds))
            })
            .next()
            .expect("an option is visible beyond the parent picker");
        cx.simulate_click(bounds.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, app| {
            let picker = view.read(app);
            assert!(picker.state.is_open());
            assert!(picker.state.open_axis.is_none());
            assert_eq!(
                picker
                    .state
                    .policy()
                    .unwrap()
                    .reasoning_effort
                    .as_ref()
                    .unwrap()
                    .id,
                id
            );
        });
    }

    #[gpui::test]
    fn policy_option_tooltip_is_a_full_side_overlay(cx: &mut gpui::TestAppContext) {
        cx.update(|app| app.set_reduce_motion(true));
        let (view, cx) = cx.add_window_view(|_, cx| {
            NativeModelSelector::new(
                NativeModelCatalog::offline().expect("real catalog"),
                None,
                ThemeMode::Dark,
                cx,
            )
        });
        cx.simulate_resize(gpui::size(px(1000.0), px(800.0)));
        cx.run_until_parked();
        let trigger = cx
            .debug_bounds(NATIVE_MODEL_SELECTOR_TRIGGER_SELECTOR)
            .unwrap();
        cx.simulate_click(trigger.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        let thinking = cx.debug_bounds("artisan-model-policy-Thinking").unwrap();
        cx.simulate_click(thinking.center(), gpui::Modifiers::none());
        cx.run_until_parked();

        let option_id = cx.update(|_, app| {
            view.read(app)
                .axis_options(NativePolicyAxis::Thinking, &view.read(app).preview_view())
                .into_iter()
                .find(|option| option.id == "ultra")
                .map(|option| option.id)
                .expect("the catalog fixture has a described policy option")
        });
        let option = cx
            .debug_bounds(Box::leak(
                format!("artisan-model-policy-option-Thinking-{option_id}").into_boxed_str(),
            ))
            .expect("described option is painted");
        cx.update(|_, app| app.set_reduce_motion(false));
        cx.simulate_mouse_move(option.center(), None, gpui::Modifiers::none());
        cx.run_until_parked();
        cx.executor()
            .advance_clock(Duration::from_millis(PICKER_TOOLTIP_SHOW_DELAY_MS));
        cx.run_until_parked();

        let tooltip = cx
            .debug_bounds("artisan-native-model-selector-option-tooltip")
            .expect("tooltip is mounted after the source delay");
        assert!(
            tooltip.left() >= option.right() + px(OPTION_TOOLTIP_GAP_PX)
                || tooltip.right() <= option.left() - px(OPTION_TOOLTIP_GAP_PX),
            "tooltip should be side-anchored with the source 8px gap"
        );
        assert!(
            tooltip.size.height >= px(96.0),
            "wrapped advisory/description must not be clipped to one line"
        );
        assert!(tooltip.size.width <= px(OPTION_TOOLTIP_WIDTH_PX));
    }

    #[gpui::test]
    fn settings_exit_can_be_reopened_and_switched_before_completion(cx: &mut gpui::TestAppContext) {
        cx.update(|app| app.set_reduce_motion(true));
        let (view, cx) = cx.add_window_view(|_, cx| {
            NativeModelSelector::new(
                NativeModelCatalog::offline().unwrap(),
                None,
                ThemeMode::Dark,
                cx,
            )
        });
        cx.simulate_resize(gpui::size(px(1000.0), px(800.0)));
        cx.run_until_parked();
        let trigger = cx
            .debug_bounds(NATIVE_MODEL_SELECTOR_TRIGGER_SELECTOR)
            .unwrap();
        cx.simulate_click(trigger.center(), gpui::Modifiers::none());
        cx.run_until_parked();

        cx.update(|window, app| {
            view.update(app, |picker, cx| {
                picker.toggle_axis(NativePolicyAxis::Thinking, window, cx)
            })
        });
        cx.run_until_parked();
        cx.update(|_, app| app.set_reduce_motion(false));
        cx.update(|window, app| {
            view.update(app, |picker, cx| {
                picker.toggle_axis(NativePolicyAxis::Thinking, window, cx)
            })
        });
        cx.run_until_parked();
        cx.update(|_, app| {
            let picker = view.read(app);
            assert!(picker.state.open_axis.is_none());
            assert_eq!(
                picker.axis_menu_motion.borrow().phase(),
                PickerMenuPhase::Closing
            );
            assert!(!picker.axis_is_interactive(NativePolicyAxis::Thinking));
        });
        assert!(
            cx.debug_bounds("artisan-model-policy-options").is_some(),
            "exit remains mounted"
        );
        cx.update(|window, app| {
            view.update(app, |picker, cx| {
                picker.toggle_axis(NativePolicyAxis::Thinking, window, cx)
            })
        });
        cx.run_until_parked();
        cx.update(|_, app| {
            assert!(
                view.read(app)
                    .state
                    .is_axis_open(NativePolicyAxis::Thinking)
            )
        });
        cx.executor().advance_clock(Duration::from_millis(110));
        cx.run_until_parked();
        cx.update(|_, app| {
            assert_eq!(
                view.read(app).axis_menu_motion.borrow().phase(),
                PickerMenuPhase::Open
            )
        });
        cx.update(|window, app| {
            view.update(app, |picker, cx| {
                picker.toggle_axis(NativePolicyAxis::Thinking, window, cx)
            })
        });
        cx.run_until_parked();

        cx.update(|window, app| {
            view.update(app, |picker, cx| {
                picker.toggle_axis(NativePolicyAxis::Speed, window, cx)
            })
        });
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(110));
        cx.run_until_parked();
        cx.update(|_, app| {
            let picker = view.read(app);
            assert!(picker.state.is_axis_open(NativePolicyAxis::Speed));
            assert_eq!(
                picker.axis_menu_motion.borrow().phase(),
                PickerMenuPhase::Open
            );
        });
        cx.update(|window, app| {
            view.update(app, |picker, cx| {
                picker.toggle_axis(NativePolicyAxis::Speed, window, cx)
            })
        });
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(110));
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("artisan-model-policy-options").is_none(),
            "finished exit unmounts"
        );
    }

    #[gpui::test]
    fn settings_scroll_only_when_the_real_options_exceed_the_viewport(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|app| app.set_reduce_motion(true));
        let (view, cx) = cx.add_window_view(|_, cx| {
            NativeModelSelector::new(
                NativeModelCatalog::offline().unwrap(),
                None,
                ThemeMode::Dark,
                cx,
            )
        });
        cx.simulate_resize(gpui::size(px(1000.0), px(800.0)));
        cx.run_until_parked();
        let trigger = cx
            .debug_bounds(NATIVE_MODEL_SELECTOR_TRIGGER_SELECTOR)
            .unwrap();
        cx.simulate_click(trigger.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        let thinking = cx.debug_bounds("artisan-model-policy-Thinking").unwrap();
        cx.simulate_click(thinking.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, app| assert_eq!(view.read(app).axis_menu_scroll.max_offset().y, px(0.0)));
        cx.simulate_resize(gpui::size(px(1000.0), px(280.0)));
        cx.run_until_parked();
        cx.update(|_, app| {
            assert!(
                view.read(app).axis_menu_scroll.max_offset().y > px(0.0),
                "short windows retain real scrolling"
            )
        });
        cx.simulate_resize(gpui::size(px(1000.0), px(800.0)));
        cx.run_until_parked();
        cx.update(|_, app| assert_eq!(view.read(app).axis_menu_scroll.max_offset().y, px(0.0)));
    }
}
