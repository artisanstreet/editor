//! Dependency-free coverage for the one-attempt route-navigation adapter.
//!
//! The existing route-navigation values are loaded as a local sibling module
//! because this focused harness intentionally does not change shared module,
//! Cargo, or Bazel registration.

#[path = "../../modules/frontend/src/route_navigation.rs"]
mod route_navigation;
#[path = "../../modules/frontend/src/route_navigation_adapter.rs"]
mod route_navigation_adapter;

use std::cell::{Cell, RefCell};

use route_navigation::{
    RouteNavigation, RouteNavigationFailure, RouteNavigationIntent, RouteNavigationOptions,
    RouteNavigationTarget,
};
use route_navigation_adapter::{RouteNavigationAdapter, RouteNavigationHost};

const OPTION_STATES: [Option<bool>; 3] = [None, Some(false), Some(true)];

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct HostFailure {
    code: &'static str,
    detail: &'static str,
}

struct CustomHost {
    calls: Cell<usize>,
    expected: RouteNavigationIntent,
}

impl RouteNavigationHost for CustomHost {
    type Error = HostFailure;

    fn navigate(&self, intent: &RouteNavigationIntent) -> Result<(), Self::Error> {
        self.calls.set(self.calls.get() + 1);
        assert_eq!(intent, &self.expected);
        Ok(())
    }
}

#[test]
fn existing_intent_helpers_keep_omission_distinct_from_explicit_values() {
    let options = RouteNavigationOptions::omitted()
        .with_keep_focus(Some(false))
        .with_no_scroll(Some(true))
        .with_replace_state(None);
    let target = RouteNavigationTarget::text("");
    let intent = RouteNavigationIntent::new(target.clone(), options);

    assert_eq!(target.clone().into_text(), "");
    assert_eq!(intent.options(), options);
    assert_eq!(intent.clone().into_target(), target);
    assert_ne!(options, RouteNavigationOptions::omitted());
}

#[test]
fn successful_host_callback_returns_unit_and_runs_once() {
    let calls = Cell::new(0);
    let intent = RouteNavigationIntent::from_path(
        "/workspace/thread",
        RouteNavigationOptions::new(Some(true), Some(false), None),
    );
    let adapter = RouteNavigationAdapter::new(|received: &RouteNavigationIntent| {
        calls.set(calls.get() + 1);
        assert_eq!(received, &intent);
        Ok::<(), HostFailure>(())
    });

    assert_eq!(adapter.navigate(&intent), Ok(()));
    assert_eq!(calls.get(), 1);
}

#[test]
fn custom_host_trait_is_the_same_narrow_boundary() {
    let intent = RouteNavigationIntent::without_options("/settings");
    let host = CustomHost {
        calls: Cell::new(0),
        expected: intent.clone(),
    };
    let adapter = RouteNavigationAdapter::new(host);

    assert_eq!(adapter.execute(&intent), Ok(()));
    assert_eq!(adapter.into_host().calls.get(), 1);
}

#[test]
fn every_optional_field_combination_reaches_the_host_unchanged() {
    let calls = Cell::new(0);
    let observed = RefCell::new(None);
    let adapter = RouteNavigationAdapter::new(|intent: &RouteNavigationIntent| {
        calls.set(calls.get() + 1);
        observed.replace(Some(intent.clone()));
        Ok::<(), HostFailure>(())
    });
    let target = RouteNavigationTarget::path("/workspace/thread?file=main.rs#line-7");
    let mut combinations = 0;

    for keep_focus in OPTION_STATES {
        for no_scroll in OPTION_STATES {
            for replace_state in OPTION_STATES {
                let options = RouteNavigationOptions::new(keep_focus, no_scroll, replace_state);
                let intent = RouteNavigationIntent::new(target.clone(), options);

                assert_eq!(adapter.navigate(&intent), Ok(()));
                assert_eq!(observed.borrow().as_ref(), Some(&intent));
                assert_eq!(intent.options.keep_focus, keep_focus);
                assert_eq!(intent.options.no_scroll, no_scroll);
                assert_eq!(intent.options.replace_state, replace_state);
                combinations += 1;
            }
        }
    }

    assert_eq!(combinations, 27);
    assert_eq!(calls.get(), 27);
}

#[test]
fn path_and_url_representations_keep_empty_and_unicode_text_exactly() {
    let observed = RefCell::new(None);
    let adapter = RouteNavigationAdapter::new(|intent: &RouteNavigationIntent| {
        observed.replace(Some(intent.clone()));
        Ok::<(), HostFailure>(())
    });
    let raw_values = [
        "",
        "  /raw route/with spaces?value=%2F#fragment\n",
        "https://example.invalid/a path?q=%2F#fragment",
        "opaque://host/not-a-validated-url?x=1",
        "Grüße, мир, こんにちは, 👋🏽\u{200b}",
    ];

    for raw in raw_values {
        let path_intent = RouteNavigationIntent::from_path(raw, RouteNavigationOptions::omitted());
        assert_eq!(adapter.navigate(&path_intent), Ok(()));
        let path_observed = observed.borrow();
        assert_eq!(path_observed.as_ref(), Some(&path_intent));
        assert_eq!(
            path_intent.target(),
            &RouteNavigationTarget::Text(raw.to_owned())
        );
        assert_eq!(path_intent.path(), raw);
        assert!(!path_intent.target().is_url());
        drop(path_observed);

        let url_intent = RouteNavigationIntent::from_url(raw, RouteNavigationOptions::omitted());
        assert_eq!(adapter.navigate(&url_intent), Ok(()));
        let url_observed = observed.borrow();
        assert_eq!(url_observed.as_ref(), Some(&url_intent));
        assert_eq!(
            url_intent.target(),
            &RouteNavigationTarget::Url(raw.to_owned())
        );
        assert_eq!(url_intent.path(), raw);
        assert!(url_intent.target().is_url());
    }
}

#[test]
fn host_failure_preserves_the_typed_cause_without_erasure() {
    let calls = Cell::new(0);
    let cause = HostFailure {
        code: "host-rejected",
        detail: "navigation is unavailable",
    };
    let intent = RouteNavigationIntent::without_options("/settings");
    let adapter = RouteNavigationAdapter::new(|_: &RouteNavigationIntent| {
        calls.set(calls.get() + 1);
        Err::<(), _>(cause.clone())
    });

    let failure = adapter
        .navigate(&intent)
        .expect_err("the host failure must reach the caller");
    assert_eq!(failure, RouteNavigationFailure::new(cause.clone()));
    assert_eq!(failure.cause(), &cause);
    assert_eq!(failure.cause.code, "host-rejected");
    assert_eq!(failure.into_cause(), cause);
    assert_eq!(calls.get(), 1);
}

#[test]
fn a_result_return_does_not_schedule_a_later_callback() {
    let calls = Cell::new(0);
    let returned = Cell::new(false);
    let adapter = RouteNavigationAdapter::new(|_: &RouteNavigationIntent| {
        calls.set(calls.get() + 1);
        assert!(!returned.get());
        Ok::<(), HostFailure>(())
    });
    let intent = RouteNavigationIntent::without_options("");

    let result = adapter.navigate(&intent);
    returned.set(true);
    assert_eq!(result, Ok(()));
    assert_eq!(calls.get(), 1);
    assert!(returned.get());
    assert_eq!(calls.get(), 1);
}

#[test]
#[should_panic(expected = "host panic must escape")]
fn host_panics_are_not_caught_or_converted() {
    let adapter =
        RouteNavigationAdapter::new(|_: &RouteNavigationIntent| -> Result<(), HostFailure> {
            panic!("host panic must escape");
        });

    let _ = adapter.navigate(&RouteNavigationIntent::without_options("/panic"));
}
