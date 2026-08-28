//! Dependency-free parity tests for the onboarding harness-setup policy.

#[path = "../../modules/frontend/src/harness_setup_policy.rs"]
mod harness_setup_policy;

use harness_setup_policy::{
    HarnessSetupAction, HarnessSetupInput, HarnessSetupState, HarnessSetupStatus,
    InstallationActivity, InstallationAuthorization, InstallationPhase, InstallationReport,
    UsageAuthentication, UsageReport, project_managed_harness_setup,
};

fn report(
    activity: InstallationActivity,
    managed: bool,
    credentials_present: bool,
) -> InstallationReport {
    InstallationReport::new(managed, credentials_present, activity)
}

fn input(report: Option<InstallationReport>) -> HarnessSetupInput {
    HarnessSetupInput {
        available: true,
        error: None,
        external_auth: None,
        pending: false,
        report,
        usage: None,
    }
}

macro_rules! assert_state {
    (
        $actual:expr,
        $action:expr,
        $authorization_url:expr,
        $busy:expr,
        $email:expr,
        $failure:expr,
        $label:expr,
        $ready:expr,
        $status:expr $(,)?
    ) => {{
        assert_eq!(
            $actual,
            HarnessSetupState {
                action: $action,
                authorization_url: $authorization_url.map(str::to_owned),
                busy: $busy,
                email: $email.map(str::to_owned),
                failure: $failure.map(str::to_owned),
                label: $label.to_owned(),
                ready: $ready,
                status: $status,
            }
        );
    }};
}

#[test]
fn discriminants_and_phase_labels_match_the_legacy_contract() {
    assert_eq!(HarnessSetupAction::Authenticate.as_str(), "authenticate");
    assert_eq!(HarnessSetupAction::Install.as_str(), "install");
    assert_eq!(
        HarnessSetupAction::OpenAuthorization.as_str(),
        "open_authorization"
    );
    assert_eq!(
        HarnessSetupAction::OpenExternalSetup.as_str(),
        "open_external_setup"
    );
    assert_eq!(HarnessSetupAction::None.as_str(), "none");

    assert_eq!(HarnessSetupStatus::Checking.as_str(), "checking");
    assert_eq!(HarnessSetupStatus::Downloading.as_str(), "downloading");
    assert_eq!(HarnessSetupStatus::Failed.as_str(), "failed");
    assert_eq!(HarnessSetupStatus::Ready.as_str(), "ready");
    assert_eq!(HarnessSetupStatus::SignIn.as_str(), "sign_in");
    assert_eq!(
        HarnessSetupStatus::WaitingForSignIn.as_str(),
        "waiting_for_sign_in"
    );

    assert_eq!(
        [
            InstallationActivity::Authenticating,
            InstallationActivity::Failed,
            InstallationActivity::Idle,
            InstallationActivity::Installing,
        ]
        .map(InstallationActivity::as_str),
        ["authenticating", "failed", "idle", "installing"]
    );
    assert_eq!(
        [
            UsageAuthentication::Authenticated,
            UsageAuthentication::Unauthenticated,
            UsageAuthentication::Unknown,
        ]
        .map(UsageAuthentication::as_str),
        ["authenticated", "unauthenticated", "unknown"]
    );

    let phases = [
        (InstallationPhase::Checking, "checking", "Checking release…"),
        (
            InstallationPhase::Downloading,
            "downloading",
            "Downloading…",
        ),
        (
            InstallationPhase::Provisioning,
            "provisioning",
            "Preparing home…",
        ),
        (
            InstallationPhase::Resolving,
            "resolving",
            "Resolving release…",
        ),
        (InstallationPhase::Staging, "staging", "Staging…"),
        (InstallationPhase::Verifying, "verifying", "Verifying…"),
    ];
    for (phase, value, label) in phases {
        assert_eq!(phase.as_str(), value);
        assert_eq!(phase.label(), label);
    }
}

#[test]
fn unavailable_and_missing_report_are_the_first_decisions() {
    let mut unavailable_input = input(Some(report(InstallationActivity::Installing, true, true)));
    unavailable_input.available = false;
    unavailable_input.error = Some("controller error".to_owned());
    unavailable_input.external_auth = Some(true);
    unavailable_input.pending = true;
    unavailable_input.usage = Some(UsageReport::new(
        UsageAuthentication::Authenticated,
        Some("user@example.test".to_owned()),
    ));
    assert_state!(
        project_managed_harness_setup(unavailable_input),
        HarnessSetupAction::None,
        None,
        true,
        None,
        None,
        "Checking…",
        false,
        HarnessSetupStatus::Checking,
    );

    let mut missing_report = input(None);
    missing_report.error = Some("controller error".to_owned());
    missing_report.external_auth = Some(true);
    missing_report.pending = true;
    missing_report.usage = Some(UsageReport::new(
        UsageAuthentication::Authenticated,
        Some("user@example.test".to_owned()),
    ));
    assert_state!(
        project_managed_harness_setup(missing_report),
        HarnessSetupAction::None,
        None,
        false,
        None,
        Some("Installation status is unavailable."),
        "Unavailable",
        false,
        HarnessSetupStatus::Failed,
    );
}

#[test]
fn installing_detail_takes_precedence_over_each_phase_and_fallback() {
    let phases = [
        (Some(InstallationPhase::Checking), "Checking release…"),
        (Some(InstallationPhase::Downloading), "Downloading…"),
        (Some(InstallationPhase::Provisioning), "Preparing home…"),
        (Some(InstallationPhase::Resolving), "Resolving release…"),
        (Some(InstallationPhase::Staging), "Staging…"),
        (Some(InstallationPhase::Verifying), "Verifying…"),
        (None, "Installing…"),
    ];

    for (phase, label) in phases {
        let mut installing = report(InstallationActivity::Installing, true, false);
        installing.activity_phase = phase;
        assert_state!(
            project_managed_harness_setup(input(Some(installing))),
            HarnessSetupAction::None,
            None,
            true,
            None,
            None,
            label,
            false,
            HarnessSetupStatus::Downloading,
        );
    }

    let mut detailed = report(InstallationActivity::Installing, true, true);
    detailed.activity_detail = Some(String::new());
    detailed.activity_phase = Some(InstallationPhase::Verifying);
    assert_state!(
        project_managed_harness_setup(input(Some(detailed.clone()))),
        HarnessSetupAction::None,
        None,
        true,
        None,
        None,
        "",
        false,
        HarnessSetupStatus::Downloading,
    );

    detailed.activity_detail = Some("Fetching release metadata".to_owned());
    assert_state!(
        project_managed_harness_setup(input(Some(detailed))),
        HarnessSetupAction::None,
        None,
        true,
        None,
        None,
        "Fetching release metadata",
        false,
        HarnessSetupStatus::Downloading,
    );
}

#[test]
fn installing_and_pending_unmanaged_work_precede_later_states() {
    let mut pending_idle = input(Some(report(InstallationActivity::Idle, false, false)));
    pending_idle.pending = true;
    assert_state!(
        project_managed_harness_setup(pending_idle),
        HarnessSetupAction::None,
        None,
        true,
        None,
        None,
        "Installing…",
        false,
        HarnessSetupStatus::Downloading,
    );

    let mut pending_failed = input(Some(report(InstallationActivity::Failed, false, false)));
    pending_failed.pending = true;
    pending_failed.error = Some("controller error".to_owned());
    assert_state!(
        project_managed_harness_setup(pending_failed),
        HarnessSetupAction::None,
        None,
        true,
        None,
        None,
        "Installing…",
        false,
        HarnessSetupStatus::Downloading,
    );

    let mut pending_authenticating = input(Some(report(
        InstallationActivity::Authenticating,
        false,
        false,
    )));
    pending_authenticating.pending = true;
    pending_authenticating.error = Some("ignored while busy".to_owned());
    let mut authenticating_report = pending_authenticating.report.unwrap();
    authenticating_report.authorization =
        Some(InstallationAuthorization::new("https://example.test/auth"));
    pending_authenticating.report = Some(authenticating_report);
    assert_state!(
        project_managed_harness_setup(pending_authenticating),
        HarnessSetupAction::None,
        None,
        true,
        None,
        None,
        "Installing…",
        false,
        HarnessSetupStatus::Downloading,
    );

    let mut managed_pending = input(Some(report(InstallationActivity::Idle, true, false)));
    managed_pending.pending = true;
    assert_state!(
        project_managed_harness_setup(managed_pending),
        HarnessSetupAction::Authenticate,
        None,
        true,
        None,
        None,
        "Starting sign-in…",
        false,
        HarnessSetupStatus::SignIn,
    );
}

#[test]
fn authenticating_waits_and_only_exposes_present_authorization() {
    let mut without_authorization = input(Some(report(
        InstallationActivity::Authenticating,
        true,
        true,
    )));
    without_authorization.error = Some("ignored while authenticating".to_owned());
    without_authorization.usage = Some(UsageReport::new(
        UsageAuthentication::Authenticated,
        Some("ignored@example.test".to_owned()),
    ));
    assert_state!(
        project_managed_harness_setup(without_authorization.clone()),
        HarnessSetupAction::None,
        None,
        true,
        None,
        None,
        "Waiting for sign-in…",
        false,
        HarnessSetupStatus::WaitingForSignIn,
    );

    let mut with_authorization = without_authorization;
    let mut report = with_authorization.report.unwrap();
    report.authorization = Some(InstallationAuthorization::new("https://example.test/auth"));
    with_authorization.report = Some(report.clone());
    assert_state!(
        project_managed_harness_setup(with_authorization.clone()),
        HarnessSetupAction::OpenAuthorization,
        Some("https://example.test/auth"),
        true,
        None,
        None,
        "Waiting for sign-in…",
        false,
        HarnessSetupStatus::WaitingForSignIn,
    );

    report.authorization = Some(InstallationAuthorization::new(""));
    with_authorization.report = Some(report);
    assert_state!(
        project_managed_harness_setup(with_authorization),
        HarnessSetupAction::OpenAuthorization,
        Some(""),
        true,
        None,
        None,
        "Waiting for sign-in…",
        false,
        HarnessSetupStatus::WaitingForSignIn,
    );
}

#[test]
fn failed_state_uses_controller_error_then_report_failure_then_fallback() {
    for (managed, action, label) in [
        (true, HarnessSetupAction::Authenticate, "Try Sign In Again"),
        (false, HarnessSetupAction::Install, "Retry Download"),
    ] {
        let mut with_both = input(Some(report(InstallationActivity::Failed, managed, true)));
        with_both.error = Some("controller failure".to_owned());
        let mut installation_report = with_both.report.unwrap();
        installation_report.failure = Some("installation failure".to_owned());
        with_both.report = Some(installation_report);
        with_both.usage = Some(UsageReport::new(
            UsageAuthentication::Authenticated,
            Some("ignored@example.test".to_owned()),
        ));
        assert_state!(
            project_managed_harness_setup(with_both),
            action,
            None,
            false,
            None,
            Some("controller failure"),
            label,
            false,
            HarnessSetupStatus::Failed,
        );

        let mut report_failure = input(Some(report(InstallationActivity::Failed, managed, false)));
        let mut installation_report = report_failure.report.unwrap();
        installation_report.failure = Some("installation failure".to_owned());
        report_failure.report = Some(installation_report);
        assert_state!(
            project_managed_harness_setup(report_failure),
            action,
            None,
            false,
            None,
            Some("installation failure"),
            label,
            false,
            HarnessSetupStatus::Failed,
        );

        assert_state!(
            project_managed_harness_setup(input(Some(report(
                InstallationActivity::Failed,
                managed,
                false,
            )))),
            action,
            None,
            false,
            None,
            Some("Setup did not complete."),
            label,
            false,
            HarnessSetupStatus::Failed,
        );
    }
}

#[test]
fn credentials_or_authenticated_usage_produce_ready_state_with_optional_email() {
    assert_state!(
        project_managed_harness_setup(input(Some(report(InstallationActivity::Idle, true, true)))),
        HarnessSetupAction::None,
        None,
        false,
        None,
        None,
        "Signed in",
        true,
        HarnessSetupStatus::Ready,
    );

    let mut credentials_with_email = input(Some(report(InstallationActivity::Idle, true, true)));
    credentials_with_email.usage = Some(UsageReport::new(
        UsageAuthentication::Unknown,
        Some("person@example.test".to_owned()),
    ));
    assert_state!(
        project_managed_harness_setup(credentials_with_email),
        HarnessSetupAction::None,
        None,
        false,
        Some("person@example.test"),
        None,
        "Signed in as",
        true,
        HarnessSetupStatus::Ready,
    );

    for authentication in [
        UsageAuthentication::Authenticated,
        UsageAuthentication::Unauthenticated,
        UsageAuthentication::Unknown,
    ] {
        let mut usage_input = input(Some(report(InstallationActivity::Idle, true, false)));
        usage_input.usage = Some(UsageReport::new(
            authentication,
            Some("usage@example.test".to_owned()),
        ));
        let expected_ready = authentication == UsageAuthentication::Authenticated;
        let state = project_managed_harness_setup(usage_input);
        assert_eq!(
            state.ready, expected_ready,
            "authentication={authentication:?}"
        );
        if expected_ready {
            assert_eq!(state.action, HarnessSetupAction::None);
            assert_eq!(state.email.as_deref(), Some("usage@example.test"));
            assert_eq!(state.label, "Signed in as");
            assert_eq!(state.status, HarnessSetupStatus::Ready);
        } else {
            assert_eq!(state.action, HarnessSetupAction::Authenticate);
            assert!(state.email.is_none());
            assert_eq!(state.label, "Sign In");
            assert_eq!(state.status, HarnessSetupStatus::SignIn);
        }
    }
}

#[test]
fn unmanaged_harness_uses_download_action_and_preserves_error_presence() {
    assert_state!(
        project_managed_harness_setup(input(Some(report(
            InstallationActivity::Idle,
            false,
            false,
        )))),
        HarnessSetupAction::Install,
        None,
        false,
        None,
        None,
        "Download",
        false,
        HarnessSetupStatus::SignIn,
    );

    let mut with_error = input(Some(report(InstallationActivity::Idle, false, false)));
    with_error.error = Some(String::new());
    assert_state!(
        project_managed_harness_setup(with_error.clone()),
        HarnessSetupAction::Install,
        None,
        false,
        None,
        Some(""),
        "Retry Download",
        false,
        HarnessSetupStatus::SignIn,
    );

    with_error.error = Some("download failed".to_owned());
    assert_state!(
        project_managed_harness_setup(with_error),
        HarnessSetupAction::Install,
        None,
        false,
        None,
        Some("download failed"),
        "Retry Download",
        false,
        HarnessSetupStatus::SignIn,
    );
}

#[test]
fn external_auth_is_exact_true_and_precedes_managed_sign_in() {
    {
        let external_auth = Some(true);
        let mut external = input(Some(report(InstallationActivity::Idle, true, false)));
        external.external_auth = external_auth;
        external.pending = true;
        external.error = Some("configuration unavailable".to_owned());
        assert_state!(
            project_managed_harness_setup(external),
            HarnessSetupAction::OpenExternalSetup,
            None,
            false,
            None,
            Some("configuration unavailable"),
            "Configure Hermes",
            false,
            HarnessSetupStatus::SignIn,
        );
    }

    for external_auth in [None, Some(false)] {
        let mut managed = input(Some(report(InstallationActivity::Idle, true, false)));
        managed.external_auth = external_auth;
        managed.error = Some("sign-in unavailable".to_owned());
        assert_state!(
            project_managed_harness_setup(managed),
            HarnessSetupAction::Authenticate,
            None,
            false,
            None,
            Some("sign-in unavailable"),
            "Sign In",
            false,
            HarnessSetupStatus::SignIn,
        );
    }
}

#[test]
fn managed_sign_in_pending_changes_only_busy_and_label() {
    for (pending, busy, label) in [(false, false, "Sign In"), (true, true, "Starting sign-in…")] {
        let mut managed = input(Some(report(InstallationActivity::Idle, true, false)));
        managed.pending = pending;
        managed.error = Some("authentication unavailable".to_owned());
        assert_state!(
            project_managed_harness_setup(managed),
            HarnessSetupAction::Authenticate,
            None,
            busy,
            None,
            Some("authentication unavailable"),
            label,
            false,
            HarnessSetupStatus::SignIn,
        );
    }
}

#[test]
fn projected_state_owns_text_after_source_reports_are_dropped() {
    let (progress, authenticating, ready, failed) = {
        let mut installing = report(InstallationActivity::Installing, true, false);
        installing.activity_detail = Some(String::from("Preparing a source-owned detail"));
        let progress = project_managed_harness_setup(input(Some(installing)));

        let mut authenticating_report = report(InstallationActivity::Authenticating, true, false);
        authenticating_report.authorization = Some(InstallationAuthorization::new(String::from(
            "https://example.test/source-owned-auth",
        )));
        let authenticating = project_managed_harness_setup(input(Some(authenticating_report)));

        let ready_input = HarnessSetupInput {
            available: true,
            error: None,
            external_auth: None,
            pending: false,
            report: Some(report(InstallationActivity::Idle, true, false)),
            usage: Some(UsageReport::new(
                UsageAuthentication::Authenticated,
                Some(String::from("source-owned@example.test")),
            )),
        };
        let ready = project_managed_harness_setup(ready_input);

        let mut failed_report = report(InstallationActivity::Failed, true, false);
        failed_report.failure = Some(String::from("source-owned failure"));
        let failed = project_managed_harness_setup(input(Some(failed_report)));

        (progress, authenticating, ready, failed)
    };

    assert_eq!(progress.label, "Preparing a source-owned detail");
    assert_eq!(
        authenticating.authorization_url.as_deref(),
        Some("https://example.test/source-owned-auth")
    );
    assert_eq!(ready.email.as_deref(), Some("source-owned@example.test"));
    assert_eq!(failed.failure.as_deref(), Some("source-owned failure"));
}
