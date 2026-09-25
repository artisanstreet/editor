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
//!   simply has no policy.

#![forbid(unsafe_code)]

use artisan_catalog::{
    NativeContextSelection, NativeModelCatalog, NativeModelDefinition, NativeModelPolicy,
    NativeOptionValue,
};
use artisan_domain::{
    CatalogOptionId, CatalogSelection, EngineProfileId, EngineRunConfig, EngineSelection,
    ModelFavoriteId,
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
            let Some(context) = claude
                .model_id()
                .and_then(|id| claude_context_for_model(model, id.as_str()))
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

/// Splits a saved Claude model id into this row's context option.
///
/// The durable Claude model composes the base id with its context suffix, so
/// an exact base match is the config-less empty-suffix option (or none), and
/// anything else must end in a real config-less option suffix. The outer
/// `None` means the saved id does not belong to this row.
#[expect(
    clippy::option_option,
    reason = "the outer None means the saved id did not match this manifest row; the inner None is the config-less option choice, and both are load-bearing"
)]
fn claude_context_for_model(
    model: &NativeModelDefinition,
    wanted: &str,
) -> Option<Option<NativeContextSelection>> {
    let capability = model.capabilities.context_window.as_ref()?;
    let selection = |option: &artisan_catalog::NativeContextWindowOption| NativeContextSelection {
        id: option.id.clone(),
        native_suffix: option.native_suffix.clone(),
        native_config: option.native_config.clone(),
    };
    if wanted == model.native_model_id {
        return Some(
            capability
                .options
                .iter()
                .find(|option| option.native_suffix.is_empty() && option.native_config.is_none())
                .map(selection),
        );
    }
    let suffix = wanted.strip_prefix(model.native_model_id.as_str())?;
    if suffix.is_empty() {
        return None;
    }
    capability
        .options
        .iter()
        .find(|option| option.native_suffix == suffix && option.native_config.is_none())
        .map(|option| Some(selection(option)))
}

#[cfg(test)]
#[path = "picker_selection/tests.rs"]
mod tests;
