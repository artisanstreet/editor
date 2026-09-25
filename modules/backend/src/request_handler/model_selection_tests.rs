use artisan_domain::{CatalogOptionId, EngineReadinessVerdict, ModelFavoriteId};

use super::*;

/// The fixture catalog as the Forge serves it with `runnable` engines ready.
fn catalog(runnable: &[&str]) -> NativeModelCatalog {
    let mut catalog = NativeModelCatalog::from_manifest_json(include_str!(
        "../../../../tests/fixtures/model_catalog.json"
    ))
    .expect("fixture catalog");
    catalog.runnable_harness_ids = runnable.iter().map(|engine| (*engine).to_owned()).collect();
    catalog
}

fn selection(model_id: &str) -> CatalogSelection {
    let shown = catalog(&[]).selection_policy_for_model(model_id).unwrap();
    let option = |value: Option<&str>| value.map(|id| CatalogOptionId::parse(id).unwrap());
    CatalogSelection {
        model_id: ModelFavoriteId::parse(model_id).unwrap(),
        profile_id: None,
        reasoning_effort: option(
            shown
                .reasoning_effort
                .as_ref()
                .map(|value| value.id.as_str()),
        ),
        speed: option(shown.speed.as_ref().map(|value| value.id.as_str())),
        context_window: option(shown.context_window.as_ref().map(|value| value.id.as_str())),
        permission: option(shown.permission.as_ref().map(|value| value.id.as_str())),
    }
}

fn resolved(runnable: &[&str], model_id: &str) -> ResolvedSelection {
    resolve_in_catalog(catalog(runnable), &selection(model_id), None)
        .unwrap_or_else(|refusal| panic!("{model_id} resolves: {}", refusal.message()))
}

fn signed_out(engine: &str) -> Option<EngineReadiness> {
    Some(
        EngineReadiness::new(
            EngineReadinessVerdict::NeedsSignIn,
            Some(format!("{engine} account sign-in is required.")),
        )
        .unwrap(),
    )
}

fn refused_kind(plan: &SubmissionPlan) -> Option<SubmissionRefusalKind> {
    match plan {
        SubmissionPlan::Refuse(refusal) => Some(refusal.kind()),
        SubmissionPlan::Admit { .. } => None,
    }
}

#[test]
fn a_first_send_saves_the_resolved_configuration_when_its_engine_can_run() {
    let resolved = resolved(&["codex"], "codex-sol");
    let config = resolved.config.clone();
    assert_eq!(
        plan_submission(None, None, Some(Ok(resolved)), |_| None),
        SubmissionPlan::Admit {
            save: Some(config),
            engine: EngineId::Codex,
        }
    );
}

#[test]
fn a_first_send_on_an_engine_that_cannot_run_is_refused_with_its_reason() {
    let plan = plan_submission(None, None, Some(Ok(resolved(&[], "codex-sol"))), |engine| {
        signed_out(if engine == "codex" { "Codex" } else { engine })
    });
    let SubmissionPlan::Refuse(refusal) = plan else {
        panic!("an unrunnable first send is refused");
    };
    assert_eq!(refusal.kind(), SubmissionRefusalKind::EngineNotReady);
    assert!(
        refusal
            .message()
            .starts_with("Codex account sign-in is required. Your draft is preserved"),
        "{}",
        refusal.message()
    );
    // Without a readiness reason the catalog's own unavailability is named.
    let plan = plan_submission(None, None, Some(Ok(resolved(&[], "codex-sol"))), |_| None);
    let SubmissionPlan::Refuse(refusal) = plan else {
        panic!("refused");
    };
    assert_eq!(refusal.message(), CATALOG_REJECTED);
}

#[test]
fn the_saved_configuration_is_admitted_even_while_its_engine_cannot_run() {
    // The Forge holds the message until the engine can run it, and says why
    // on the outbox row; it does not refuse the send.
    let saved = resolved(&[], "codex-sol").config;
    assert_eq!(
        plan_submission(
            None,
            Some(&saved),
            Some(Ok(resolved(&[], "codex-sol"))),
            |_| None
        ),
        SubmissionPlan::Admit {
            save: None,
            engine: EngineId::Codex,
        }
    );
    assert_eq!(
        plan_submission(None, Some(&saved), None, |_| None),
        SubmissionPlan::Admit {
            save: None,
            engine: EngineId::Codex,
        }
    );
}

#[test]
fn a_selection_that_changes_the_configuration_is_saved_first() {
    let saved = resolved(&["codex", "claude"], "codex-sol").config;
    let next = resolved(&["codex", "claude"], "claude-fable");
    let config = next.config.clone();
    assert_eq!(
        plan_submission(
            Some(RunLiveStatus::Running),
            Some(&saved),
            Some(Ok(next)),
            |_| None
        ),
        SubmissionPlan::Admit {
            save: Some(config),
            engine: EngineId::Claude,
        }
    );
}

#[test]
fn unconfigured_without_selection_starting_runs_and_invalid_selections_are_refused() {
    assert_eq!(
        refused_kind(&plan_submission(None, None, None, |_| None)),
        Some(SubmissionRefusalKind::NoSelection)
    );
    let saved = resolved(&["codex"], "codex-sol").config;
    assert_eq!(
        refused_kind(&plan_submission(
            Some(RunLiveStatus::Queued),
            Some(&saved),
            None,
            |_| None
        )),
        Some(SubmissionRefusalKind::RunStarting)
    );
    let mut stale = selection("codex-sol");
    stale.permission = Some(CatalogOptionId::parse("retired-permission").unwrap());
    let invalid = resolve_in_catalog(catalog(&["codex"]), &stale, None);
    let Err(refusal) = &invalid else {
        panic!("a retired option is refused");
    };
    assert_eq!(refusal.kind(), SubmissionRefusalKind::InvalidSelection);
    assert_eq!(
        refused_kind(&plan_submission(None, Some(&saved), Some(invalid), |_| {
            None
        })),
        Some(SubmissionRefusalKind::InvalidSelection)
    );
}

#[test]
fn a_send_steers_only_a_live_run_on_its_engine() {
    let run = RunId::parse("run-live").unwrap();
    let live = |status| Some((run.clone(), status, EngineId::Codex));
    assert_eq!(
        steer_target(live(RunLiveStatus::Running), EngineId::Codex),
        Some(run.clone())
    );
    assert_eq!(
        steer_target(live(RunLiveStatus::Waiting), EngineId::Codex),
        Some(run.clone())
    );
    assert_eq!(
        steer_target(live(RunLiveStatus::Running), EngineId::Claude),
        None
    );
    assert_eq!(
        steer_target(live(RunLiveStatus::Queued), EngineId::Codex),
        None
    );
    assert_eq!(steer_target(None, EngineId::Codex), None);
}
