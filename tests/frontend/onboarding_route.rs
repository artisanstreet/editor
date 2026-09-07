//! Exhaustive boundary coverage for the pure onboarding redirect policy.
//!
//! The implementation is loaded directly so this focused, dependency-free
//! harness does not require the VP-owned `lib.rs` or build-file registration.
//! The cross-product table covers every completion `Option` state, both
//! defaults-availability booleans, and each pathname partition at the exact
//! exclusion boundaries.

#[path = "../../modules/frontend/src/onboarding_route.rs"]
mod onboarding_route;

use onboarding_route::{OnboardingRouteInput, should_redirect_to_onboarding};

#[derive(Clone, Copy, Debug)]
struct PathCase {
    label: &'static str,
    pathname: &'static str,
    redirects_when_ready: bool,
}

const COMPLETION_STATES: [(&str, Option<bool>); 3] = [
    ("unavailable", None),
    ("incomplete", Some(false)),
    ("complete", Some(true)),
];

const DEFAULTS_STATES: [(&str, bool); 2] = [("unavailable", false), ("available", true)];

const PATH_CASES: [PathCase; 19] = [
    PathCase {
        label: "empty pathname",
        pathname: "",
        redirects_when_ready: true,
    },
    PathCase {
        label: "root pathname",
        pathname: "/",
        redirects_when_ready: true,
    },
    PathCase {
        label: "ordinary route",
        pathname: "/settings",
        redirects_when_ready: true,
    },
    PathCase {
        label: "exact onboarding route",
        pathname: "/onboarding",
        redirects_when_ready: false,
    },
    PathCase {
        label: "onboarding trailing slash",
        pathname: "/onboarding/",
        redirects_when_ready: true,
    },
    PathCase {
        label: "nested onboarding route",
        pathname: "/onboarding/step",
        redirects_when_ready: true,
    },
    PathCase {
        label: "onboarding suffix",
        pathname: "/onboarding?from=home",
        redirects_when_ready: true,
    },
    PathCase {
        label: "exact debug route",
        pathname: "/debug",
        redirects_when_ready: false,
    },
    PathCase {
        label: "debug nested boundary",
        pathname: "/debug/",
        redirects_when_ready: false,
    },
    PathCase {
        label: "debug nested route",
        pathname: "/debug/logs",
        redirects_when_ready: false,
    },
    PathCase {
        label: "deep debug nested route",
        pathname: "/debug/trace/events",
        redirects_when_ready: false,
    },
    PathCase {
        label: "debug double slash nested route",
        pathname: "/debug//nested",
        redirects_when_ready: false,
    },
    PathCase {
        label: "debug suffix",
        pathname: "/debug?panel=1",
        redirects_when_ready: true,
    },
    PathCase {
        label: "debugger lookalike",
        pathname: "/debugger",
        redirects_when_ready: true,
    },
    PathCase {
        label: "debugging lookalike",
        pathname: "/debugging",
        redirects_when_ready: true,
    },
    PathCase {
        label: "relative debug lookalike",
        pathname: "debug",
        redirects_when_ready: true,
    },
    PathCase {
        label: "case-shifted debug lookalike",
        pathname: "/DEBUG",
        redirects_when_ready: true,
    },
    PathCase {
        label: "backslash debug lookalike",
        pathname: "/debug\\logs",
        redirects_when_ready: true,
    },
    PathCase {
        label: "leading-space debug lookalike",
        pathname: " /debug",
        redirects_when_ready: true,
    },
];

#[test]
fn all_input_states_and_path_partitions_match_the_typescript_predicate() {
    for (completion_label, completed) in COMPLETION_STATES {
        for (defaults_label, defaults_available) in DEFAULTS_STATES {
            for path in PATH_CASES {
                let expected =
                    defaults_available && completed != Some(true) && path.redirects_when_ready;
                let input = OnboardingRouteInput::new(completed, defaults_available, path.pathname);

                assert_eq!(
                    should_redirect_to_onboarding(input),
                    expected,
                    "completion={completion_label}, defaults={defaults_label}, path={} ({})",
                    path.pathname,
                    path.label,
                );
            }
        }
    }
}

#[test]
fn path_exclusions_do_not_normalize_or_expand_exact_matches() {
    let eligible_paths = [
        "/onboarding/",
        "/onboarding/step",
        "/onboarding?from=home",
        "/debug?panel=1",
        "/debugger",
        "/debugging",
        "/debug\\logs",
    ];
    for pathname in eligible_paths {
        assert!(should_redirect_to_onboarding(OnboardingRouteInput::new(
            None, true, pathname,
        )));
    }

    let excluded_paths = ["/debug", "/debug/", "/debug/logs", "/debug//nested"];
    for pathname in excluded_paths {
        assert!(!should_redirect_to_onboarding(OnboardingRouteInput::new(
            Some(false),
            true,
            pathname,
        )));
    }
}
