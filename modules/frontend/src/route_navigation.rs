//! Typed, side-effect-free route-navigation values.
//!
//! This is the native value boundary for
//! `modules/frontend/src/lib/browser/route-navigation.ts`. It carries the
//! caller's path-or-URL text, the three optional navigation fields, and a
//! typed failure around an adapter seam. It deliberately does not parse or
//! normalize targets, choose routes, perform navigation, or model history.

#![allow(clippy::module_name_repetitions)]

/// The caller-selected representation accepted by the legacy navigation API.
///
/// Both variants remain text-backed. [`Self::Url`] is not parsed or validated;
/// its string is only a caller-provided representation that an eventual host
/// adapter may pass to its platform API.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum RouteNavigationTarget {
    /// A path or other string target supplied by the caller.
    Text(String),
    /// A URL supplied by the caller, retained as text without URL parsing.
    Url(String),
}

impl RouteNavigationTarget {
    /// Creates a path target while preserving every character of `value`.
    #[must_use]
    pub fn path(value: impl Into<String>) -> Self {
        Self::Text(value.into())
    }

    /// Alias for [`Self::path`] using the target's text-oriented name.
    #[must_use]
    pub fn text(value: impl Into<String>) -> Self {
        Self::path(value)
    }

    /// Creates a URL target without parsing or normalizing `value`.
    #[must_use]
    pub fn url(value: impl Into<String>) -> Self {
        Self::Url(value.into())
    }

    /// Returns the exact text carried by this target.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Text(value) | Self::Url(value) => value,
        }
    }

    /// Returns whether the caller selected the URL representation.
    #[must_use]
    pub const fn is_url(&self) -> bool {
        matches!(self, Self::Url(_))
    }

    /// Returns the target's owned text without changing it.
    #[must_use]
    pub fn into_text(self) -> String {
        match self {
            Self::Text(value) | Self::Url(value) => value,
        }
    }
}

impl From<&str> for RouteNavigationTarget {
    fn from(value: &str) -> Self {
        Self::path(value)
    }
}

impl From<String> for RouteNavigationTarget {
    fn from(value: String) -> Self {
        Self::path(value)
    }
}

/// The three independent optional fields passed to route navigation.
///
/// Each field retains the distinction made by the TypeScript interface:
/// `None` means the property was omitted, while `Some(false)` is an explicit
/// false and `Some(true)` is an explicit true. This value has no default so a
/// caller cannot accidentally turn omission into a platform-specific value.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RouteNavigationOptions {
    /// The optional `keepFocus` field.
    pub keep_focus: Option<bool>,
    /// The optional `noScroll` field.
    pub no_scroll: Option<bool>,
    /// The optional `replaceState` field.
    pub replace_state: Option<bool>,
}

impl RouteNavigationOptions {
    /// Creates one exact combination of the three optional fields.
    #[must_use]
    pub const fn new(
        keep_focus: Option<bool>,
        no_scroll: Option<bool>,
        replace_state: Option<bool>,
    ) -> Self {
        Self {
            keep_focus,
            no_scroll,
            replace_state,
        }
    }

    /// Represents a navigation call whose options object was omitted.
    #[must_use]
    pub const fn omitted() -> Self {
        Self::new(None, None, None)
    }

    /// Returns a copy with only `keep_focus` changed.
    #[must_use]
    pub const fn with_keep_focus(mut self, value: Option<bool>) -> Self {
        self.keep_focus = value;
        self
    }

    /// Returns a copy with only `no_scroll` changed.
    #[must_use]
    pub const fn with_no_scroll(mut self, value: Option<bool>) -> Self {
        self.no_scroll = value;
        self
    }

    /// Returns a copy with only `replace_state` changed.
    #[must_use]
    pub const fn with_replace_state(mut self, value: Option<bool>) -> Self {
        self.replace_state = value;
        self
    }
}

/// One typed request for a route-navigation capability.
///
/// The target and options remain values supplied by the caller. Constructing
/// an intent has no route lookup, URL parsing, validation, history operation,
/// or navigation side effect.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RouteNavigationIntent {
    /// The exact path or URL representation selected by the caller.
    pub target: RouteNavigationTarget,
    /// The three optional fields to pass through unchanged.
    pub options: RouteNavigationOptions,
}

impl RouteNavigationIntent {
    /// Creates an intent from an already-selected target and options.
    #[must_use]
    pub fn new(target: impl Into<RouteNavigationTarget>, options: RouteNavigationOptions) -> Self {
        Self {
            target: target.into(),
            options,
        }
    }

    /// Creates an intent for the legacy call form with no options object.
    #[must_use]
    pub fn without_options(target: impl Into<RouteNavigationTarget>) -> Self {
        Self::new(target, RouteNavigationOptions::omitted())
    }

    /// Creates a path intent with the supplied optional fields.
    #[must_use]
    pub fn from_path(path: impl Into<String>, options: RouteNavigationOptions) -> Self {
        Self::new(RouteNavigationTarget::path(path), options)
    }

    /// Creates a URL intent with the supplied optional fields, without parsing
    /// or normalizing the URL text.
    #[must_use]
    pub fn from_url(url: impl Into<String>, options: RouteNavigationOptions) -> Self {
        Self::new(RouteNavigationTarget::url(url), options)
    }

    /// Borrows the caller-selected target representation.
    #[must_use]
    pub const fn target(&self) -> &RouteNavigationTarget {
        &self.target
    }

    /// Borrows the exact path-or-URL text carried by the intent.
    #[must_use]
    pub fn path(&self) -> &str {
        self.target.as_str()
    }

    /// Returns the complete optional field set unchanged.
    #[must_use]
    pub const fn options(&self) -> RouteNavigationOptions {
        self.options
    }

    /// Returns the selected target, retaining its exact representation.
    #[must_use]
    pub fn into_target(self) -> RouteNavigationTarget {
        self.target
    }
}

/// A route-navigation failure that retains the adapter's typed cause.
///
/// The generic cause is intentional: the platform-independent boundary does
/// not know whether a future adapter reports a browser, desktop, or test
/// error. No cause is converted to a string or discarded here.
#[must_use = "a route-navigation failure should be handled or returned"]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RouteNavigationFailure<Cause> {
    /// The exact typed failure supplied by the adapter.
    pub cause: Cause,
}

impl<Cause> RouteNavigationFailure<Cause> {
    /// Wraps an adapter cause without erasing its type or value.
    #[must_use = "a route-navigation failure should be handled or returned"]
    pub const fn new(cause: Cause) -> Self {
        Self { cause }
    }

    /// Borrows the exact adapter cause.
    #[must_use]
    pub const fn cause(&self) -> &Cause {
        &self.cause
    }

    /// Returns the exact adapter cause.
    #[must_use]
    pub fn into_cause(self) -> Cause {
        self.cause
    }
}

/// Capability implemented by an eventual route-navigation adapter.
///
/// The capability receives only a typed value and does not prescribe how a
/// host performs navigation. An implementation chooses its own error type;
/// the required result wrapper keeps that error visible to the caller.
pub trait RouteNavigation {
    /// The adapter-specific failure preserved by [`RouteNavigationFailure`].
    type Error;

    /// Attempts to hand one intent to this adapter.
    ///
    /// This declaration is the platform seam only. It performs no operation
    /// until a caller supplies an implementation.
    ///
    /// # Errors
    ///
    /// Returns the adapter's exact typed failure inside
    /// [`RouteNavigationFailure`].
    #[must_use = "route-navigation results must be handled"]
    fn navigate(
        &self,
        intent: &RouteNavigationIntent,
    ) -> Result<(), RouteNavigationFailure<Self::Error>>;
}
