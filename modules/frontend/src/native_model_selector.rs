//! Reusable GPUI model-selector popover for the native composer.
//!
//! The selector mirrors the Electron model picker at the presentation
//! boundary: the full static catalog remains readable when disconnected,
//! runtime availability is shown instead of guessed, and every mutation is an
//! explicit event for the application owner. Persistence and backend
//! authority stay outside this entity.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::{cell::RefCell, rc::Rc};

use artisan_assets::AssetId;
use artisan_ui::{
    icon::{IconSize, IconStyle, IconTint, icon},
    theme::{ArtisanTheme, ThemeMode},
};
use gpui::{
    Anchor, AnyElement, App, ClickEvent, Context, Div, EventEmitter, FocusHandle, Focusable,
    FontWeight, InteractiveElement as _, KeyDownEvent, MouseDownEvent, ParentElement as _, Pixels,
    Point, Render, ScrollHandle, Size, Stateful, StatefulInteractiveElement as _, Styled as _,
    Window, anchored, canvas, deferred, div, point, prelude::FluentBuilder as _,
    prelude::IntoElement, px,
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

const MENU_WIDTH_PX: f32 = 560.0;
const MENU_MAX_HEIGHT_PX: f32 = 460.0;
const MENU_VIEWPORT_INSET_X_PX: f32 = 24.0;
const MENU_VIEWPORT_INSET_Y_PX: f32 = 24.0;
const MENU_GAP_PX: f32 = 8.0;
const MODEL_LIST_WIDTH_PX: f32 = 244.0;
const MODEL_ROW_HEIGHT_PX: f32 = 48.0;
const COMPACT_CONTROL_HEIGHT_PX: f32 = 32.0;

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
}

/// Owner-controlled persistence/status feedback.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NativeModelSelectorStatus {
    /// A policy or favorite request is currently being persisted.
    pub saving: bool,
    /// Last owner-reported persistence/runtime error.
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
        let policy = policy.and_then(|policy| snapshot.rebase_policy(&policy));
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
    /// compatible option values. A model that becomes unavailable remains
    /// displayable as an authoritative selection; it is not newly admitted.
    pub fn set_snapshot(&mut self, snapshot: NativeModelCatalog) {
        let policy = self
            .policy
            .as_ref()
            .and_then(|policy| snapshot.rebase_policy(policy));
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
                self.policy = None;
                true
            }
            Some(policy) => {
                let Some(policy) = self.snapshot.rebase_policy(&policy) else {
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
            .map_or_else(|| "Select model".to_owned(), |model| model.name.clone())
    }

    /// Returns the compact trigger's exact policy-axis summary.
    #[must_use]
    pub fn trigger_summary(&self) -> String {
        let Some(policy) = &self.policy else {
            return String::new();
        };
        let mut values = Vec::new();
        if let Some(value) = &policy.reasoning_effort {
            values.push(value.id.clone());
        }
        if let Some(value) = &policy.speed {
            values.push(value.id.clone());
        }
        if let Some(value) = &policy.context_window {
            values.push(value.id.clone());
        }
        values.join(" · ")
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
            self.highlighted_model_id = self
                .snapshot
                .selectability(model_id)
                .is_available()
                .then(|| model_id.to_owned());
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

    /// Selects a model by exact catalog ID. Unavailable models remain preview
    /// targets but cannot emit a runnable policy.
    pub fn select_model(&mut self, model_id: &str) -> Option<NativeModelSelectorEvent> {
        self.preview_model(model_id);
        let policy = match self.snapshot.policy_for_model(model_id) {
            Ok(policy) => policy,
            Err(error) => {
                self.local_error = Some(error.to_string());
                return None;
            }
        };
        self.policy = Some(policy.clone());
        self.open = false;
        self.open_axis = None;
        self.previewed_model_id = Some(model_id.to_owned());
        self.highlighted_model_id = None;
        self.local_error = None;
        Some(NativeModelSelectorEvent::SelectPolicy(policy))
    }

    /// Applies one exact option ID/native value to the preview policy.
    pub fn choose_option(
        &mut self,
        axis: NativePolicyAxis,
        option_id: &str,
    ) -> Result<Option<NativeModelSelectorEvent>, NativePolicyValidationError> {
        if axis == NativePolicyAxis::Variant {
            return Ok(self.select_model(option_id));
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
            self.snapshot.policy_for_model(&model_id)?
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
        self.snapshot.admit_policy(&policy)?;
        self.policy = Some(policy.clone());
        self.local_error = None;
        self.open_axis = None;
        Ok(Some(NativeModelSelectorEvent::SelectPolicy(policy)))
    }

    /// Emits a favorite intent without mutating the authoritative favorite
    /// list. The owner must send a new snapshot after persistence.
    pub fn toggle_favorite(&mut self, model_id: &str) -> Option<NativeModelSelectorEvent> {
        if !self
            .snapshot
            .manifest
            .models
            .iter()
            .any(|model| model.id == model_id)
            || !self.snapshot.selectability(model_id).is_available()
        {
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
            .find(|row| row.available)
            .map(|row| row.id.clone());
    }

    fn move_highlight(&mut self, direction: isize) -> Option<NativeModelSelectorEvent> {
        let rows = self
            .visible_models()
            .into_iter()
            .filter(|row| row.available)
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
            .filter(|row| row.available)
            .collect::<Vec<_>>();
        let row = if last { rows.last() } else { rows.first() }?;
        self.highlighted_model_id = Some(row.id.clone());
        self.previewed_model_id = Some(row.id.clone());
        None
    }

    fn set_local_error(&mut self, error: Option<String>) {
        self.local_error = error;
    }
}

/// A GPUI entity that paints the model trigger and bounded selector popover.
pub struct NativeModelSelector {
    state: NativeModelSelectorState,
    theme: ArtisanTheme,
    trigger_focus: FocusHandle,
    menu_focus: FocusHandle,
    menu_scroll: ScrollHandle,
    trigger_origin: Rc<RefCell<Option<Point<Pixels>>>>,
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
        if !self.state.is_open() || self.menu_scroll.bounds().contains(&event.position) {
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
        let surface = self.theme.colors.muted.to_paint();
        let engine = self.state.policy().map_or_else(
            || self.state.active_engine().to_owned(),
            |policy| policy.engine_id.clone(),
        );
        let summary = self.state.trigger_summary();
        let mut trigger = div()
            .id("artisan-native-model-selector-trigger")
            .track_focus(&self.trigger_focus)
            .debug_selector(|| NATIVE_MODEL_SELECTOR_TRIGGER_SELECTOR.to_owned())
            .on_click(cx.listener(Self::handle_trigger_click))
            .flex()
            .items_center()
            .gap(px(7.0))
            .h(px(COMPACT_CONTROL_HEIGHT_PX))
            .max_w(px(300.0))
            .px(px(8.0))
            .rounded(px(8.0))
            .bg(surface)
            .text_color(foreground);
        trigger = trigger.child(
            icon(IconStyle::resolve(
                self.theme,
                engine_asset(&engine),
                IconSize::Compact,
                IconTint::Muted,
            ))
            .size(px(14.0))
            .flex_shrink_0(),
        );
        trigger = trigger.child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w(px(0.0))
                .overflow_hidden()
                .child(
                    div()
                        .truncate()
                        .text_size(px(13.0))
                        .line_height(px(16.0))
                        .child(self.state.trigger_label()),
                )
                .when(!summary.is_empty(), |body| {
                    body.child(
                        div()
                            .truncate()
                            .text_size(px(10.0))
                            .line_height(px(12.0))
                            .text_color(muted)
                            .child(summary),
                    )
                }),
        );
        trigger.child(
            icon(IconStyle::resolve(
                self.theme,
                AssetId::TABLER_CHEVRON_DOWN,
                IconSize::Compact,
                IconTint::Muted,
            ))
            .size(px(14.0))
            .flex_shrink_0(),
        )
    }

    fn render_menu(&self, viewport: Size<Pixels>, cx: &Context<Self>) -> Option<AnyElement> {
        if !self.state.is_open() {
            return None;
        }
        let origin = self.trigger_origin.borrow().as_ref().copied()?;
        let mut panel = div()
            .id("artisan-native-model-selector-menu")
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
            .rounded(px(14.0))
            .bg(self.theme.colors.popover.to_paint())
            .border_1()
            .border_color(self.theme.colors.border.to_paint());
        panel = panel.child(self.render_search());
        panel = panel.child(self.render_engine_tabs(cx));
        panel = panel.child(
            div()
                .flex()
                .flex_row()
                .flex_1()
                .min_h(px(0.0))
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

    fn render_search(&self) -> Stateful<Div> {
        let query = self.state.query();
        let value = if query.is_empty() {
            "Search models…".to_owned()
        } else {
            query.to_owned()
        };
        div()
            .id("artisan-native-model-selector-search")
            .flex()
            .items_center()
            .gap(px(7.0))
            .h(px(32.0))
            .px(px(9.0))
            .rounded(px(8.0))
            .bg(self.theme.colors.muted.to_paint())
            .text_size(px(12.0))
            .text_color(if query.is_empty() {
                self.theme.colors.muted_foreground.to_paint()
            } else {
                self.theme.colors.foreground.to_paint()
            })
            .child(
                icon(IconStyle::resolve(
                    self.theme,
                    AssetId::TABLER_SEARCH,
                    IconSize::Compact,
                    IconTint::Muted,
                ))
                .size(px(14.0))
                .flex_shrink_0(),
            )
            .child(div().flex_1().min_w(px(0.0)).truncate().child(value))
            .child(
                div()
                    .text_size(px(10.0))
                    .text_color(self.theme.colors.muted_foreground.to_paint())
                    .child("Type to filter"),
            )
    }

    fn render_engine_tabs(&self, cx: &Context<Self>) -> Div {
        let mut tabs = div()
            .flex()
            .flex_row()
            .gap(px(3.0))
            .overflow_x_hidden()
            .border_b_1()
            .border_color(self.theme.colors.border.to_paint());
        for harness in &self.state.snapshot().manifest.harnesses {
            let selected = harness.id == self.state.active_engine();
            let engine_id = harness.id.clone();
            let mut tab = div()
                .id(format!(
                    "{NATIVE_MODEL_SELECTOR_ENGINE_SELECTOR_PREFIX}-{}",
                    harness.id
                ))
                .on_click(cx.listener(move |view: &mut Self, _: &ClickEvent, _, cx| {
                    view.switch_engine(engine_id.clone(), cx);
                }))
                .flex()
                .items_center()
                .gap(px(5.0))
                .px(px(7.0))
                .py(px(6.0))
                .rounded(px(6.0))
                .text_size(px(11.0))
                .font_weight(if selected {
                    FontWeight::MEDIUM
                } else {
                    FontWeight::NORMAL
                })
                .text_color(if selected {
                    self.theme.colors.foreground.to_paint()
                } else {
                    self.theme.colors.muted_foreground.to_paint()
                });
            if selected {
                tab = tab.bg(self.theme.colors.accent.to_paint());
            }
            tab = tab.child(
                icon(IconStyle::resolve(
                    self.theme,
                    engine_asset(&harness.id),
                    IconSize::Compact,
                    IconTint::Muted,
                ))
                .size(px(14.0))
                .flex_shrink_0(),
            );
            tabs = tabs.child(tab.child(harness.label.clone()));
        }
        tabs
    }

    fn render_model_list(&self, cx: &Context<Self>) -> Stateful<Div> {
        let groups = self.state.model_groups();
        let mut list = div()
            .id("artisan-native-model-selector-model-list")
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(MODEL_LIST_WIDTH_PX))
            .min_h(px(0.0))
            .overflow_y_scroll()
            .track_scroll(&self.menu_scroll)
            .gap(px(3.0));
        if groups.is_empty() {
            return list.child(
                div()
                    .p(px(12.0))
                    .text_size(px(12.0))
                    .text_color(self.theme.colors.muted_foreground.to_paint())
                    .child("No models match this filter."),
            );
        }
        let show_group_headers =
            groups.len() > 1 || groups.first().is_some_and(|group| group.id != "default");
        for group in groups {
            let mut section = div().flex().flex_col().gap(px(2.0));
            if show_group_headers {
                let mut header = div()
                    .flex()
                    .items_center()
                    .gap(px(5.0))
                    .px(px(5.0))
                    .py(px(3.0))
                    .text_size(px(10.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(self.theme.colors.muted_foreground.to_paint())
                    .child(group.label);
                if !group.available {
                    header = header.child(
                        icon(IconStyle::resolve(
                            self.theme,
                            AssetId::TABLER_ALERT_TRIANGLE,
                            IconSize::Compact,
                            IconTint::Muted,
                        ))
                        .size(px(12.0))
                        .text_color(self.theme.colors.destructive.to_paint()),
                    );
                }
                section = section.child(header);
            }
            for model in group.models {
                section = section.child(self.render_model_row(model, cx));
            }
            list = list.child(section);
        }
        list
    }

    fn render_model_row(&self, model: NativeModelView, cx: &Context<Self>) -> Stateful<Div> {
        let selected = model.selected;
        let highlighted = self.state.highlighted_model_id() == Some(model.id.as_str());
        let model_id = model.id.clone();
        let favorite_id = model.id.clone();
        let metadata = model
            .variant_label
            .as_deref()
            .map(humanize_variant)
            .or_else(|| model.route_label.as_deref().map(str::to_owned));
        let mut row = div()
            .id(format!(
                "{NATIVE_MODEL_SELECTOR_ROW_SELECTOR_PREFIX}-{}",
                model.id
            ))
            .debug_selector({
                let selector = format!("{NATIVE_MODEL_SELECTOR_ROW_SELECTOR_PREFIX}-{}", model.id);
                move || selector.clone()
            })
            .on_click(
                cx.listener(move |view: &mut Self, _: &ClickEvent, window, cx| {
                    view.choose_model(model_id.clone(), window, cx);
                }),
            )
            .flex()
            .items_center()
            .gap(px(7.0))
            .h(px(MODEL_ROW_HEIGHT_PX))
            .px(px(7.0))
            .rounded(px(8.0))
            .when(highlighted, |row| {
                row.bg(self.theme.colors.muted.to_paint())
            })
            .when(selected, |row| row.bg(self.theme.colors.accent.to_paint()))
            .when(!model.available, |row| row.opacity(0.58));
        row = row.child(
            icon(IconStyle::resolve(
                self.theme,
                provider_asset(&model.provider_id, &model.engine_id),
                IconSize::Default,
                IconTint::Muted,
            ))
            .size(px(16.0))
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
                    .text_size(px(12.0))
                    .line_height(px(16.0))
                    .child(model.name.clone()),
            )
            .child(
                div()
                    .truncate()
                    .text_size(px(10.0))
                    .line_height(px(13.0))
                    .text_color(self.theme.colors.muted_foreground.to_paint())
                    .child(metadata.map_or_else(
                        || model.lab.clone(),
                        |metadata| format!("{} · {}", model.lab, metadata),
                    )),
            );
        if !model.available
            && let Some(reason) = model.unavailable_reason.clone()
        {
            text = text.child(
                div()
                    .truncate()
                    .text_size(px(9.0))
                    .line_height(px(12.0))
                    .text_color(self.theme.colors.destructive.to_paint())
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
            .size(px(24.0))
            .rounded(px(5.0))
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
                .size(px(14.0)),
            );
        if model.available {
            favorite =
                favorite.on_click(cx.listener(move |view: &mut Self, _: &ClickEvent, _, cx| {
                    view.toggle_favorite(favorite_id.clone(), cx);
                    cx.stop_propagation();
                }));
        }
        row = row.child(favorite);
        if selected {
            row = row.child(
                icon(IconStyle::resolve(
                    self.theme,
                    AssetId::TABLER_CHECK,
                    IconSize::Compact,
                    IconTint::Muted,
                ))
                .size(px(14.0))
                .flex_shrink_0(),
            );
        }
        row
    }

    fn render_preview(&self, cx: &Context<Self>) -> Stateful<Div> {
        let Some(model_id) = self.state.previewed_model_id() else {
            return div()
                .id("artisan-native-model-selector-preview")
                .flex_1()
                .min_w(px(0.0))
                .p(px(12.0))
                .text_color(self.theme.colors.muted_foreground.to_paint())
                .child("Select a model to preview its capabilities.");
        };
        let Some(model) = self.state.snapshot().manifest.model(model_id).cloned() else {
            return div()
                .id("artisan-native-model-selector-preview")
                .flex_1()
                .child("Model is not in this catalog.");
        };
        let view = self
            .state
            .snapshot()
            .models_for_engine(&model.harness, "", Some(model_id))
            .into_iter()
            .find(|row| row.id == model.id)
            .unwrap_or_else(|| fallback_model_view(&model));
        let mut preview = div()
            .id("artisan-native-model-selector-preview")
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .overflow_y_scroll()
            .gap(px(8.0))
            .p(px(10.0))
            .rounded(px(10.0))
            .bg(self.theme.colors.muted.to_paint());
        preview = preview.child(
            div()
                .flex()
                .items_start()
                .gap(px(7.0))
                .child(
                    icon(IconStyle::resolve(
                        self.theme,
                        provider_asset(&view.provider_id, &view.engine_id),
                        IconSize::Default,
                        IconTint::Muted,
                    ))
                    .size(px(16.0))
                    .flex_shrink_0(),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(
                            div()
                                .truncate()
                                .font_weight(FontWeight::MEDIUM)
                                .text_size(px(14.0))
                                .child(model.name.clone()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(10.0))
                                .text_color(self.theme.colors.muted_foreground.to_paint())
                                .child(format!("{} · {}", view.lab, model.status)),
                        ),
                ),
        );
        if let Some(reason) = view.unavailable_reason.clone() {
            preview = preview.child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(5.0))
                    .p(px(7.0))
                    .rounded(px(7.0))
                    .bg(self.theme.colors.destructive.with_alpha(0.12).to_paint())
                    .text_size(px(11.0))
                    .text_color(self.theme.colors.destructive.to_paint())
                    .child(
                        icon(IconStyle::resolve(
                            self.theme,
                            AssetId::TABLER_ALERT_TRIANGLE,
                            IconSize::Compact,
                            IconTint::Muted,
                        ))
                        .size(px(14.0))
                        .flex_shrink_0(),
                    )
                    .child(div().flex_1().child(reason)),
            );
        }
        if let Some(description) = model.description.clone() {
            preview = preview.child(
                div()
                    .text_size(px(11.0))
                    .line_height(px(16.0))
                    .text_color(self.theme.colors.foreground.to_paint())
                    .child(description),
            );
        }
        preview = preview.child(self.render_capability_chips(&model));
        if let Some(policy) = self.state.preview_policy() {
            preview = preview.child(self.render_policy_controls(&view, &policy, cx));
        }
        if !view.available {
            preview = preview.child(
                div()
                    .text_size(px(10.0))
                    .text_color(self.theme.colors.muted_foreground.to_paint())
                    .child("Connect and configure this engine before applying options."),
            );
        }
        if let Some(error) = self
            .state
            .status()
            .error
            .clone()
            .or_else(|| self.state.local_error.clone())
        {
            preview = preview.child(
                div()
                    .text_size(px(10.0))
                    .text_color(self.theme.colors.destructive.to_paint())
                    .child(error),
            );
        } else if self.state.status().saving {
            preview = preview.child(
                div()
                    .text_size(px(10.0))
                    .text_color(self.theme.colors.muted_foreground.to_paint())
                    .child("Saving…"),
            );
        }
        preview
    }

    fn render_capability_chips(&self, model: &NativeModelDefinition) -> Div {
        let context = model
            .capabilities
            .context_window
            .as_ref()
            .and_then(|capability| {
                capability
                    .options
                    .iter()
                    .find(|option| option.id == capability.default)
                    .map(|option| option.label.clone())
            })
            .or_else(|| {
                model
                    .capabilities
                    .context_window_tokens
                    .map(format_context_tokens)
            });
        let mut chips = div().flex().flex_row().flex_wrap().gap(px(4.0));
        if let Some(context) = context {
            chips = chips.child(self.capability_chip(format!("Context {context}")));
        }
        chips = chips.child(self.capability_chip(if model.capabilities.image_input {
            "Images".to_owned()
        } else {
            "No images".to_owned()
        }));
        chips = chips.child(self.capability_chip(if model.capabilities.local_tools {
            "Local tools".to_owned()
        } else {
            "No local tools".to_owned()
        }));
        if model.capabilities.mcp {
            chips = chips.child(self.capability_chip("MCP".to_owned()));
        }
        if model.capabilities.web_search {
            chips = chips.child(self.capability_chip("Web".to_owned()));
        }
        chips
    }

    fn capability_chip(&self, label: String) -> Div {
        div()
            .px(px(6.0))
            .py(px(3.0))
            .rounded_full()
            .bg(self.theme.colors.popover.to_paint())
            .text_size(px(10.0))
            .text_color(self.theme.colors.muted_foreground.to_paint())
            .child(label)
    }

    fn render_policy_controls(
        &self,
        model: &NativeModelView,
        policy: &NativeModelPolicy,
        cx: &Context<Self>,
    ) -> Div {
        let mut controls = div().flex().flex_col().gap(px(5.0));
        for (axis, label) in [
            (NativePolicyAxis::Variant, "Variant"),
            (NativePolicyAxis::Thinking, "Reasoning"),
            (NativePolicyAxis::Speed, "Speed"),
            (NativePolicyAxis::ContextWindow, "Context window"),
            (NativePolicyAxis::Permission, "Permissions"),
        ] {
            let options = self.axis_options(axis, model);
            let should_show = match axis {
                NativePolicyAxis::Thinking => {
                    !matches!(
                        &model.capabilities.thinking,
                        NativeThinkingCapability::Unavailable
                    ) && options.len() > 1
                }
                NativePolicyAxis::Speed
                | NativePolicyAxis::ContextWindow
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
                .unwrap_or_else(|| "Unavailable".to_owned());
            controls =
                controls.child(self.render_axis_control(axis, label, value, !model.available, cx));
        }
        controls.child(
            div()
                .text_size(px(9.0))
                .text_color(self.theme.colors.muted_foreground.to_paint())
                .child(format!("Revision {}", policy.catalog_revision)),
        )
    }

    fn render_axis_control(
        &self,
        axis: NativePolicyAxis,
        label: &str,
        value: String,
        disabled: bool,
        cx: &Context<Self>,
    ) -> Div {
        let mut control = div().flex().flex_col().gap(px(3.0));
        let mut trigger = div()
            .id(format!("artisan-native-model-selector-axis-{axis:?}"))
            .on_click(cx.listener(move |view: &mut Self, _: &ClickEvent, _, cx| {
                view.toggle_axis(axis, cx);
            }))
            .flex()
            .items_center()
            .justify_between()
            .h(px(30.0))
            .px(px(8.0))
            .rounded(px(7.0))
            .border_1()
            .border_color(self.theme.colors.border.to_paint())
            .text_size(px(11.0))
            .when(disabled, |trigger| trigger.opacity(0.58))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .text_color(self.theme.colors.muted_foreground.to_paint())
                            .child(label.to_owned()),
                    )
                    .child(div().truncate().child(value)),
            )
            .child(
                icon(IconStyle::resolve(
                    self.theme,
                    AssetId::TABLER_CHEVRON_DOWN,
                    IconSize::Compact,
                    IconTint::Muted,
                ))
                .size(px(14.0)),
            );
        if self.state.is_axis_open(axis) {
            trigger = trigger.bg(self.theme.colors.popover.to_paint());
        }
        control = control.child(trigger);
        if self.state.is_axis_open(axis) {
            let mut options = div()
                .id(format!(
                    "artisan-native-model-selector-axis-options-{axis:?}"
                ))
                .flex()
                .flex_col()
                .max_h(px(170.0))
                .overflow_y_scroll()
                .p(px(3.0))
                .gap(px(2.0))
                .rounded(px(7.0))
                .bg(self.theme.colors.popover.to_paint())
                .border_1()
                .border_color(self.theme.colors.border.to_paint());
            for option in self.axis_options(axis, &self.preview_view()) {
                options = options.child(self.render_axis_option(axis, option, cx));
            }
            control = control.child(options);
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
        let mut row = div()
            .id(format!(
                "artisan-native-model-selector-option-{axis:?}-{}",
                option.id
            ))
            .on_click(
                cx.listener(move |view: &mut Self, _: &ClickEvent, window, cx| {
                    view.choose_axis(axis, option_id.clone(), window, cx);
                }),
            )
            .flex()
            .flex_col()
            .gap(px(1.0))
            .p(px(6.0))
            .rounded(px(6.0))
            .text_size(px(11.0))
            .when(option.selected, |row| {
                row.bg(self.theme.colors.accent.to_paint())
            })
            .when(option.disabled, |row| row.opacity(0.5))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(option.label),
            );
        if let Some(description) = option.description {
            row = row.child(
                div()
                    .text_size(px(9.0))
                    .line_height(px(12.0))
                    .text_color(self.theme.colors.muted_foreground.to_paint())
                    .child(description),
            );
        }
        if let Some(advisory) = option.advisory {
            row = row.child(
                div()
                    .text_size(px(9.0))
                    .line_height(px(12.0))
                    .text_color(self.theme.colors.destructive.to_paint())
                    .child(advisory),
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
                    description: Some(candidate.lab.clone()),
                    advisory: candidate.unavailable_reason.clone(),
                    selected: candidate.id == model.id,
                    disabled: !candidate.available,
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
                        selected: policy
                            .as_ref()
                            .and_then(|policy| policy.reasoning_effort.as_ref())
                            .is_some_and(|value| value.id == option.id),
                        disabled: !model.available,
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
                .map(|option| SelectorOption {
                    id: option.id.clone(),
                    label: option.label.clone(),
                    description: Some(option.description.clone()),
                    advisory: None,
                    selected: policy
                        .as_ref()
                        .and_then(|policy| policy.speed.as_ref())
                        .is_some_and(|value| value.id == option.id),
                    disabled: !model.available || option.disabled.is_some(),
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
                            selected: policy
                                .as_ref()
                                .and_then(|policy| policy.context_window.as_ref())
                                .is_some_and(|value| value.id == option.id),
                            disabled: !model.available,
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
                            description: Some(option.description.clone()),
                            advisory: None,
                            selected: policy
                                .as_ref()
                                .and_then(|policy| policy.permission.as_ref())
                                .is_some_and(|value| value.id == option.id),
                            disabled: !model.available,
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
        let scroll = self.menu_scroll.clone();
        let probe = canvas(
            move |_, _, _| {},
            move |bounds, (), window, cx| {
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
            .on_mouse_down_out(cx.listener(Self::handle_outside_press))
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

#[derive(Clone, Debug)]
struct SelectorOption {
    id: String,
    label: String,
    description: Option<String>,
    advisory: Option<String>,
    selected: bool,
    disabled: bool,
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
        "backspace" if !modified => Some(NativeModelSelectorKey::Backspace),
        _ if !modified && key.chars().count() == 1 => {
            key.chars().next().map(NativeModelSelectorKey::Character)
        }
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
        unavailable_reason: Some("Model availability is not known.".to_owned()),
        source_index: 0,
    }
}

fn fallback_model_view_from_state(state: &NativeModelSelectorState) -> NativeModelView {
    state.snapshot().manifest.models.first().map_or_else(
        || NativeModelView {
            id: String::new(),
            name: "Select model".to_owned(),
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
            unavailable_reason: Some("No model is in this catalog.".to_owned()),
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

    fn state_with_codex_runtime() -> NativeModelSelectorState {
        let mut catalog =
            NativeModelCatalog::offline().expect("the real bundled catalog must decode");
        catalog.runnable_harness_ids.push("codex".to_owned());
        NativeModelSelectorState::new(catalog, None)
    }

    #[test]
    fn keyboard_commit_selects_policy_and_preserves_native_values() {
        let mut state = state_with_codex_runtime();
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
        let mut state = state_with_codex_runtime();
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
        let mut state = state_with_codex_runtime();
        let event = state
            .toggle_favorite("codex-sol")
            .expect("available model emits favorite intent");
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
    fn unavailable_model_can_be_previewed_but_not_committed() {
        let mut state = NativeModelSelectorState::new(
            NativeModelCatalog::offline().expect("real catalog"),
            None,
        );
        state.press_trigger();
        state.preview_model("codex-sol");
        assert_eq!(state.previewed_model_id(), Some("codex-sol"));
        assert!(state.select_model("codex-sol").is_none());
    }
}
