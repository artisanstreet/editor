pub use crate::core::account::Provider;
use crate::core::account::{AccountPhase, Factor};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EmailProblem {
    Missing,
    Malformed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Step {
    Entering,
    Verifying,
    Rejected(Factor),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum View {
    Restoring,
    Form { problem: Option<EmailProblem> },
    Sending,
    Browser { provider: Provider },
    Waiting,
    Approved,
    SecondFactor { step: Step },
    Finishing,
    Failed { message: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Page {
    Email,
    Code,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Feedback {
    None,
    Error(String),
    Success(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Link {
    Terms,
    Privacy,
    Recover,
}

impl Link {
    #[must_use]
    pub const fn path(self) -> &'static str {
        match self {
            Self::Terms => "/terms",
            Self::Privacy => "/privacy",
            Self::Recover => "/recover",
        }
    }
}

#[must_use]
pub fn view_of(phase: &AccountPhase, problem: Option<EmailProblem>) -> View {
    match phase {
        AccountPhase::SignedOut => View::Form { problem },
        AccountPhase::Restoring | AccountPhase::Authenticated { .. } => View::Restoring,
        AccountPhase::Requesting => View::Sending,
        AccountPhase::Browser { provider } => View::Browser {
            provider: *provider,
        },
        AccountPhase::Pending { .. } => View::Waiting,
        AccountPhase::EmailApproved { approval, rejected } => match (approval.totp_required, rejected) {
            (true, Some(factor)) => View::SecondFactor {
                step: Step::Rejected(*factor),
            },
            (true, None) => View::SecondFactor {
                step: Step::Entering,
            },
            (false, _) => View::Approved,
        },
        AccountPhase::Finalizing { approval, .. } => match approval.totp_required {
            true => View::SecondFactor {
                step: Step::Verifying,
            },
            false => View::Finishing,
        },
        AccountPhase::Error { message } => View::Failed {
            message: message.clone(),
        },
    }
}

#[must_use]
pub const fn page_of(view: &View) -> Page {
    match view {
        View::SecondFactor { .. } => Page::Code,
        View::Restoring
        | View::Form { .. }
        | View::Sending
        | View::Browser { .. }
        | View::Waiting
        | View::Approved
        | View::Finishing
        | View::Failed { .. } => Page::Email,
    }
}

#[must_use]
pub const fn locked(view: &View) -> bool {
    match view {
        View::Form { .. } | View::Browser { .. } | View::Failed { .. } => false,
        View::SecondFactor { step } => matches!(step, Step::Verifying),
        View::Restoring | View::Sending | View::Waiting | View::Approved | View::Finishing => true,
    }
}

#[must_use]
pub const fn cancellable(view: &View) -> bool {
    matches!(
        view,
        View::Sending
            | View::Browser { .. }
            | View::Waiting
            | View::Approved
            | View::SecondFactor { .. }
            | View::Finishing
    )
}

#[must_use]
pub fn feedback_of(view: &View) -> Feedback {
    match view {
        View::Form {
            problem: Some(EmailProblem::Missing),
        } => Feedback::Error("Enter your email address and we'll send you a sign-in link.".into()),
        View::Form {
            problem: Some(EmailProblem::Malformed),
        } => Feedback::Error(
            "That doesn't look like an email address. Check for typos and try again.".into(),
        ),
        View::Waiting => Feedback::Success("Check your email for the sign-in link.".into()),
        View::Browser { provider } => Feedback::Success(format!(
            "Continue in your browser to sign in with {}.",
            provider.name()
        )),
        View::Approved => Feedback::Success("Approved. Finish signing in.".into()),
        View::SecondFactor {
            step: Step::Rejected(Factor::Authenticator),
        } => Feedback::Error(
            "That code didn't match. Codes refresh every 30 seconds, so try the latest one.".into(),
        ),
        View::SecondFactor {
            step: Step::Rejected(Factor::Recovery),
        } => Feedback::Error(
            "That recovery code didn't work. Each one only works once, so try another.".into(),
        ),
        View::Failed { message } => Feedback::Error(message.clone()),
        View::Form { problem: None }
        | View::Restoring
        | View::Sending
        | View::SecondFactor { .. }
        | View::Finishing => Feedback::None,
    }
}

pub fn checked_email(value: &str) -> Result<&str, EmailProblem> {
    let email = value.trim();
    let shaped = email.split_once('@').is_some_and(|(local, domain)| {
        !local.is_empty()
            && !domain.contains('@')
            && domain
                .split_once('.')
                .is_some_and(|(name, tld)| !name.is_empty() && !tld.is_empty() && !tld.ends_with('.'))
            && !email.chars().any(char::is_whitespace)
    });
    match (email.is_empty(), shaped) {
        (true, _) => Err(EmailProblem::Missing),
        (false, true) => Ok(email),
        (false, false) => Err(EmailProblem::Malformed),
    }
}
