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

use std::{cell::RefCell, rc::Rc};

use artisan_assets::AssetId;
use artisan_ui::{
    gradient::{hover_fill_gradient, vertical_gradient},
    icon::{IconSize, IconStyle, IconTint, icon},
    theme::{ArtisanTheme, SurfaceStep, ThemeMode},
};
use gpui::{
    Anchor, AnyElement, App, AppContext as _, Bounds, ClickEvent, Context, Div, EventEmitter,
    FocusHandle, Focusable, FontWeight, InteractiveElement as _, KeyDownEvent, MouseDownEvent,
    ParentElement as _, Pixels, Point, Render, ScrollHandle, Size, Stateful,
    StatefulInteractiveElement as _, Styled as _, Window, anchored, canvas, deferred, div, point,
    prelude::FluentBuilder as _, prelude::IntoElement, px,
};

use crate::native_composer_material::{
    GLASS_BLUR_RADIUS_PX, glass_card_shadows, glass_highlight_layer, glass_material,
};
use crate::native_model_catalog::{
    NativeModelCatalog, NativeModelDefinition, NativeModelPolicy, NativeModelView,
    NativeOptionValue, NativePolicyValidationError, NativeThinkingCapability,
};

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
const MODEL_PANEL_HEIGHT_PX: f32 = 192.0;
const MODEL_PREVIEW_WIDTH_PX: f32 = 224.0;
const MODEL_ROW_HEIGHT_PX: f32 = 48.0;
const COMPACT_CONTROL_HEIGHT_PX: f32 = 32.0;
const POLICY_CONTROL_HEIGHT_PX: f32 = 24.0;

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
        values.join(" Â· ")
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

/// A GPUI entity that paints the model trigger and bounded selector popover.
pub struct NativeModelSelector {
    state: NativeModelSelectorState,
    theme: ArtisanTheme,
    trigger_focus: FocusHandle,
    menu_focus: FocusHandle,
    menu_scroll: ScrollHandle,
    trigger_origin: Rc<RefCell<Option<Point<Pixels>>>>,
    menu_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
    trigger_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
    axis_trigger_bounds: Rc<RefCell<[Option<Bounds<Pixels>>; 5]>>,
    axis_menu_bounds: Rc<RefCell<Option<Bounds<Pixels>>>>,
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
            trigger_origin: Rc::new(RefCell::new(None)),
            menu_bounds: Rc::new(RefCell::new(None)),
            trigger_bounds: Rc::new(RefCell::new(None)),
            axis_trigger_bounds: Rc::new(RefCell::new([None; 5])),
            axis_menu_bounds: Rc::new(RefCell::new(None)),
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
        self.state.set_snapshot(snapshot);
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

    fn handle_trigger_click(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.press_trigger();
        self.sync_focus_after_transition(window, cx);
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
                    None
                }
                _ => None,
            };
            if let Some(index) = next {
                self.highlighted_axis_option = Some(options[index].id.clone());
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
            window.focus(&self.trigger_focus, cx);
        } else if self.state.is_open() {
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
        self.state.dismiss();
        window.focus(&self.trigger_focus, cx);
        cx.notify();
    }

    fn choose_model(&mut self, model_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let emitted = self.state.select_model(&model_id);
        if let Some(event) = emitted {
            cx.emit(event);
        }
        self.sync_focus_after_transition(window, cx);
        cx.notify();
    }

    fn toggle_favorite(&mut self, model_id: String, cx: &mut Context<Self>) {
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
        match self.state.choose_option(axis, &option_id) {
            Ok(Some(event)) => cx.emit(event),
            Ok(None) => {}
            Err(error) => self.state.set_local_error(Some(error.to_string())),
        }
        if !self.state.is_open() {
            window.focus(&self.trigger_focus, cx);
        }
        cx.notify();
    }

    fn toggle_axis(&mut self, axis: NativePolicyAxis, cx: &mut Context<Self>) {
        self.highlighted_axis_option = None;
        self.axis_menu_bounds.borrow_mut().take();
        self.state.toggle_axis(axis);
        cx.notify();
    }

    fn switch_engine(&mut self, engine_id: String, cx: &mut Context<Self>) {
        self.state.set_active_engine(engine_id);
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
        if !self.state.is_open() {
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
            .backdrop_blur(px(GLASS_BLUR_RADIUS_PX))
            .bg(glass_material())
            .text_color(self.theme.colors.foreground.to_paint())
            .shadow(source_menu_shadows(self.theme));
        panel = panel.child(glass_highlight_layer(px(22.0)));
        panel = panel.child(self.render_engine_tabs(cx));
        panel = panel.child(
            div()
                .flex()
                .flex_row()
                .h(px(MODEL_PANEL_HEIGHT_PX))
                .flex_shrink_0()
                .gap(px(8.0))
                .child(self.render_model_list(cx))
                .child(self.render_preview(cx)),
        );
        Some(
            anchored()
                .anchor(Anchor::BottomLeft)
                .position(origin)
                .offset(point(px(0.0), px(-MENU_GAP_PX)))
                .child(panel)
                .into_any_element(),
        )
    }

    fn render_engine_tabs(&self, cx: &Context<Self>) -> Stateful<Div> {
        let mut tabs = div()
            .id("artisan-model-engine-tabs")
            .flex()
            .flex_row()
            .w_full()
            .h(px(40.0))
            .gap(px(4.0))
            .p(px(4.0))
            .rounded(px(10.0))
            .overflow_x_scroll()
            .scrollbar_width(px(0.0))
            .bg(source_control_gradient(self.theme))
            .shadow(source_card_shadows(self.theme));
        for harness in &self.state.snapshot().manifest.harnesses {
            let selected = harness.id == self.state.active_engine();
            let engine_id = harness.id.clone();
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
                .rounded(px(6.0))
                .hover(|style| style.bg(hover_fill_gradient(self.theme)))
                .text_color(if selected {
                    self.theme.colors.foreground.to_paint()
                } else {
                    self.theme.colors.muted_foreground.to_paint()
                });
            if selected {
                tab = tab.bg(hover_fill_gradient(self.theme));
            }
            tab = tab.child(
                icon(IconStyle::resolve(
                    self.theme,
                    engine_asset(&harness.id),
                    IconSize::Compact,
                    if selected {
                        IconTint::Inherit
                    } else {
                        IconTint::Muted
                    },
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
        let mut list = div()
            .id("artisan-native-model-selector-model-list")
            .flex()
            .flex_col()
            .w_full()
            .min_w(px(0.0))
            .h(px(MODEL_PANEL_HEIGHT_PX))
            .min_h(px(0.0))
            .overflow_y_scroll()
            .scrollbar_width(px(0.0))
            .track_scroll(&self.menu_scroll)
            .gap(px(3.0));
        if groups.is_empty() {
            return list;
        }
        let show_group_headers =
            groups.len() > 1 || groups.first().is_some_and(|group| group.id != "default");
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
            list = list.child(section);
        }
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
        let selected = model.selected;
        let highlighted = self.state.highlighted_model_id() == Some(model.id.as_str());
        let definition_disabled = self.state.model_definition_disabled(&model.id);
        let disabled_reason = self.state.model_definition_disabled_reason(&model.id);
        let model_id = model.id.clone();
        let hover_model_id = model.id.clone();
        let favorite_id = model.id.clone();
        let mut row = div()
            .id(format!(
                "{NATIVE_MODEL_SELECTOR_ROW_SELECTOR_PREFIX}-{}",
                model.id
            ))
            .debug_selector({
                let selector = format!("{NATIVE_MODEL_SELECTOR_ROW_SELECTOR_PREFIX}-{}", model.id);
                move || selector.clone()
            })
            .flex()
            .items_center()
            .gap(px(8.0))
            .h(px(MODEL_ROW_HEIGHT_PX))
            .px(px(10.0))
            .rounded(px(12.0))
            .hover(|style| style.bg(hover_fill_gradient(self.theme)))
            .when(
                highlighted || (selected && self.state.highlighted_model_id().is_none()),
                |row| row.bg(hover_fill_gradient(self.theme)),
            )
            .when(definition_disabled, |row| row.opacity(0.58));
        row = row.on_hover(cx.listener(move |view: &mut Self, hovered: &bool, _, cx| {
            if *hovered {
                view.state.preview_model(&hover_model_id);
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

    fn render_preview(&self, cx: &Context<Self>) -> Stateful<Div> {
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
            preview = preview.child(self.render_policy_controls(&view, cx));
        }
        preview
    }

    fn render_policy_controls(&self, model: &NativeModelView, cx: &Context<Self>) -> Div {
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
            .shadow(
                self.theme
                    .elevation
                    .card_shadow
                    .into_iter()
                    .map(|layer| layer.to_box_shadow())
                    .collect(),
            )
            .when(disabled, |trigger| trigger.opacity(0.58))
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
            trigger =
                trigger.on_click(cx.listener(move |view: &mut Self, _: &ClickEvent, _, cx| {
                    view.toggle_axis(axis, cx);
                }));
        }
        if self.state.is_axis_open(axis) {
            trigger = trigger.bg(hover_fill_gradient(self.theme));
        }
        control = control.child(trigger);
        if self.state.is_axis_open(axis)
            && let Some(trigger_bounds) = self.axis_trigger_bounds.borrow()[axis as usize]
        {
            let popup_bounds = Rc::clone(&self.axis_menu_bounds);
            let popup_probe = canvas(
                |_, _, _| {},
                move |bounds, (), _, _| {
                    *popup_bounds.borrow_mut() = Some(bounds);
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
                .flex()
                .flex_col()
                .max_h(px(220.0))
                .overflow_y_scroll()
                .scrollbar_width(px(0.0))
                .p(px(4.0))
                .gap(px(2.0))
                .rounded(px(18.0))
                .backdrop_blur(px(GLASS_BLUR_RADIUS_PX))
                .bg(glass_material())
                .shadow(source_card_shadows(self.theme));
            options = options.child(glass_highlight_layer(px(18.0)));
            let mut current_thinking_group: Option<String> = None;
            for option in self.axis_options(axis, &self.preview_view()) {
                let group = option.group.clone();
                if axis == NativePolicyAxis::Thinking && group != current_thinking_group {
                    if current_thinking_group.is_some() {
                        options = options.child(
                            div().mx(px(8.0)).my(px(3.0)).h(px(1.0)).bg(self
                                .theme
                                .colors
                                .border
                                .with_alpha(0.4)
                                .to_paint()),
                        );
                    }
                    if let Some(group) = group.as_deref() {
                        options = options.child(
                            div()
                                .px(px(9.0))
                                .pt(px(6.0))
                                .pb(px(3.0))
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
                options = options.child(self.render_axis_option(axis, option, cx));
            }
            control = control.child(
                deferred(
                    anchored()
                        .anchor(Anchor::TopLeft)
                        .position(point(
                            trigger_bounds.left(),
                            trigger_bounds.bottom() + px(4.0),
                        ))
                        .child(options),
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
        let highlighted = self.highlighted_axis_option.as_ref() == Some(&option.id);
        let option_selector = format!("artisan-model-policy-option-{axis:?}-{}", option.id);
        let option_description = option.description.clone();
        let option_advisory = option.advisory.clone();
        let tooltip_description =
            option_tooltip_text(option_advisory.as_deref(), option_description.as_deref());
        let mut row = div()
            .id(format!(
                "artisan-native-model-selector-option-{axis:?}-{}",
                option.id
            ))
            .debug_selector(move || option_selector.clone())
            .role(gpui::Role::Button)
            .aria_label(format!("{axis:?}: {option_label}"))
            .flex()
            .flex_col()
            .gap(px(1.0))
            .p(px(6.0))
            .rounded(px(10.0))
            .text_size(px(11.0))
            .hover(|row| row.bg(hover_fill_gradient(self.theme)))
            .when(
                highlighted || (option.selected && self.highlighted_axis_option.is_none()),
                |row| row.bg(hover_fill_gradient(self.theme)),
            )
            .when(option.disabled, |row| row.opacity(0.5))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(option_label),
            );
        if let Some(tooltip_description) = tooltip_description {
            let tooltip_advisory = option_advisory.clone();
            let tooltip_copy = option_description.clone();
            let tooltip_theme = self.theme;
            row = row
                .aria_description(tooltip_description.clone())
                .tooltip(move |_, cx| {
                    let advisory = tooltip_advisory.clone();
                    let description = tooltip_copy.clone();
                    cx.new(move |_| NativeModelSelectorOptionTooltip {
                        theme: tooltip_theme,
                        advisory,
                        description,
                    })
                    .into()
                });
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
                icon(IconStyle::resolve(
                    self.theme,
                    AssetId::TABLER_CHECK,
                    IconSize::Compact,
                    IconTint::Muted,
                ))
                .size(px(12.0))
                .absolute()
                .right(px(5.0))
                .top(px(5.0)),
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
    }
}

struct NativeModelSelectorOptionTooltip {
    theme: ArtisanTheme,
    advisory: Option<String>,
    description: Option<String>,
}

impl Render for NativeModelSelectorOptionTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let mut tooltip = div()
            .flex()
            .flex_col()
            .max_w(px(320.0))
            .gap(px(2.0))
            .px(px(12.0))
            .py(px(8.0))
            .rounded(px(18.0))
            .backdrop_blur(px(GLASS_BLUR_RADIUS_PX))
            .bg(glass_material())
            .shadow(source_card_shadows(self.theme))
            .text_size(px(12.0))
            .line_height(px(16.0))
            .text_color(self.theme.colors.muted_foreground.to_paint())
            .relative()
            .overflow_hidden()
            .child(glass_highlight_layer(px(18.0)));
        if let Some(advisory) = self.advisory.clone() {
            tooltip = tooltip.child(
                div()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(self.theme.colors.destructive.to_paint())
                    .child(advisory),
            );
        }
        if let Some(description) = self.description.clone() {
            tooltip = tooltip.child(div().child(description));
        }
        tooltip
    }
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

fn source_control_gradient(theme: ArtisanTheme) -> gpui::Background {
    let (top, bottom) = match theme.mode {
        ThemeMode::Light => (SurfaceStep::S225, SurfaceStep::S200),
        ThemeMode::Dark => (SurfaceStep::S800, SurfaceStep::S925),
    };
    vertical_gradient(theme.surfaces.value(top), theme.surfaces.value(bottom))
}

fn source_card_shadows(_theme: ArtisanTheme) -> Vec<gpui::BoxShadow> {
    glass_card_shadows()
}

fn source_menu_shadows(_theme: ArtisanTheme) -> Vec<gpui::BoxShadow> {
    glass_card_shadows()
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
        "xhigh" => "X-High".to_owned(),
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
}
