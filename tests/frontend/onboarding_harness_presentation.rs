//! Exhaustive direct tests for the dependency-free onboarding presentation
//! and completion-controller boundary.

#![forbid(unsafe_code)]
#![allow(clippy::float_cmp)]

#[path = "../../modules/frontend/src/onboarding_harness_presentation.rs"]
mod onboarding_harness_presentation;

use onboarding_harness_presentation::{
    CONTINUE_FOOTER_LABEL, CompletionAction, CompletionOutcome, CompletionSaveToken,
    CompletionTransition, HERMES_SETUP_URL, HarnessCard, HarnessCatalog, HarnessRefreshAction,
    HarnessSetupAction, HarnessSetupIcon, HarnessSetupState, ONBOARDING_COMPLETION_FAILURE_MESSAGE,
    ONBOARDING_COMPLETION_ROUTE, OnboardingCompletionController, SAVING_FOOTER_LABEL,
    forced_refresh_actions, forced_refresh_actions_for, harness_catalog, present_harness_card,
    setup_icon_for, setup_is_actionable,
};

fn setup(
    action: HarnessSetupAction,
    ready: bool,
    busy: bool,
    label: &str,
    email: Option<&str>,
    failure: Option<&str>,
) -> HarnessSetupState {
    HarnessSetupState::new(
        action,
        ready,
        busy,
        label,
        email.map(str::to_owned),
        failure.map(str::to_owned),
    )
}

fn save_token(transition: &CompletionTransition) -> CompletionSaveToken {
    match transition.actions().first() {
        Some(CompletionAction::SaveDefaults { token, .. }) => *token,
        other => panic!("expected one save action, got {other:?}"),
    }
}

#[test]
fn catalog_has_the_exact_legacy_order_and_every_exact_field() {
    let catalog = harness_catalog();
    assert_eq!(catalog, HarnessCatalog::new());
    assert_eq!(catalog.len(), 6);
    assert!(!catalog.is_empty());

    let expected = [
        (
            "codex",
            "Codex",
            "OpenAI's terminal coding agent for GPT and Codex models.",
            "#000000",
            false,
            0.0,
            -0.4,
            0.1,
            false,
        ),
        (
            "claude",
            "Claude Code",
            "Anthropic's terminal coding agent for Claude models.",
            "#D97757",
            false,
            3.7,
            0.25,
            -0.2,
            false,
        ),
        (
            "cursor",
            "Cursor",
            "Cursor's CLI agent, using the models enabled on your account.",
            "#1B1913",
            true,
            7.9,
            0.55,
            0.3,
            false,
        ),
        (
            "grok",
            "Grok",
            "xAI's Grok Build coding agent and model catalog.",
            "#000000",
            true,
            11.4,
            -0.15,
            0.5,
            false,
        ),
        (
            "opencode2",
            "OpenCode",
            "Open-source terminal agent with built-in multi-provider support.",
            "#211E1E",
            true,
            20.1,
            -0.5,
            -0.25,
            false,
        ),
        (
            "hermes",
            "Hermes",
            "Nous Research's terminal agent with tools, subagents, and provider profiles.",
            "#0000F2",
            true,
            15.8,
            0.4,
            -0.45,
            true,
        ),
    ];

    for (card, expected) in catalog.cards().iter().zip(expected) {
        assert_eq!(card.id, expected.0);
        assert_eq!(card.title, expected.1);
        assert_eq!(card.description, expected.2);
        assert_eq!(card.button_color, expected.3);
        assert_eq!(card.experimental, expected.4);
        assert_eq!(card.phase, expected.5);
        assert_eq!(card.x, expected.6);
        assert_eq!(card.y, expected.7);
        assert_eq!(card.external_auth, expected.8);
    }
}

#[test]
fn catalog_lookup_is_exact_and_missing_ids_return_none() {
    let catalog = harness_catalog();

    let hermes = catalog.lookup("hermes").expect("Hermes is catalogued");
    assert_eq!(hermes.title, "Hermes");
    assert_eq!(catalog.lookup_ref("hermes"), Some(&hermes));

    for missing in ["", " Hermes", "HERMES", "missing", "hermes ", "🚀"] {
        assert_eq!(
            catalog.lookup(missing),
            None,
            "lookup must be exact: {missing:?}"
        );
        assert_eq!(catalog.lookup_ref(missing), None);
    }

    let mut owned = catalog.lookup("codex").expect("Codex is catalogued");
    owned.title.clear();
    assert_eq!(
        catalog
            .lookup("codex")
            .expect("Codex remains catalogued")
            .title,
        "Codex"
    );
    assert_eq!(catalog.into_cards().len(), 6);
}

#[test]
fn hermes_external_auth_and_setup_url_are_exact() {
    let catalog = harness_catalog();
    assert_eq!(
        HERMES_SETUP_URL,
        "https://hermes-agent.nousresearch.com/docs/user-guide/configuring-models"
    );
    assert!(
        catalog
            .lookup("hermes")
            .expect("Hermes is catalogued")
            .external_auth
    );
    for id in ["codex", "claude", "cursor", "grok", "opencode2"] {
        assert!(
            !catalog
                .lookup(id)
                .expect("catalog id is present")
                .external_auth
        );
    }
}

#[test]
fn setup_actions_are_typed_exactly_and_only_none_is_not_actionable() {
    let cases = [
        ("install", HarnessSetupAction::Install, true),
        ("authenticate", HarnessSetupAction::Authenticate, true),
        (
            "open_authorization",
            HarnessSetupAction::OpenAuthorization,
            true,
        ),
        (
            "open_external_setup",
            HarnessSetupAction::OpenExternalSetup,
            true,
        ),
        ("none", HarnessSetupAction::None, false),
    ];

    for (raw, action, actionable) in cases {
        assert_eq!(HarnessSetupAction::from_raw(raw), Some(action));
        assert_eq!(action.as_str(), raw);
        assert_eq!(action.is_actionable(), actionable);
        assert_eq!(setup_is_actionable(action), actionable);
        assert_eq!(
            HarnessSetupState::new(action, false, false, "label", None, None).is_actionable(),
            actionable
        );
        assert_eq!(
            HarnessSetupState::default().action == action,
            action == HarnessSetupAction::None
        );
    }
    assert_eq!(HarnessSetupAction::from_raw(" Install"), None);
    assert_eq!(HarnessSetupAction::from_raw("future"), None);
}

#[test]
fn icon_precedence_is_ready_then_install_then_login() {
    let actions = [
        HarnessSetupAction::Install,
        HarnessSetupAction::Authenticate,
        HarnessSetupAction::OpenAuthorization,
        HarnessSetupAction::OpenExternalSetup,
        HarnessSetupAction::None,
    ];

    for action in actions {
        assert_eq!(setup_icon_for(true, action), HarnessSetupIcon::Ready);
        assert_eq!(
            HarnessSetupState::new(action, false, false, "label", None, None).icon(),
            if action == HarnessSetupAction::Install {
                HarnessSetupIcon::Download
            } else {
                HarnessSetupIcon::Login
            }
        );
    }
    assert_eq!(HarnessSetupIcon::Ready.as_str(), "ready");
    assert_eq!(HarnessSetupIcon::Download.as_str(), "download");
    assert_eq!(HarnessSetupIcon::Login.as_str(), "login");
}

#[test]
fn presentation_separates_ready_experimental_and_button_interaction_facts() {
    let experimental = HarnessCard::new(
        "cursor",
        "Cursor",
        "description",
        "#1B1913",
        true,
        7.9,
        0.55,
        0.3,
        false,
    );
    let ordinary = HarnessCard::new(
        "codex",
        "Codex",
        "description",
        "#000000",
        false,
        0.0,
        -0.4,
        0.1,
        false,
    );

    let waiting = present_harness_card(
        &experimental,
        &setup(
            HarnessSetupAction::OpenAuthorization,
            false,
            true,
            "Waiting for sign-in…",
            Some("account@example.test"),
            Some("  try again 🚀  "),
        ),
    );
    assert_eq!(waiting.icon, HarnessSetupIcon::Login);
    assert!(!waiting.ready);
    assert!(!waiting.show_installed_status);
    assert!(waiting.show_experimental_help);
    assert!(waiting.busy);
    assert!(waiting.actionable);
    assert!(!waiting.aria_disabled);
    assert!(!waiting.cursor_default);
    assert!(waiting.hover_enabled);
    assert_eq!(waiting.label, "Waiting for sign-in…");
    assert_eq!(waiting.email.as_deref(), Some("account@example.test"));
    assert_eq!(waiting.failure.as_deref(), Some("  try again 🚀  "));

    let ready = experimental.present(&setup(
        HarnessSetupAction::None,
        true,
        true,
        "Signed in",
        None,
        None,
    ));
    assert_eq!(ready.icon, HarnessSetupIcon::Ready);
    assert!(ready.ready);
    assert!(ready.show_installed_status);
    assert!(!ready.show_experimental_help);
    assert!(ready.busy);
    assert!(!ready.actionable);
    assert!(ready.aria_disabled);
    assert!(ready.cursor_default);
    assert!(!ready.hover_enabled);

    let ordinary_not_ready = ordinary.present(&setup(
        HarnessSetupAction::None,
        false,
        false,
        "Unavailable",
        None,
        None,
    ));
    assert!(!ordinary_not_ready.show_experimental_help);
    assert!(!ordinary_not_ready.busy);
    assert!(!ordinary_not_ready.actionable);
    assert!(ordinary_not_ready.aria_disabled);
    assert!(ordinary_not_ready.cursor_default);
    assert!(!ordinary_not_ready.hover_enabled);
}

#[test]
fn presentation_owns_and_preserves_labels_email_failure_and_empty_values() {
    let card = harness_catalog()
        .lookup("hermes")
        .expect("Hermes is catalogued");
    let state = HarnessSetupState::new(
        HarnessSetupAction::OpenExternalSetup,
        false,
        false,
        "  名前\n— café 🚀  ",
        Some(String::new()),
        Some(String::new()),
    );
    let presentation = present_harness_card(&card, &state);

    assert_eq!(presentation.label, "  名前\n— café 🚀  ");
    assert_eq!(presentation.email, Some(String::new()));
    assert_eq!(presentation.failure, Some(String::new()));
    assert_eq!(presentation.rendered_label(), "  名前\n— café 🚀   ");

    let mut returned = presentation.clone();
    returned.label.clear();
    returned.email = None;
    returned.failure = None;
    assert_eq!(presentation.label, "  名前\n— café 🚀  ");
    assert_eq!(presentation.email, Some(String::new()));
    assert_eq!(presentation.failure, Some(String::new()));
    assert_eq!(state.label, "  名前\n— café 🚀  ");
}

#[test]
fn forced_refresh_actions_are_installation_first_then_catalog_order() {
    let actions = forced_refresh_actions();
    assert_eq!(actions.len(), 7);
    assert_eq!(actions[0], HarnessRefreshAction::RefreshInstallations);

    let ids = actions
        .iter()
        .skip(1)
        .map(|action| match action {
            HarnessRefreshAction::LoadUsage { harness_id, force } => {
                assert!(*force);
                harness_id.as_str()
            }
            HarnessRefreshAction::RefreshInstallations => {
                panic!("installation refresh must be first")
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        ["codex", "claude", "cursor", "grok", "opencode2", "hermes"]
    );
}

#[test]
fn custom_refresh_order_is_owned_and_has_one_forced_load_per_card() {
    let cards = vec![
        HarnessCard::new("second", "", "", "", false, 0.0, 0.0, 0.0, false),
        HarnessCard::new("", "", "", "", false, 0.0, 0.0, 0.0, false),
        HarnessCard::new("世界🚀", "", "", "", false, 0.0, 0.0, 0.0, false),
    ];
    let actions = forced_refresh_actions_for(&cards);
    drop(cards);

    assert_eq!(
        actions,
        vec![
            HarnessRefreshAction::RefreshInstallations,
            HarnessRefreshAction::LoadUsage {
                harness_id: "second".to_owned(),
                force: true,
            },
            HarnessRefreshAction::LoadUsage {
                harness_id: String::new(),
                force: true,
            },
            HarnessRefreshAction::LoadUsage {
                harness_id: "世界🚀".to_owned(),
                force: true,
            },
        ]
    );
}

#[test]
fn completion_starts_idle_with_exact_footer_labels_and_unicode_saving_label() {
    let controller = OnboardingCompletionController::new();
    assert!(!controller.is_saving());
    assert_eq!(controller.visible_error(), None);
    assert_eq!(controller.current_token(), None);
    assert_eq!(controller.next_token(), 0);
    assert_eq!(controller.footer_label(), CONTINUE_FOOTER_LABEL);
    assert_eq!(CONTINUE_FOOTER_LABEL, "Continue");
    assert_eq!(SAVING_FOOTER_LABEL, "Saving…");
    assert_eq!(SAVING_FOOTER_LABEL.chars().last(), Some('…'));
}

#[test]
fn completion_admission_clears_error_marks_saving_and_emits_one_save_action() {
    let mut controller = OnboardingCompletionController::new();
    let first = controller.request_completion();
    let token = save_token(&first);

    assert_eq!(first.token(), Some(token));
    assert_eq!(token, CompletionSaveToken::new(1));
    assert_eq!(controller.current_token(), Some(token));
    assert_eq!(controller.next_token(), 1);
    assert!(controller.is_saving());
    assert_eq!(controller.footer_label(), SAVING_FOOTER_LABEL);
    assert_eq!(first.len(), 1);
    assert_eq!(
        first.actions(),
        &[CompletionAction::SaveDefaults {
            token,
            onboarding_completed: true,
        }]
    );

    let second = controller.request_completion();
    assert!(second.is_empty());
    assert_eq!(second.token(), None);
    assert_eq!(controller.current_token(), Some(token));
    assert_eq!(controller.next_token(), 1);
    assert!(controller.is_saving());

    // The transition owns its action values rather than borrowing the
    // controller's state.
    assert_eq!(
        first.into_actions(),
        vec![CompletionAction::SaveDefaults {
            token,
            onboarding_completed: true,
        }]
    );
}

#[test]
fn stale_settlement_is_inert_and_failure_clears_saving_without_navigation() {
    let mut controller = OnboardingCompletionController::new();
    let request = controller.request_completion();
    let token = save_token(&request);
    let stale = CompletionSaveToken::new(token.get() + 1);
    let before_stale = controller.clone();

    assert!(
        controller
            .settle(stale, CompletionOutcome::Failed)
            .is_empty()
    );
    assert_eq!(controller, before_stale);

    let failure = controller.settle(token, CompletionOutcome::Failed);
    assert!(failure.is_empty());
    assert_eq!(failure.actions(), &[]);
    assert!(!controller.is_saving());
    assert_eq!(controller.current_token(), None);
    assert_eq!(controller.footer_label(), CONTINUE_FOOTER_LABEL);
    assert_eq!(
        controller.visible_error(),
        Some(ONBOARDING_COMPLETION_FAILURE_MESSAGE)
    );
}

#[test]
fn success_clears_error_navigates_in_order_and_late_tokens_stay_inert() {
    let mut controller = OnboardingCompletionController::new();
    let first = controller.request_completion();
    let first_token = save_token(&first);
    let _ = controller.settle(first_token, CompletionOutcome::Failed);

    let retry = controller.request_completion();
    let retry_token = save_token(&retry);
    assert_eq!(retry_token, CompletionSaveToken::new(2));
    assert!(controller.visible_error().is_none());
    assert!(controller.is_saving());

    assert!(
        controller
            .settle(first_token, CompletionOutcome::Succeeded)
            .is_empty()
    );
    assert!(controller.is_saving());
    assert_eq!(controller.current_token(), Some(retry_token));
    assert!(controller.visible_error().is_none());

    let success = controller.settle(retry_token, CompletionOutcome::Succeeded);
    assert_eq!(
        success.actions(),
        &[CompletionAction::Navigate {
            path: ONBOARDING_COMPLETION_ROUTE.to_owned(),
        }]
    );
    assert_eq!(success.token(), None);
    assert!(!controller.is_saving());
    assert_eq!(controller.current_token(), None);
    assert_eq!(controller.visible_error(), None);
    assert_eq!(controller.footer_label(), CONTINUE_FOOTER_LABEL);

    let settled = controller.clone();
    assert!(
        controller
            .settle(retry_token, CompletionOutcome::Failed)
            .is_empty()
    );
    assert_eq!(controller, settled);

    let third = controller.request_completion();
    assert_eq!(save_token(&third), CompletionSaveToken::new(3));
    assert!(controller.is_saving());
}

#[test]
fn completion_transition_helpers_and_empty_default_are_exact() {
    let empty = CompletionTransition::default();
    assert!(empty.is_empty());
    assert_eq!(empty.len(), 0);
    assert_eq!(empty.token(), None);
    assert_eq!(empty.actions(), &[]);
    assert_eq!(empty.into_actions(), Vec::<CompletionAction>::new());

    let action = CompletionAction::Navigate {
        path: String::from("/"),
    };
    let transition = CompletionTransition::new(None, vec![action.clone()]);
    assert!(!transition.is_empty());
    assert_eq!(transition.len(), 1);
    assert_eq!(transition.actions(), &[action]);
    assert_eq!(
        transition.into_actions(),
        vec![CompletionAction::Navigate {
            path: String::from("/"),
        }]
    );
}

#[test]
fn empty_setup_strings_remain_empty_and_default_action_is_none() {
    let state = HarnessSetupState::default();
    assert_eq!(state.action, HarnessSetupAction::None);
    assert!(!state.ready);
    assert!(!state.busy);
    assert!(state.label.is_empty());
    assert_eq!(state.email, None);
    assert_eq!(state.failure, None);

    let empty = setup(
        HarnessSetupAction::None,
        false,
        false,
        "",
        Some(""),
        Some(""),
    );
    let presentation = present_harness_card(&harness_catalog().cards()[0], &empty);
    assert_eq!(presentation.label, "");
    assert_eq!(presentation.email, Some(String::new()));
    assert_eq!(presentation.failure, Some(String::new()));
    assert_eq!(presentation.rendered_label(), " ");
}
