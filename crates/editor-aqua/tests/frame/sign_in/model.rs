use editor_aqua::core::account::{AccountPhase, Approval, Factor, SecondFactor};
use editor_aqua::frame::sign_in::model::{
    EmailProblem, Feedback, Page, Provider, Step, View, checked_email, feedback_of, locked, page_of,
    view_of,
};

fn approval(totp_required: bool) -> Approval {
    Approval {
        request_id: "request".into(),
        request_secret: "secret".into(),
        email: "you@artisan.email".into(),
        totp_required,
    }
}

#[test]
fn emails_are_trimmed_and_checked_for_shape() {
    assert_eq!(checked_email("  you@artisan.email "), Ok("you@artisan.email"));
    assert_eq!(checked_email("   "), Err(EmailProblem::Missing));
    for malformed in ["you", "you@", "@artisan.email", "you@artisan", "you@artisan.", "a@b@c.d", "y ou@artisan.email"] {
        assert_eq!(checked_email(malformed), Err(EmailProblem::Malformed), "{malformed}");
    }
}

#[test]
fn second_factor_codes_split_into_authenticator_and_recovery() {
    assert_eq!(SecondFactor::parse(" 123456 ").map(|factor| factor.kind()), Some(Factor::Authenticator));
    assert_eq!(SecondFactor::parse("12345").map(|factor| factor.kind()), Some(Factor::Recovery));
    assert_eq!(SecondFactor::parse("abcd-efgh").map(|factor| factor.kind()), Some(Factor::Recovery));
    assert_eq!(SecondFactor::parse("  "), None);
}

#[test]
fn approval_without_two_step_finishes_on_the_email_page() {
    let view = view_of(
        &AccountPhase::EmailApproved {
            approval: approval(false),
            rejected: None,
        },
        None,
    );
    assert_eq!(view, View::Approved);
    assert_eq!(page_of(&view), Page::Email);
    assert!(locked(&view));
}

#[test]
fn a_rejected_code_returns_to_the_code_page_with_a_factor_specific_error() {
    let view = view_of(
        &AccountPhase::EmailApproved {
            approval: approval(true),
            rejected: Some(Factor::Recovery),
        },
        None,
    );
    assert_eq!(
        view,
        View::SecondFactor {
            step: Step::Rejected(Factor::Recovery)
        }
    );
    assert_eq!(page_of(&view), Page::Code);
    assert!(!locked(&view));
    assert!(matches!(feedback_of(&view), Feedback::Error(message) if message.contains("recovery code")));
}

#[test]
fn verifying_a_code_locks_the_code_page() {
    let view = view_of(
        &AccountPhase::Finalizing {
            approval: approval(true),
            factor: SecondFactor::parse("123456"),
        },
        None,
    );
    assert_eq!(view, View::SecondFactor { step: Step::Verifying });
    assert!(locked(&view));
    assert_eq!(feedback_of(&view), Feedback::None);
}

#[test]
fn the_signed_out_form_reports_the_email_problem() {
    let view = view_of(&AccountPhase::SignedOut, Some(EmailProblem::Missing));
    assert!(!locked(&view));
    assert!(matches!(feedback_of(&view), Feedback::Error(_)));
    assert_eq!(feedback_of(&view_of(&AccountPhase::SignedOut, None)), Feedback::None);
}

#[test]
fn waiting_on_the_browser_keeps_the_form_open_and_says_where_to_continue() {
    let view = view_of(
        &AccountPhase::Browser {
            provider: Provider::GitHub,
        },
        None,
    );
    assert_eq!(view, View::Browser { provider: Provider::GitHub });
    assert_eq!(page_of(&view), Page::Email);
    assert!(!locked(&view));
    assert!(matches!(feedback_of(&view), Feedback::Success(message) if message.contains("GitHub")));
}

#[test]
fn locked_sign_in_attempts_keep_cancellation_available() {
    use editor_aqua::frame::sign_in::model::cancellable;
    for view in [
        View::Sending,
        View::Waiting,
        View::Approved,
        View::Finishing,
        View::SecondFactor { step: Step::Verifying },
    ] {
        assert!(locked(&view));
        assert!(cancellable(&view), "{view:?}");
    }
    assert!(cancellable(&View::Browser { provider: Provider::Google }));
    assert!(!cancellable(&View::Form { problem: None }));
    assert!(!cancellable(&View::Restoring));
}
