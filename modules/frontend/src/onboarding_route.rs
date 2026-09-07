//! Pure route-selection policy for the onboarding redirect.
//!
//! This is the native equivalent of
//! `modules/frontend/src/lib/onboarding-route.ts`. It deliberately accepts a
//! borrowed pathname and performs no URL parsing, trimming, normalization, or
//! navigation side effect. Callers decide how to obtain the pathname and how
//! to act on the returned decision.

/// The route state consumed by [`should_redirect_to_onboarding`].
///
/// `completed` maps the TypeScript policy's `boolean | undefined` value:
/// `None` means that completion is unavailable, `Some(false)` means that
/// onboarding is incomplete, and `Some(true)` means that it is complete.
/// Missing completion is therefore treated like incomplete completion, while
/// only an explicit `true` suppresses the redirect.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OnboardingRouteInput<'a> {
    /// Whether onboarding has been completed, when that fact is available.
    pub completed: Option<bool>,
    /// Whether the session defaults needed by the application are available.
    pub defaults_available: bool,
    /// The already-selected pathname, preserved exactly for comparison.
    pub pathname: &'a str,
}

impl<'a> OnboardingRouteInput<'a> {
    /// Creates an onboarding redirect input without allocating or normalizing.
    #[must_use]
    pub const fn new(completed: Option<bool>, defaults_available: bool, pathname: &'a str) -> Self {
        Self {
            completed,
            defaults_available,
            pathname,
        }
    }
}

/// Returns whether the application should redirect to `/onboarding`.
///
/// This preserves the TypeScript predicate exactly:
///
/// - defaults must be available;
/// - an explicit completion value of `true` prevents the redirect;
/// - the exact `/onboarding` and `/debug` pathnames are excluded; and
/// - `/debug/` and every deeper pathname below it are excluded.
///
/// No other pathname is excluded. In particular, `/onboarding/`,
/// `/onboarding/...`, `/debugger`, and `/debug?query` remain eligible when the
/// other conditions pass. The input is borrowed only through its pathname, so
/// this decision is allocation-free and has no observable side effects.
#[must_use]
pub fn should_redirect_to_onboarding(input: OnboardingRouteInput<'_>) -> bool {
    input.defaults_available
        && input.completed != Some(true)
        && input.pathname != "/onboarding"
        && input.pathname != "/debug"
        && !input.pathname.starts_with("/debug/")
}
