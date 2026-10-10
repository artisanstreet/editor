//! Pure picker presentation of model selections.
//!
//! The Forge builds and validates engine configurations (a selection is
//! resolved by the Forge, and a send carries the selection for the Forge to
//! admit). The Editor only translates between what the picker shows and
//! catalog identities:
//!
//! - [`selection_for_policy`] names a displayed policy by catalog identities
//!   for the Forge, and
//! - [`saved_config_policy`] finds the catalog row and options that display a
//!   saved configuration, by matching its model and option values. It never
//!   builds a configuration: a saved configuration the catalog cannot display
//!   simply has no policy, and
//! - [`UserModelPreferences`] lays the Forge's remembered choices over a
//!   catalog: the model the user last chose becomes the catalog's default
//!   model, and each model starts from the options the user last chose it
//!   with.

#![forbid(unsafe_code)]

use artisan_catalog::{
    NativeContextSelection, NativeModelCatalog, NativeModelDefaults, NativeModelDefinition,
    NativeModelPolicy, NativeOptionValue,
};
use artisan_domain::{
    CatalogOptionId, CatalogSelection, EngineProfileId, EngineRunConfig, EngineSelection,
    ModelFavoriteId, ModelPreference, UserPreferences,
};

/// Profile the Forge runs native engines under when a selection names none;
/// the Editor reads an unconfigured thread's catalog in this scope.
pub(crate) const NATIVE_DEFAULT_PROFILE_ID: &str = "default";

/// Names a displayed policy by catalog identities; `None` when an identity
/// cannot cross the boundary (the Forge would refuse it anyway).
#[must_use]
pub(crate) fn selection_for_policy(policy: &NativeModelPolicy) -> Option<CatalogSelection> {
    let option = |id: Option<&str>| id.map(CatalogOptionId::parse).transpose().ok();
    Some(CatalogSelection {
        model_id: ModelFavoriteId::parse(policy.model_id.clone()).ok()?,
        profile_id: policy
            .profile_id
            .clone()
            .map(EngineProfileId::parse)
            .transpose()
            .ok()?,
        reasoning_effort: option(
            policy
                .reasoning_effort
                .as_ref()
                .map(|value| value.id.as_str()),
        )?,
        speed: option(policy.speed.as_ref().map(|value| value.id.as_str()))?,
        context_window: option(
            policy
                .context_window
                .as_ref()
                .map(|value| value.id.as_str()),
        )?,
        permission: option(policy.permission.as_ref().map(|value| value.id.as_str()))?,
    })
}

/// The model choices the Forge remembers for its user.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct UserModelPreferences {
    /// The model the user last chose, with its options.
    last_used: Option<EngineRunConfig>,
    /// The options the user last chose each model with.
    models: Vec<ModelPreference>,
}

impl UserModelPreferences {
    /// The model choices of the Forge's preferences.
    #[must_use]
    pub fn of(preferences: &UserPreferences) -> Self {
        Self {
            last_used: preferences.default_engine_config.clone(),
            models: preferences.model_preferences.clone(),
        }
    }

    /// Lays the remembered choices over `catalog`: its default model is the
    /// row displaying the last-used configuration, and each remembered model
    /// defaults to the options it was last chosen with (the last-used model
    /// to the last-used options). A choice the catalog cannot display is
    /// left out.
    pub fn apply_to(&self, catalog: &mut NativeModelCatalog) {
        let last_used = self
            .last_used
            .as_ref()
            .and_then(|config| saved_config_policy(catalog, config));
        let remembered = self.models.iter().filter_map(|preference| {
            saved_config_policy(catalog, &preference.config)
                .filter(|policy| policy.model_id == preference.model_id.as_str())
        });
        let mut model_defaults: Vec<NativeModelDefaults> = Vec::new();
        for policy in last_used.iter().cloned().chain(remembered) {
            if model_defaults
                .iter()
                .any(|defaults| defaults.model_id == policy.model_id)
            {
                continue;
            }
            model_defaults.push(NativeModelDefaults {
                model_id: policy.model_id,
                reasoning_effort: policy.reasoning_effort,
                speed: policy.speed,
                context_window: policy.context_window,
                permission: policy.permission,
            });
        }
        catalog.model_defaults = model_defaults;
        catalog.default_model_id = last_used.map(|policy| policy.model_id);
    }
}

/// Finds the policy that displays a saved configuration in `catalog`.
///
/// The model row is found by its native identity (the Claude context suffix
/// split back off the saved model id, the `OpenCode` route and variant) and
/// every option by the value the configuration carries; the permission by
/// the option id the configuration records. The first row whose options all
/// exist wins; none means the catalog cannot display the configuration.
#[must_use]
pub(crate) fn saved_config_policy(
    catalog: &NativeModelCatalog,
    saved: &EngineRunConfig,
) -> Option<NativeModelPolicy> {
    let selection = saved.selection();
    let profile = selection.profile_id().as_str().to_owned();
    let permission_id = selection.permission().permission_id().as_str().to_owned();
    let candidates = catalog
        .manifest
        .models
        .iter()
        .filter(|model| model.harness == selection.engine_id().as_str());
    for model in candidates {
        let Ok(mut policy) = catalog.preview_policy_for_model(&model.id) else {
            continue;
        };
        if !fit_selection(model, &mut policy, selection) {
            continue;
        }
        let Some(option) = catalog
            .manifest
            .harness(&model.harness)
            .and_then(|harness| {
                harness
                    .permissions
                    .options
                    .iter()
                    .find(|option| option.id == permission_id)
            })
        else {
            continue;
        };
        policy.permission = Some(NativeOptionValue {
            id: option.id.clone(),
            native_value: option.native_value.clone(),
        });
        // `OpenCode` profiles are scoped by the catalog; native selections
        // keep the profile they were saved under.
        if !matches!(selection, EngineSelection::OpenCode2(_)) {
            policy.profile_id = Some(profile.clone());
        } else if catalog.scope.as_ref()?.profile_id != profile {
            return None;
        }
        return Some(policy);
    }
    None
}

/// Whether `model` displays the saved `selection`, setting the options it
/// carries on `policy`.
fn fit_selection(
    model: &NativeModelDefinition,
    policy: &mut NativeModelPolicy,
    selection: &EngineSelection,
) -> bool {
    use artisan_domain::EngineModelId;
    match selection {
        EngineSelection::Codex(codex) => {
            let window = codex
                .model_context_window()
                .map(artisan_domain::CodexModelContextWindow::get);
            fit_direct(model, codex.model_id().map(EngineModelId::as_str))
                && fit_thinking(
                    model,
                    policy,
                    codex
                        .reasoning_effort()
                        .map(artisan_domain::CodexReasoningEffort::as_str),
                )
                && fit_speed(
                    model,
                    policy,
                    codex
                        .service_tier()
                        .map(artisan_domain::CodexServiceTier::as_str),
                )
                && fit_window(model, policy, window)
        }
        EngineSelection::Claude(claude) => {
            let window = claude
                .auto_compact_window()
                .map(artisan_domain::ClaudeAutoCompactWindow::get);
            let Some(context) = claude
                .model_id()
                .and_then(|id| claude_context_for_model(model, id.as_str(), window))
            else {
                return false;
            };
            policy.context_window = context;
            policy.speed = None;
            fit_thinking(
                model,
                policy,
                claude.effort().map(artisan_domain::ClaudeEffort::as_str),
            )
        }
        EngineSelection::Grok(grok) => {
            fit_direct(model, grok.model_id().map(EngineModelId::as_str))
                && fit_thinking(
                    model,
                    policy,
                    grok.reasoning_effort()
                        .map(artisan_domain::GrokReasoningEffort::as_str),
                )
        }
        EngineSelection::Cursor(cursor) => {
            fit_direct(model, cursor.model_id().map(EngineModelId::as_str))
                && fit_thinking(
                    model,
                    policy,
                    cursor
                        .reasoning_effort()
                        .map(artisan_domain::CursorReasoningEffort::as_str),
                )
                && fit_speed(
                    model,
                    policy,
                    cursor.speed().map(artisan_domain::CursorSpeed::as_str),
                )
        }
        EngineSelection::OpenCode2(opencode) => {
            model.native_selection.as_ref().is_some_and(|native| {
                native.model_id == opencode.model_id().as_str()
                    && native.provider_route_id == opencode.route_id().as_str()
                    && native.variant_id.as_deref()
                        == opencode
                            .variant_id()
                            .map(artisan_domain::EngineVariantId::as_str)
            })
        }
    }
}

/// A direct (non-routed) model row names the saved model.
fn fit_direct(model: &NativeModelDefinition, wanted: Option<&str>) -> bool {
    wanted.is_some_and(|wanted| model.native_model_id == wanted)
}

/// Sets the reasoning option carrying `native`, or none when unsaved.
fn fit_thinking(
    model: &NativeModelDefinition,
    policy: &mut NativeModelPolicy,
    native: Option<&str>,
) -> bool {
    let Some(native) = native else {
        policy.reasoning_effort = None;
        return true;
    };
    let artisan_catalog::NativeThinkingCapability::Supported { options, .. } =
        &model.capabilities.thinking
    else {
        return false;
    };
    policy.reasoning_effort = options
        .iter()
        .find(|option| option.native_value == native)
        .map(|option| NativeOptionValue {
            id: option.id.clone(),
            native_value: option.native_value.clone(),
        });
    policy.reasoning_effort.is_some()
}

/// Sets the enabled speed option carrying `native`, or none when unsaved.
fn fit_speed(
    model: &NativeModelDefinition,
    policy: &mut NativeModelPolicy,
    native: Option<&str>,
) -> bool {
    let Some(native) = native else {
        policy.speed = None;
        return true;
    };
    policy.speed = model
        .capabilities
        .speed_options
        .iter()
        .find(|option| option.native_value == native && option.disabled.is_none())
        .map(|option| NativeOptionValue {
            id: option.id.clone(),
            native_value: option.native_value.clone(),
        });
    policy.speed.is_some()
}

/// Sets the Codex window option carrying `window`; an unsaved window is the
/// provider's base window even when the catalog recommends another.
fn fit_window(
    model: &NativeModelDefinition,
    policy: &mut NativeModelPolicy,
    window: Option<u64>,
) -> bool {
    let options = model
        .capabilities
        .context_window
        .as_ref()
        .map_or(&[][..], |capability| capability.options.as_slice());
    let found = options.iter().find(|option| match window {
        Some(window) => option
            .native_config
            .as_ref()
            .is_some_and(|config| config.model_context_window == window),
        None => option.native_config.is_none() && option.native_suffix.is_empty(),
    });
    policy.context_window = found.map(|option| NativeContextSelection {
        id: option.id.clone(),
        native_suffix: option.native_suffix.clone(),
        native_config: option.native_config.clone(),
    });
    window.is_none() || policy.context_window.is_some()
}

/// Splits a saved Claude model id and auto-compact window into this row's
/// context option.
///
/// The durable Claude model composes the base id with its context suffix, so
/// an exact base match is the empty-suffix option (or none), and anything
/// else must end in a real option suffix. A saved window selects the option
/// whose configuration carries it; without one only config-less options
/// match. The outer `None` means the saved selection does not belong to this
/// row.
#[expect(
    clippy::option_option,
    reason = "the outer None means the saved id did not match this manifest row; the inner None is the config-less option choice, and both are load-bearing"
)]
fn claude_context_for_model(
    model: &NativeModelDefinition,
    wanted: &str,
    window: Option<u64>,
) -> Option<Option<NativeContextSelection>> {
    // A row without context options (its limits went unreported) displays
    // only its own base id.
    let Some(capability) = model.capabilities.context_window.as_ref() else {
        return (wanted == model.native_model_id && window.is_none()).then_some(None);
    };
    let config_matches = |option: &artisan_catalog::NativeContextWindowOption| {
        option
            .native_config
            .as_ref()
            .map(|config| config.model_context_window)
            == window
    };
    let selection = |option: &artisan_catalog::NativeContextWindowOption| NativeContextSelection {
        id: option.id.clone(),
        native_suffix: option.native_suffix.clone(),
        native_config: option.native_config.clone(),
    };
    if wanted == model.native_model_id {
        let found = capability
            .options
            .iter()
            .find(|option| option.native_suffix.is_empty() && config_matches(option))
            .map(selection);
        // A saved window this row cannot carry means another row saved it.
        return (found.is_some() || window.is_none()).then_some(found);
    }
    let suffix = wanted.strip_prefix(model.native_model_id.as_str())?;
    if suffix.is_empty() {
        return None;
    }
    capability
        .options
        .iter()
        .find(|option| option.native_suffix == suffix && config_matches(option))
        .map(|option| Some(selection(option)))
}

#[cfg(test)]
#[path = "picker_selection/tests.rs"]
mod tests;
