//! One-attempt host adapter for native route-navigation intents.
//!
//! This is the dependency-free native counterpart of
//! `modules/frontend/src/lib/browser/route-navigation-live.ts`. It hands the
//! existing route-navigation intent to one host callback and reports the
//! callback's result. The adapter does not inspect or transform the target or
//! options, schedule more work, or catch a panic from the host.

#![allow(clippy::module_name_repetitions)]

use crate::route_navigation::{RouteNavigation, RouteNavigationFailure, RouteNavigationIntent};

/// The host operation used by [`RouteNavigationAdapter`].
pub trait RouteNavigationHost {
    /// The concrete failure produced by the host.
    type Error;

    /// Performs one host-side attempt for the supplied intent.
    ///
    /// The intent is borrowed so the adapter can forward the exact existing
    /// value without rebuilding its target or option fields. Implementations
    /// must keep any platform behavior behind this boundary.
    ///
    /// # Errors
    ///
    /// Returns the host-specific [`Self::Error`] produced by the one host
    /// attempt.
    fn navigate(&self, intent: &RouteNavigationIntent) -> Result<(), Self::Error>;
}

impl<Callback, Error> RouteNavigationHost for Callback
where
    Callback: Fn(&RouteNavigationIntent) -> Result<(), Error>,
{
    type Error = Error;

    fn navigate(&self, intent: &RouteNavigationIntent) -> Result<(), Self::Error> {
        self(intent)
    }
}

/// Executes one route-navigation intent through a host callback.
///
/// The adapter is deliberately synchronous and stateless with respect to
/// navigation. Each call to [`RouteNavigation::navigate`] invokes the host
/// exactly once. A successful host result remains `Ok(())`; a host failure is
/// wrapped in [`RouteNavigationFailure`] without converting its type or value.
/// Panics from the host are allowed to propagate unchanged.
pub struct RouteNavigationAdapter<Host> {
    host: Host,
}

impl<Host> RouteNavigationAdapter<Host> {
    /// Creates an adapter around one host operation.
    #[must_use]
    pub const fn new(host: Host) -> Self {
        Self { host }
    }

    /// Returns the host operation without invoking it.
    #[must_use]
    pub fn into_host(self) -> Host {
        self.host
    }
}

impl<Host> RouteNavigationAdapter<Host>
where
    Host: RouteNavigationHost,
{
    /// Executes one host attempt for `intent` and returns its typed outcome.
    #[must_use = "route-navigation results must be handled"]
    ///
    /// # Errors
    ///
    /// Returns [`RouteNavigationFailure`] containing the exact typed error
    /// returned by the host's one attempt.
    pub fn execute(
        &self,
        intent: &RouteNavigationIntent,
    ) -> Result<(), RouteNavigationFailure<Host::Error>> {
        self.host
            .navigate(intent)
            .map_err(RouteNavigationFailure::new)
    }
}

impl<Host> RouteNavigation for RouteNavigationAdapter<Host>
where
    Host: RouteNavigationHost,
{
    type Error = Host::Error;

    fn navigate(
        &self,
        intent: &RouteNavigationIntent,
    ) -> Result<(), RouteNavigationFailure<Self::Error>> {
        self.execute(intent)
    }
}
