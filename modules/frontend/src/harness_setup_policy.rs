//! Dependency-free presentation policy for onboarding harness setup.
//!
//! This is the native counterpart of
//! `routes/components/onboarding/setup-state.ts`. The caller supplies
//! already-decoded installation and usage observations; this module only
//! chooses the label, status, and action to present. It does not perform
//! installation, authentication, browser, network, or controller work.

#![allow(clippy::module_name_repetitions)]
#![forbid(unsafe_code)]

/// A setup action that a later UI or host boundary may execute.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HarnessSetupAction {
    /// Start provider authentication for a managed installation.
    Authenticate,
    /// Start or retry installing an unmanaged harness.
    Install,
    /// Open the authorization URL supplied by an in-flight authentication.
    OpenAuthorization,
    /// Open the provider's external setup documentation.
    OpenExternalSetup,
    /// No action is currently available.
    None,
}

impl HarnessSetupAction {
    /// Returns the exact legacy action value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Authenticate => "authenticate",
            Self::Install => "install",
            Self::OpenAuthorization => "open_authorization",
            Self::OpenExternalSetup => "open_external_setup",
            Self::None => "none",
        }
    }
}

/// The setup status shown for one harness.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HarnessSetupStatus {
    /// Installation status has not been received yet.
    Checking,
    /// Installation is active or waiting for an unmanaged install to settle.
    Downloading,
    /// Setup has failed and may be retried.
    Failed,
    /// Installation and authentication are ready.
    Ready,
    /// A setup action is available.
    SignIn,
    /// Authentication is waiting for browser/device completion.
    WaitingForSignIn,
}

impl HarnessSetupStatus {
    /// Returns the exact legacy status value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Checking => "checking",
            Self::Downloading => "downloading",
            Self::Failed => "failed",
            Self::Ready => "ready",
            Self::SignIn => "sign_in",
            Self::WaitingForSignIn => "waiting_for_sign_in",
        }
    }
}

/// The lifecycle activity of an Artisan-managed installation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum InstallationActivity {
    /// Authentication is waiting for the provider to complete.
    Authenticating,
    /// The latest installation attempt failed.
    Failed,
    /// No installation operation is active.
    Idle,
    /// An installation operation is active.
    Installing,
}

impl InstallationActivity {
    /// Returns the exact legacy activity value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Authenticating => "authenticating",
            Self::Failed => "failed",
            Self::Idle => "idle",
            Self::Installing => "installing",
        }
    }
}

/// The optional phase reported during installation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum InstallationPhase {
    /// Checking the release channel.
    Checking,
    /// Downloading the selected release.
    Downloading,
    /// Preparing the managed home.
    Provisioning,
    /// Resolving a release version.
    Resolving,
    /// Staging the downloaded release.
    Staging,
    /// Verifying the staged release.
    Verifying,
}

impl InstallationPhase {
    /// Returns the exact legacy phase value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Checking => "checking",
            Self::Downloading => "downloading",
            Self::Provisioning => "provisioning",
            Self::Resolving => "resolving",
            Self::Staging => "staging",
            Self::Verifying => "verifying",
        }
    }

    /// Returns the exact user-facing label for this phase.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Checking => "Checking release…",
            Self::Downloading => "Downloading…",
            Self::Provisioning => "Preparing home…",
            Self::Resolving => "Resolving release…",
            Self::Staging => "Staging…",
            Self::Verifying => "Verifying…",
        }
    }
}

/// Browser authorization data relevant to the setup projection.
///
/// The legacy report carries additional attempt metadata, but the onboarding
/// state only reads its URL. Secrets and execution details therefore remain
/// outside this policy type.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct InstallationAuthorization {
    /// URL that a later browser boundary may open.
    pub url: String,
}

impl InstallationAuthorization {
    /// Creates authorization data from its already-decoded URL.
    #[must_use]
    pub fn new(url: impl Into<String>) -> Self {
        Self { url: url.into() }
    }
}

/// The decoded installation fields consumed by onboarding presentation.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct InstallationReport {
    /// Current installation lifecycle activity.
    pub activity: InstallationActivity,
    /// Optional safe user-facing detail for the active operation.
    pub activity_detail: Option<String>,
    /// Optional structured phase for the active operation.
    pub activity_phase: Option<InstallationPhase>,
    /// Optional browser authorization data for authentication.
    pub authorization: Option<InstallationAuthorization>,
    /// Whether the managed home already contains provider credentials.
    pub credentials_present: bool,
    /// Optional failure reported by the installation service.
    pub failure: Option<String>,
    /// Whether the active binary is Artisan-managed.
    pub managed: bool,
}

impl InstallationReport {
    /// Creates a report with no optional detail, authorization, or failure
    /// fields.
    #[must_use]
    pub const fn new(
        managed: bool,
        credentials_present: bool,
        activity: InstallationActivity,
    ) -> Self {
        Self {
            activity,
            activity_detail: None,
            activity_phase: None,
            authorization: None,
            credentials_present,
            failure: None,
            managed,
        }
    }
}

/// The decoded authentication state of a provider usage report.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum UsageAuthentication {
    /// The provider account is authenticated.
    Authenticated,
    /// The provider account is known to be unauthenticated.
    Unauthenticated,
    /// The provider account state is not known.
    Unknown,
}

impl UsageAuthentication {
    /// Returns the exact legacy authentication value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Authenticated => "authenticated",
            Self::Unauthenticated => "unauthenticated",
            Self::Unknown => "unknown",
        }
    }
}

/// The usage fields consumed by onboarding presentation.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct UsageReport {
    /// Provider account authentication classification.
    pub authentication: UsageAuthentication,
    /// Optional provider account email.
    pub account_email: Option<String>,
}

impl UsageReport {
    /// Creates a usage report with the supplied authentication and email.
    #[must_use]
    pub fn new(authentication: UsageAuthentication, account_email: Option<String>) -> Self {
        Self {
            account_email,
            authentication,
        }
    }
}

/// Inputs already decoded by the installation and usage controllers.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct HarnessSetupInput {
    /// Whether the installation controller currently has usable state.
    pub available: bool,
    /// Optional controller-level error for the selected harness.
    pub error: Option<String>,
    /// Whether this harness requires external provider setup.
    pub external_auth: Option<bool>,
    /// Whether an installation or authentication request is pending.
    pub pending: bool,
    /// Latest installation report, when available.
    pub report: Option<InstallationReport>,
    /// Latest usage report, when available.
    pub usage: Option<UsageReport>,
}

/// The deterministic state consumed by an onboarding renderer.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct HarnessSetupState {
    /// Action a host boundary may execute, if any.
    pub action: HarnessSetupAction,
    /// Authorization URL when the action is [`HarnessSetupAction::OpenAuthorization`].
    pub authorization_url: Option<String>,
    /// Whether the presentation represents an in-flight operation.
    pub busy: bool,
    /// Provider email when the ready state has one.
    pub email: Option<String>,
    /// Controller or installation failure to display, when present.
    pub failure: Option<String>,
    /// Exact user-facing label.
    pub label: String,
    /// Whether setup is complete.
    pub ready: bool,
    /// Exact setup status.
    pub status: HarnessSetupStatus,
}

const INSTALLATION_STATUS_UNAVAILABLE_FAILURE: &str = "Installation status is unavailable.";
const SETUP_DID_NOT_COMPLETE_FAILURE: &str = "Setup did not complete.";

fn signed_in(usage: Option<UsageReport>) -> HarnessSetupState {
    let email = usage.and_then(|report| report.account_email);
    let has_email = email.is_some();
    HarnessSetupState {
        action: HarnessSetupAction::None,
        authorization_url: None,
        busy: false,
        email,
        failure: None,
        label: if has_email {
            String::from("Signed in as")
        } else {
            String::from("Signed in")
        },
        ready: true,
        status: HarnessSetupStatus::Ready,
    }
}

fn installation_progress_label(report: &InstallationReport) -> String {
    if let Some(detail) = report.activity_detail.as_ref() {
        return detail.clone();
    }

    report
        .activity_phase
        .map(InstallationPhase::label)
        .unwrap_or("Installing…")
        .to_owned()
}

/// Projects decoded harness setup observations into the legacy onboarding
/// state.
///
/// The branch order intentionally mirrors `ProjectManagedHarnessSetup`:
/// availability, active installation/pending unmanaged work, authenticating,
/// failed, ready credentials, unmanaged download, external Hermes setup, and
/// managed sign-in. Optional strings are moved or cloned without trimming,
/// normalizing, or otherwise changing their presence or contents, and the
/// returned state owns every string it exposes.
#[must_use]
pub fn project_managed_harness_setup(input: HarnessSetupInput) -> HarnessSetupState {
    if !input.available {
        return HarnessSetupState {
            action: HarnessSetupAction::None,
            authorization_url: None,
            busy: true,
            email: None,
            failure: None,
            label: String::from("Checking…"),
            ready: false,
            status: HarnessSetupStatus::Checking,
        };
    }

    let Some(report) = input.report else {
        return HarnessSetupState {
            action: HarnessSetupAction::None,
            authorization_url: None,
            busy: false,
            email: None,
            failure: Some(INSTALLATION_STATUS_UNAVAILABLE_FAILURE.to_owned()),
            label: String::from("Unavailable"),
            ready: false,
            status: HarnessSetupStatus::Failed,
        };
    };

    if report.activity == InstallationActivity::Installing || (input.pending && !report.managed) {
        return HarnessSetupState {
            action: HarnessSetupAction::None,
            authorization_url: None,
            busy: true,
            email: None,
            failure: None,
            label: installation_progress_label(&report),
            ready: false,
            status: HarnessSetupStatus::Downloading,
        };
    }

    if report.activity == InstallationActivity::Authenticating {
        let authorization_url = report.authorization.map(|authorization| authorization.url);
        return HarnessSetupState {
            action: if authorization_url.is_some() {
                HarnessSetupAction::OpenAuthorization
            } else {
                HarnessSetupAction::None
            },
            authorization_url,
            busy: true,
            email: None,
            failure: None,
            label: String::from("Waiting for sign-in…"),
            ready: false,
            status: HarnessSetupStatus::WaitingForSignIn,
        };
    }

    if report.activity == InstallationActivity::Failed {
        let failure = input
            .error
            .or(report.failure)
            .or_else(|| Some(SETUP_DID_NOT_COMPLETE_FAILURE.to_owned()));
        let managed = report.managed;
        return HarnessSetupState {
            action: if managed {
                HarnessSetupAction::Authenticate
            } else {
                HarnessSetupAction::Install
            },
            authorization_url: None,
            busy: false,
            email: None,
            failure,
            label: if managed {
                String::from("Try Sign In Again")
            } else {
                String::from("Retry Download")
            },
            ready: false,
            status: HarnessSetupStatus::Failed,
        };
    }

    if report.credentials_present
        || input
            .usage
            .as_ref()
            .is_some_and(|usage| usage.authentication == UsageAuthentication::Authenticated)
    {
        return signed_in(input.usage);
    }

    if !report.managed {
        let error = input.error;
        let has_error = error.is_some();
        return HarnessSetupState {
            action: HarnessSetupAction::Install,
            authorization_url: None,
            busy: false,
            email: None,
            failure: error,
            label: if has_error {
                String::from("Retry Download")
            } else {
                String::from("Download")
            },
            ready: false,
            status: HarnessSetupStatus::SignIn,
        };
    }

    if input.external_auth == Some(true) {
        return HarnessSetupState {
            action: HarnessSetupAction::OpenExternalSetup,
            authorization_url: None,
            busy: false,
            email: None,
            failure: input.error,
            label: String::from("Configure Hermes"),
            ready: false,
            status: HarnessSetupStatus::SignIn,
        };
    }

    HarnessSetupState {
        action: HarnessSetupAction::Authenticate,
        authorization_url: None,
        busy: input.pending,
        email: None,
        failure: input.error,
        label: if input.pending {
            String::from("Starting sign-in…")
        } else {
            String::from("Sign In")
        },
        ready: false,
        status: HarnessSetupStatus::SignIn,
    }
}
