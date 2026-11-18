use super::{Effect, Event, Machine};
use artisan_api::SignInState;

#[test]
fn a_stale_signed_out_probe_does_not_interrupt_login_and_cancel_stops_before_checking() {
    let mut machine = Machine::default();
    let mut effects = Vec::new();
    machine.handle_with_context(&Event::Login, &mut effects);
    assert_eq!(std::mem::take(&mut effects), [Effect::StartLogin]);
    machine.handle_with_context(
        &Event::Prompt {
            url: Some("https://auth.example".into()),
            code: None,
        },
        &mut effects,
    );
    machine.handle_with_context(&Event::Checked(SignInState::SignedOut), &mut effects);
    machine.handle_with_context(&Event::Login, &mut effects);
    assert!(effects.is_empty());
    assert!(
        matches!(machine.state(), super::State::LoggingIn { url: Some(url), code: None }
        if url == "https://auth.example")
    );
    machine.handle_with_context(&Event::Cancel, &mut effects);
    assert_eq!(effects, [Effect::StopLogin, Effect::Check]);
    assert!(matches!(machine.state(), super::State::Unknown {}));
}

#[test]
fn a_successful_probe_stops_an_active_login_before_publishing_the_account() {
    let mut machine = Machine::default();
    let mut effects = Vec::new();
    machine.handle_with_context(&Event::Login, &mut effects);
    effects.clear();
    machine.handle_with_context(
        &Event::Checked(SignInState::SignedIn {
            account: Some("alice".into()),
        }),
        &mut effects,
    );
    assert_eq!(effects, [Effect::StopLogin]);
    assert!(
        matches!(machine.state(), super::State::SignedIn { account: Some(account) } if account == "alice")
    );
}
