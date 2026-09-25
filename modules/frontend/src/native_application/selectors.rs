//! Debug selectors, visible labels, and retained-limit policy constants for
//! the native application shell.
//!
//! Extracted verbatim from `native_application.rs` during the phase-1 module
//! split; visibility was widened to `pub(super)` where the parent module reads
//! the constant.

use std::time::Duration;

/// The one shipping application title.
pub(crate) const WINDOW_TITLE: &str = "Artisan Editor";

/// OS window title: product, selected host, and, for every build that is
/// not a stable release, the channel and commit, so the taskbar and window
/// switcher can never show a dev or unstaged build as the real app.
pub(crate) fn window_title(host: &str) -> String {
    match artisan_build_info::BuildIdentity::current().title_marker() {
        Some(marker) => format!("{WINDOW_TITLE} — {host} — {marker}"),
        None => format!("{WINDOW_TITLE} — {host}"),
    }
}

/// Stable selector for the real application root.
pub(crate) const NATIVE_ROOT_SELECTOR: &str = "artisan-native-application";

/// Debug selector prefix for the desktop header's route title.
pub(crate) const TITLEBAR_ROUTE_TITLE_SELECTOR: &str = "artisan-desktop-route-title";

/// Stable selector for the titlebar workspace-header cluster.
pub(crate) const TITLEBAR_HEADER_SELECTOR: &str = "artisan-desktop-titlebar-header";

/// Stable selector for the repository host mark in the workspace header.
pub(crate) const TITLEBAR_REPOSITORY_MARK_SELECTOR: &str =
    "artisan-desktop-titlebar-repository-mark";

/// Stable selector prefix for the qualified repository label.
pub(crate) const TITLEBAR_REPOSITORY_LABEL_SELECTOR: &str =
    "artisan-desktop-titlebar-repository-label";

/// Stable selector for the project-folder fallback in the workspace header.
pub(crate) const TITLEBAR_PROJECT_FOLDER_SELECTOR: &str = "artisan-desktop-titlebar-project-folder";

/// Stable selector for the darker separator before the thread subject.
pub(crate) const TITLEBAR_THREAD_SEPARATOR_SELECTOR: &str =
    "artisan-desktop-titlebar-thread-separator";

/// Stable selector for the state panel.
pub(crate) const NATIVE_STATUS_SELECTOR: &str = "artisan-native-status";

/// Stable selector for the rail's add-project action.
#[cfg(test)]
pub(crate) const NATIVE_RAIL_ADD_PROJECT_SELECTOR: &str = "artisan-native-rail-add-project";

/// Accessible name retained by the rail's icon-only add-project action.
#[cfg(test)]
pub(crate) const NATIVE_RAIL_ADD_PROJECT_LABEL: &str = "Add project";

pub(super) const NATIVE_KEY_CONTEXT: &str = "artisan-native-application";
pub(super) const SURFACE_WIDTH: f32 = 1_024.0;
pub(super) const SURFACE_HEIGHT: f32 = 720.0;
pub(super) const POLL_INTERVAL: Duration = Duration::from_millis(16);
pub(super) const SIDEBAR_NEW_THREAD_HOVER_ID: &str = "new-thread";
pub(super) const SIDEBAR_MARKETPLACE_HOVER_ID: &str = "marketplace";
pub(super) const SIDEBAR_PROFILE_HOVER_ID: &str = "profile";

pub(super) const PROFILE_SETTINGS_HOVER_ID: &str = "profile-settings";
pub(super) const PROFILE_USAGE_HOVER_ID: &str = "profile-usage";
/// Breathing room kept between the profile panel top edge and the viewport.
pub(super) const PROFILE_MENU_VIEWPORT_MARGIN_PX: f32 = 8.0;
/// Vertical gap between the panel bottom and the profile trigger top,
/// matching the source content `sideOffset` and the anchored offset applied
/// when placing the panel.
pub(super) const PROFILE_MENU_ANCHOR_GAP_PX: f32 = 4.0;

pub(super) const MAX_RETAINED_SWITCH_REQUEST_IDS: usize = 8;
pub(super) const MAX_RETAINED_SWITCH_PATCH_IDS: usize = 256;
pub(super) const MAX_RETAINED_SWITCH_LISTINGS: usize = 8;
