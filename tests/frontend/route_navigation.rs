//! Dependency-free coverage for the native route-navigation value boundary.
//!
//! The implementation is included directly so this packet can run with plain
//! Rust 1.98 without changing frontend module, Cargo, or Bazel registration.

#[path = "../../modules/frontend/src/route_navigation.rs"]
mod route_navigation;

use route_navigation::{
    RouteNavigation, RouteNavigationFailure, RouteNavigationIntent, RouteNavigationOptions,
    RouteNavigationTarget,
};

const OPTION_STATES: [Option<bool>; 3] = [None, Some(false), Some(true)];

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct AdapterFailure {
    code: &'static str,
}

struct TestAdapter {
    expected_intent: RouteNavigationIntent,
    failure: Option<AdapterFailure>,
}

impl RouteNavigation for TestAdapter {
    type Error = AdapterFailure;

    fn navigate(
        &self,
        intent: &RouteNavigationIntent,
    ) -> Result<(), RouteNavigationFailure<Self::Error>> {
        assert_eq!(intent, &self.expected_intent);
        match &self.failure {
            Some(failure) => Err(RouteNavigationFailure::new(failure.clone())),
            None => Ok(()),
        }
    }
}

#[test]
fn every_optional_field_combination_is_preserved_independently() {
    let target = RouteNavigationTarget::path("/workspace/thread?file=main.rs#line-7");
    let mut combinations = 0;

    for keep_focus in OPTION_STATES {
        for no_scroll in OPTION_STATES {
            for replace_state in OPTION_STATES {
                let options = RouteNavigationOptions::new(keep_focus, no_scroll, replace_state);
                let intent = RouteNavigationIntent::new(target.clone(), options);

                assert_eq!(intent.options.keep_focus, keep_focus);
                assert_eq!(intent.options.no_scroll, no_scroll);
                assert_eq!(intent.options.replace_state, replace_state);
                assert_eq!(intent.options(), options);
                combinations += 1;
            }
        }
    }

    assert_eq!(combinations, 27);
}

#[test]
fn omitted_and_explicit_false_are_distinct_for_each_option() {
    let omitted = RouteNavigationOptions::omitted();
    let cases = [
        (
            omitted.with_keep_focus(Some(false)),
            RouteNavigationOptions::new(Some(false), None, None),
        ),
        (
            omitted.with_no_scroll(Some(false)),
            RouteNavigationOptions::new(None, Some(false), None),
        ),
        (
            omitted.with_replace_state(Some(false)),
            RouteNavigationOptions::new(None, None, Some(false)),
        ),
    ];

    for (explicit_false, expected) in cases {
        assert_eq!(explicit_false, expected);
        assert_ne!(explicit_false, omitted);
    }
}

#[test]
fn path_and_url_targets_preserve_exact_caller_text() {
    let raw_values = [
        "",
        "  /raw route/with spaces?value=%2F#fragment\n",
        "https://example.invalid/a path?q=%2F#fragment",
        "opaque://host/not-a-validated-url?x=1",
        "Grüße, мир, こんにちは, 👋🏽\u{200b}",
    ];

    for raw in raw_values {
        let path = RouteNavigationTarget::path(raw);
        let url = RouteNavigationTarget::url(raw);

        assert_eq!(path.as_str(), raw);
        assert_eq!(url.as_str(), raw);
        assert_eq!(path.clone().into_text(), raw);
        assert_eq!(url.clone().into_text(), raw);
        assert!(!path.is_url());
        assert!(url.is_url());

        let path_intent = RouteNavigationIntent::from_path(raw, RouteNavigationOptions::omitted());
        let url_intent = RouteNavigationIntent::from_url(raw, RouteNavigationOptions::omitted());
        assert_eq!(path_intent.path(), raw);
        assert_eq!(url_intent.path(), raw);
        assert_eq!(path_intent.target(), &path);
        assert_eq!(url_intent.target(), &url);
    }
}

#[test]
fn a_string_target_is_text_backed_without_url_parsing() {
    let raw = "not a URL, and not a route either";
    let target = RouteNavigationTarget::text(raw);
    let intent = RouteNavigationIntent::without_options(raw.to_owned());

    assert_eq!(target, RouteNavigationTarget::Text(raw.to_owned()));
    assert_eq!(intent.target(), &target);
    assert_eq!(intent.path(), raw);
    assert_eq!(intent.options(), RouteNavigationOptions::omitted());
    assert_eq!(intent.clone().into_target(), target);
}

#[test]
fn capability_success_receives_the_exact_typed_intent() {
    let intent = RouteNavigationIntent::new(
        RouteNavigationTarget::url("https://example.invalid/a path?x=%2F#end"),
        RouteNavigationOptions::new(Some(false), None, Some(true)),
    );
    let adapter = TestAdapter {
        expected_intent: intent.clone(),
        failure: None,
    };

    assert_eq!(adapter.navigate(&intent), Ok(()));
}

#[test]
fn capability_failure_retains_the_adapter_cause_without_erasure() {
    let intent = RouteNavigationIntent::without_options("/settings");
    let cause = AdapterFailure {
        code: "host-rejected",
    };
    let adapter = TestAdapter {
        expected_intent: intent.clone(),
        failure: Some(cause.clone()),
    };

    let failure = adapter
        .navigate(&intent)
        .expect_err("the adapter's typed failure must reach the caller");
    assert_eq!(failure.cause(), &cause);
    assert_eq!(failure.cause.code, "host-rejected");
    assert_eq!(failure.into_cause(), cause);
}
