//! Pure system-notification contract and settings policy.
//!
//! This is the dependency-free Rust counterpart of
//! `modules/frontend/src/lib/notifications/contract.ts` and the pure
//! settings decisions in `modules/frontend/src/lib/notifications/service.ts`.
//! It contains no presenter, browser, Effect, queue, timer, navigation, or
//! persistence behavior.

/// What the host will currently allow the renderer to post.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SystemNotificationPermission {
    /// The host does not expose notification support.
    Unsupported,
    /// The host has not received a permission answer yet.
    Default,
    /// The host has granted notification permission.
    Granted,
    /// The host has denied notification permission.
    Denied,
}

impl SystemNotificationPermission {
    /// Returns the exact permission value used by the TypeScript contract.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::Default => "default",
            Self::Granted => "granted",
            Self::Denied => "denied",
        }
    }
}

/// The durable category carried by a host notification.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SystemNotificationCategory {
    /// A pending approval interaction.
    Approval,
    /// A pending question interaction.
    Question,
    /// A successfully completed run.
    RunCompleted,
    /// A failed run.
    RunFailed,
}

impl SystemNotificationCategory {
    /// Returns the exact category value used by the TypeScript contract.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Approval => "approval",
            Self::Question => "question",
            Self::RunCompleted => "run_completed",
            Self::RunFailed => "run_failed",
        }
    }
}

/// One owned notification value passed to a host-facing adapter.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SystemNotification {
    /// The body presented by the host.
    pub body: String,
    /// The semantic notification category.
    pub category: SystemNotificationCategory,
    /// The durable identity of the notification.
    pub id: String,
    /// The route opened when the notification is activated.
    pub route_path: String,
    /// The title presented by the host.
    pub title: String,
}

impl SystemNotification {
    /// Creates an owned notification without changing any supplied field.
    #[must_use]
    pub fn new(
        body: impl Into<String>,
        category: SystemNotificationCategory,
        id: impl Into<String>,
        route_path: impl Into<String>,
        title: impl Into<String>,
    ) -> Self {
        Self {
            body: body.into(),
            category,
            id: id.into(),
            route_path: route_path.into(),
            title: title.into(),
        }
    }
}

/// The notification settings state needed by the settings surface.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SystemNotificationSettings {
    /// Whether the reader asked to receive notifications.
    pub enabled: bool,
    /// What the host currently allows.
    pub permission: SystemNotificationPermission,
}

impl SystemNotificationSettings {
    /// Creates settings from the reader's enabled choice and host permission.
    #[must_use]
    pub const fn new(enabled: bool, permission: SystemNotificationPermission) -> Self {
        Self {
            enabled,
            permission,
        }
    }
}

/// What stands between enabled settings and a notification arriving.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SystemNotificationGap {
    /// Notifications can arrive when enabled.
    None,
    /// The host denied the requested permission.
    Blocked,
    /// The host has not been prompted for permission.
    Unprompted,
    /// The host does not support notifications.
    Unsupported,
}

impl SystemNotificationGap {
    /// Returns the exact gap value used by the settings contract.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Blocked => "blocked",
            Self::Unprompted => "unprompted",
            Self::Unsupported => "unsupported",
        }
    }
}

/// Returns the exact settings gap precedence from `SystemNotificationGapFor`.
///
/// Unsupported permission wins regardless of the enabled switch. Otherwise,
/// disabled settings and granted permission both report no gap; an enabled
/// denied permission is blocked, and an enabled default permission is
/// unprompted.
#[must_use]
pub const fn system_notification_gap_for(
    settings: SystemNotificationSettings,
) -> SystemNotificationGap {
    if matches!(
        settings.permission,
        SystemNotificationPermission::Unsupported
    ) {
        return SystemNotificationGap::Unsupported;
    }
    if !settings.enabled || matches!(settings.permission, SystemNotificationPermission::Granted) {
        return SystemNotificationGap::None;
    }
    if matches!(settings.permission, SystemNotificationPermission::Denied) {
        SystemNotificationGap::Blocked
    } else {
        SystemNotificationGap::Unprompted
    }
}

/// Returns whether notifications are enabled and host permission is granted.
#[must_use]
pub const fn system_notifications_are_active(settings: SystemNotificationSettings) -> bool {
    settings.enabled && matches!(settings.permission, SystemNotificationPermission::Granted)
}
