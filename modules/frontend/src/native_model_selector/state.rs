//! Pure model-selector state, label composition, and policy reconciliation.
//!
//! Extracted verbatim from `native_model_selector.rs` during the module split;
//! visibility was widened to `pub(super)` for helpers read by sibling modules.

use super::*;

impl NativeModelLabelToken {
    fn name(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            role: NativeModelLabelRole::Name,
        }
    }

    fn detail(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            role: NativeModelLabelRole::Detail,
        }
    }
}

impl NativeModelLabel {
    /// Builds a name-only label; used when no model resolves.
    #[must_use]
    pub fn name(text: impl Into<String>) -> Self {
        Self {
            tokens: vec![NativeModelLabelToken::name(text)],
        }
    }

    /// Returns the ordered tokens.
    #[must_use]
    pub fn tokens(&self) -> &[NativeModelLabelToken] {
        &self.tokens
    }

    /// Returns the plain space-separated label text.
    #[must_use]
    pub fn plain_text(&self) -> String {
        self.tokens
            .iter()
            .map(|token| token.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Composes the full model label in reference order:
/// `<name> <context> <effort> <variant> <speed>`.
///
/// Only existing tokens are included: the active context-window label appears
/// for a model that declares a configurable window (never for a model with
/// only a fixed token count), the effort label uses the shared thinking
/// vocabulary, a non-default routed variant adds its label, and the speed
/// token appears only when the selection is not the model's own default.
/// Tokens are separated by single spaces; no bullet or middot separator is
/// emitted anywhere.
#[must_use]
pub fn model_display_label(
    catalog: &NativeModelCatalog,
    policy: &NativeModelPolicy,
) -> NativeModelLabel {
    let Some(model) = catalog.manifest.model(&policy.model_id) else {
        return NativeModelLabel::name(policy.model_id.clone());
    };
    let mut tokens = vec![NativeModelLabelToken::name(model.name.clone())];
    if let Some(context) = context_window_label(model, policy) {
        tokens.push(NativeModelLabelToken::detail(context));
    }
    if let Some(effort) = effort_label(model, policy) {
        tokens.push(NativeModelLabelToken::detail(effort));
    }
    if let Some(variant) = variant_label(policy) {
        tokens.push(NativeModelLabelToken::detail(variant));
    }
    if let Some(speed) = speed_label_token(model, policy) {
        tokens.push(speed);
    }
    NativeModelLabel { tokens }
}

/// Resolves the active context-window label for a configurable window.
///
/// The policy's exact option id wins; a missing or stale id falls back to the
/// capability default and then the first option. The catalog's display label
/// is used below one million tokens. Larger windows use the token count so
/// a catalog label such as `1000K` reads `1M`.
fn context_window_label(
    model: &NativeModelDefinition,
    policy: &NativeModelPolicy,
) -> Option<String> {
    let capability = model.capabilities.context_window.as_ref()?;
    if capability.options.is_empty() {
        return None;
    }
    let selected = policy
        .context_window
        .as_ref()
        .and_then(|selection| {
            capability
                .options
                .iter()
                .find(|option| option.id == selection.id)
        })
        .or_else(|| {
            capability
                .options
                .iter()
                .find(|option| option.id == capability.default)
        })
        .or_else(|| capability.options.first())?;
    Some(
        if selected.tokens >= 1_000_000 || selected.label.trim().is_empty() {
            format_context_tokens(selected.tokens)
        } else {
            selected.label.clone()
        },
    )
}

/// Resolves the reasoning-effort label using the shared thinking vocabulary.
fn effort_label(model: &NativeModelDefinition, policy: &NativeModelPolicy) -> Option<String> {
    let effort = policy.reasoning_effort.as_ref()?;
    if !matches!(
        &model.capabilities.thinking,
        NativeThinkingCapability::Supported { .. }
    ) {
        return None;
    }
    Some(thinking_level_label(&effort.id))
}

/// Resolves a routed variant label; `default` adds no token.
fn variant_label(policy: &NativeModelPolicy) -> Option<String> {
    let variant = policy.native_selection.as_ref()?.variant_id.as_deref()?;
    (variant != "default").then(|| humanize_variant(variant))
}

/// Resolves the accelerated-speed token, hiding the model's own default.
fn speed_label_token(
    model: &NativeModelDefinition,
    policy: &NativeModelPolicy,
) -> Option<NativeModelLabelToken> {
    let value = policy.speed.as_ref()?;
    let option = model
        .capabilities
        .speed_options
        .iter()
        .find(|option| option.id == value.id)?;
    if option.default {
        return None;
    }
    let presentation = crate::speed_presentation::speed_option_presentation(
        &crate::speed_presentation::SpeedOption::new(
            option.id.clone(),
            option.label.clone(),
            option.native_value.clone(),
            option.description.clone(),
            option.default,
            option.disabled.map(|_| String::new()),
        ),
    );
    let role = crate::speed_presentation::speed_label_gradient(&option.id)
        .map_or(NativeModelLabelRole::Detail, NativeModelLabelRole::Gradient);
    Some(NativeModelLabelToken {
        text: presentation.label,
        role,
    })
}

/// Maps a policy effort id onto the reference thinking vocabulary.
fn thinking_level_label(value: &str) -> String {
    match value {
        "xhigh" => "Extra High".to_owned(),
        "high" => "High".to_owned(),
        "medium" => "Medium".to_owned(),
        "light" | "low" => "Light".to_owned(),
        "minimal" => "Minimal".to_owned(),
        "max" => "Max".to_owned(),
        "ultra" => "Ultra".to_owned(),
        other => humanize_variant(other),
    }
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
            .filter(|engine| {
                snapshot
                    .manifest
                    .harness(engine)
                    .is_some_and(|harness| !harness.hidden)
            })
            .or_else(|| {
                snapshot
                    .manifest
                    .harnesses
                    .iter()
                    .find(|harness| !harness.hidden)
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
            model_groups_cache: RefCell::new(None),
            collapsed_groups: std::collections::HashSet::new(),
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
        let active_engine = if snapshot
            .manifest
            .harness(&self.active_engine)
            .is_some_and(|harness| !harness.hidden)
        {
            self.active_engine.clone()
        } else {
            policy
                .as_ref()
                .map(|policy| policy.engine_id.clone())
                .filter(|engine| {
                    snapshot
                        .manifest
                        .harness(engine)
                        .is_some_and(|harness| !harness.hidden)
                })
                .or_else(|| {
                    snapshot
                        .manifest
                        .harnesses
                        .iter()
                        .find(|harness| !harness.hidden)
                        .map(|harness| harness.id.clone())
                })
                .unwrap_or_default()
        };
        self.model_groups_cache.get_mut().take();
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

    /// Returns the full-format trigger label as ordered tokens.
    ///
    /// The label never invents a model name: with no selected model it reads
    /// `No model`. Every existing policy axis contributes at most one token,
    /// and accelerated speed tiers keep their gradient role for painting.
    #[must_use]
    pub fn trigger_label(&self) -> NativeModelLabel {
        self.policy.as_ref().map_or_else(
            || NativeModelLabel::name("No model"),
            |policy| model_display_label(&self.snapshot, policy),
        )
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
    pub fn model_groups(&self) -> Rc<[crate::native_model_catalog::NativeModelGroupView]> {
        let selected_model_id = self.selected_model_id();
        let mut cache = self.model_groups_cache.borrow_mut();
        if let Some(cached) = cache.as_ref()
            && cached.engine == self.active_engine
            && cached.query == self.query
            && cached.selected_model_id.as_deref() == selected_model_id
        {
            return Rc::clone(&cached.groups);
        }
        let mut groups = self.snapshot.route_groups_for_engine(
            &self.active_engine,
            &self.query,
            selected_model_id,
        );
        for group in &mut groups {
            let mut families: Vec<NativeModelView> = Vec::new();
            for model in std::mem::take(&mut group.models) {
                if let Some(existing) = families
                    .iter_mut()
                    .find(|existing| same_model_family(existing, &model))
                {
                    if model.selected || (!existing.selected && model.variant_label.is_none()) {
                        *existing = model;
                    }
                } else {
                    families.push(model);
                }
            }
            group.models = families;
        }
        let groups: Rc<[_]> = groups.into();
        *cache = Some(ModelGroupsCache {
            engine: self.active_engine.clone(),
            query: self.query.clone(),
            selected_model_id: selected_model_id.map(str::to_owned),
            groups: Rc::clone(&groups),
        });
        groups
    }

    pub(super) fn group_collapsed(&self, id: &str) -> bool {
        self.query.is_empty()
            && self
                .collapsed_groups
                .contains(&(self.active_engine.clone(), id.to_owned()))
    }

    pub(super) fn toggle_group(&mut self, id: &str) {
        let key = (self.active_engine.clone(), id.to_owned());
        if !self.collapsed_groups.remove(&key) {
            self.collapsed_groups.insert(key);
        }
        self.refresh_preview_and_highlight();
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
    ///
    /// # Errors
    ///
    /// Returns [`NativePolicyValidationError`] when the previewed model is
    /// unknown or the option is absent/disabled for its capability set.
    #[expect(
        clippy::too_many_lines,
        reason = "one validation pass per policy axis keeps the option-to-native-value mapping reviewable in one place"
    )]
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
        self.model_groups()
            .iter()
            .filter(|group| !self.group_collapsed(&group.id))
            .flat_map(|group| group.models.iter().cloned())
            .collect()
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

    pub(super) fn set_local_error(&mut self, error: Option<String>) {
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

    pub(super) fn model_definition_disabled(&self, model_id: &str) -> bool {
        self.snapshot
            .manifest
            .model(model_id)
            .is_some_and(|model| model.disabled.is_some())
    }

    pub(super) fn model_definition_disabled_reason(&self, model_id: &str) -> Option<String> {
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

pub(super) fn rebase_selection_policy(
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
        candidate
            .reasoning_effort
            .clone_from(&policy.reasoning_effort);
        if snapshot.validate_selection_policy(&candidate).is_ok() {
            rebased.reasoning_effort = candidate.reasoning_effort;
        }
    }
    if policy.speed.is_some() {
        let mut candidate = rebased.clone();
        candidate.speed.clone_from(&policy.speed);
        if snapshot.validate_selection_policy(&candidate).is_ok() {
            rebased.speed = candidate.speed;
        }
    }
    if policy.context_window.is_some() {
        let mut candidate = rebased.clone();
        candidate.context_window = snapshot
            .rebase_policy(policy)
            .and_then(|policy| policy.context_window);
        if snapshot.validate_selection_policy(&candidate).is_ok() {
            rebased.context_window = candidate.context_window;
        }
    }
    if policy.permission.is_some() {
        let mut candidate = rebased.clone();
        candidate.permission.clone_from(&policy.permission);
        if snapshot.validate_selection_policy(&candidate).is_ok() {
            rebased.permission = candidate.permission;
        }
    }
    // An explicit native profile is owner-authoritative durability (the
    // saved thread configuration), not scope state: keep it so a reloaded
    // thread displays its saved profile. Managed `OpenCode` policies stay
    // scope-fenced, as does any policy without an explicit profile.
    if policy.engine_id != "opencode2" && policy.profile_id.is_some() {
        rebased.profile_id.clone_from(&policy.profile_id);
    }
    Some(rebased)
}

fn format_context_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        let millions = tokens / 1_000_000 + u64::from(tokens % 1_000_000 >= 500_000);
        format!("{millions}M")
    } else if tokens >= 1_000 {
        let thousands = tokens / 1_000 + u64::from(tokens % 1_000 >= 500);
        if thousands == 1_000 {
            "1M".to_owned()
        } else {
            format!("{thousands}K")
        }
    } else {
        tokens.to_string()
    }
}

pub(super) fn humanize_variant(value: &str) -> String {
    match value {
        "xhigh" => "Extra High".to_owned(),
        "minimal" => "Minimal".to_owned(),
        "medium" => "Medium".to_owned(),
        "high" => "High".to_owned(),
        "light" | "low" => "Light".to_owned(),
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

pub(super) fn same_model_family(left: &NativeModelView, right: &NativeModelView) -> bool {
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

pub(super) fn fallback_model_view(model: &NativeModelDefinition) -> NativeModelView {
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

pub(super) fn fallback_model_view_from_state(state: &NativeModelSelectorState) -> NativeModelView {
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
