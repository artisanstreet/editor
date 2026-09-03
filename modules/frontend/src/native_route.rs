//! Native route model: explicit screen identity with bounded history.
//!
//! GPUI has no router; the old `SvelteKit` file-based routes
//! (`modules/frontend/src/routes/`) become this enum. The root view
//! matches on the current route to select its screen, and navigation
//! pushes the previous route onto a bounded history stack. History is
//! capped because GPUI re-renders on every notify and retained views
//! would otherwise leak windows.

use artisan_domain::{ProjectId, ThreadId};

/// Maximum retained history entries.
pub const MAX_ROUTE_HISTORY: usize = 50;

/// Application screen identity, mirroring the old route tree.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeRoute {
    /// New-thread surface (`/`), optionally scoped to a project
    /// (`/t/[workspace]`).
    NewThread { project: Option<ProjectId> },
    /// Open thread transcript (`/t/[workspace]/[thread]`).
    Thread {
        project: ProjectId,
        thread: ThreadId,
    },
    /// Editor surface (`/e/[workspace]/[thread]`).
    Editor {
        project: ProjectId,
        thread: ThreadId,
    },
    /// Settings section (`/settings/...`).
    Settings(SettingsRoute),
    /// Onboarding wizard (`/onboarding`).
    Onboarding,
}

impl Default for NativeRoute {
    fn default() -> Self {
        Self::NewThread { project: None }
    }
}

/// Settings subsection, mirroring `/settings/*` routes.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum SettingsRoute {
    /// `/settings/models` (also the `/settings` alias target).
    #[default]
    Models,
    /// `/settings/appearance`.
    Appearance,
    /// `/settings/engines`.
    Engines,
    /// `/settings/notifications`.
    Notifications,
    /// `/settings/privacy`.
    Privacy,
    /// `/settings/threads`.
    Threads,
}

impl NativeRoute {
    /// Returns the stable debug-selector suffix for this route.
    #[must_use]
    pub fn selector_suffix(&self) -> String {
        match self {
            Self::NewThread { project } => {
                if project.is_some() {
                    "route-new-thread-project".to_owned()
                } else {
                    "route-new-thread".to_owned()
                }
            }
            Self::Thread { .. } => "route-thread".to_owned(),
            Self::Editor { .. } => "route-editor".to_owned(),
            Self::Settings(section) => format!("route-settings-{}", section.as_str()),
            Self::Onboarding => "route-onboarding".to_owned(),
        }
    }
}

impl SettingsRoute {
    /// Returns the URL-style slug for this section.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Models => "models",
            Self::Appearance => "appearance",
            Self::Engines => "engines",
            Self::Notifications => "notifications",
            Self::Privacy => "privacy",
            Self::Threads => "threads",
        }
    }
}

/// Bounded navigation history over [`NativeRoute`].
#[derive(Clone, Debug, Default)]
pub struct RouteHistory {
    current: NativeRoute,
    stack: Vec<NativeRoute>,
}

impl RouteHistory {
    /// Creates history rooted at the default route.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the current route.
    #[must_use]
    pub const fn current(&self) -> &NativeRoute {
        &self.current
    }

    /// Returns the number of retained history entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.stack.len()
    }

    /// Returns whether any back navigation is available.
    #[must_use]
    pub fn can_go_back(&self) -> bool {
        !self.is_empty()
    }

    /// Returns whether history is empty.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    /// Navigates to `route`, retaining the previous route. Navigating to
    /// the route already current is a no-op and retains nothing.
    pub fn navigate(&mut self, route: NativeRoute) {
        if route == self.current {
            return;
        }
        let previous = std::mem::replace(&mut self.current, route);
        self.stack.push(previous);
        if self.stack.len() > MAX_ROUTE_HISTORY {
            self.stack.remove(0);
        }
    }

    /// Returns to the previous route, or `false` when history is empty.
    pub fn go_back(&mut self) -> bool {
        if let Some(previous) = self.stack.pop() {
            self.current = previous;
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_route_is_unscoped_new_thread() {
        let history = RouteHistory::new();
        assert_eq!(history.current(), &NativeRoute::default());
        assert_eq!(
            history.current().selector_suffix(),
            "route-new-thread".to_owned()
        );
        assert!(!history.can_go_back());
        assert_eq!(history.len(), 0);
    }

    #[test]
    fn navigate_retains_previous_and_back_restores() {
        let mut history = RouteHistory::new();
        history.navigate(NativeRoute::Settings(SettingsRoute::Appearance));
        assert!(history.can_go_back());
        assert_eq!(history.len(), 1);
        assert_eq!(
            history.current().selector_suffix(),
            "route-settings-appearance".to_owned()
        );
        assert!(history.go_back());
        assert_eq!(history.current(), &NativeRoute::default());
        assert!(!history.can_go_back());
    }

    #[test]
    fn back_on_empty_history_is_stable_noop() {
        let mut history = RouteHistory::new();
        assert!(!history.go_back());
        assert_eq!(history.current(), &NativeRoute::default());
    }

    #[test]
    fn navigate_to_current_route_retains_nothing() {
        let mut history = RouteHistory::new();
        history.navigate(NativeRoute::default());
        assert_eq!(history.len(), 0);
        assert!(!history.can_go_back());
    }

    #[test]
    fn history_is_bounded_at_max() {
        let mut history = RouteHistory::new();
        for section in [
            SettingsRoute::Models,
            SettingsRoute::Appearance,
            SettingsRoute::Engines,
            SettingsRoute::Notifications,
            SettingsRoute::Privacy,
            SettingsRoute::Threads,
        ] {
            history.navigate(NativeRoute::Settings(section));
        }
        // Push past the cap with distinct thread routes is unnecessary;
        // alternating two routes exercises the bound directly.
        for _ in 0..MAX_ROUTE_HISTORY {
            history.navigate(NativeRoute::Onboarding);
            history.navigate(NativeRoute::default());
        }
        assert_eq!(history.len(), MAX_ROUTE_HISTORY);
        // Oldest entries were evicted; newest back-step still works.
        assert!(history.go_back());
        assert_eq!(history.current(), &NativeRoute::Onboarding);
    }

    #[test]
    fn settings_slugs_cover_all_sections() {
        assert_eq!(SettingsRoute::Models.as_str(), "models");
        assert_eq!(SettingsRoute::Appearance.as_str(), "appearance");
        assert_eq!(SettingsRoute::Engines.as_str(), "engines");
        assert_eq!(SettingsRoute::Notifications.as_str(), "notifications");
        assert_eq!(SettingsRoute::Privacy.as_str(), "privacy");
        assert_eq!(SettingsRoute::Threads.as_str(), "threads");
        assert_eq!(SettingsRoute::default(), SettingsRoute::Models);
    }
}
