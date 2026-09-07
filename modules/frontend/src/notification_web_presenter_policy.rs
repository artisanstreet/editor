//! Dependency-free state policy for the web notification presenter.
//!
//! The browser adapter owns the actual `Notification` API. This module only
//! normalizes observations and turns presenter events into deterministic host
//! actions. A generation-bearing handle authenticates callbacks from one host
//! notification instance, so a callback retained by a replaced notification
//! cannot remove its replacement.

#![allow(clippy::module_name_repetitions)]

use std::collections::BTreeMap;

pub use crate::notification_events::SystemNotification;

/// The semantic asset identifier used by the browser notification adapter.
pub const ARTISAN_NOTIFICATION_ICON: &str = "artisan.app-icon";

/// The normalized permission values exposed by the notification contract.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SystemNotificationPermission {
    /// The host does not expose a notification API.
    Unsupported,
    /// The host has not granted or denied notification permission.
    Default,
    /// The host granted notification permission.
    Granted,
    /// The host denied notification permission.
    Denied,
}

impl SystemNotificationPermission {
    /// Returns the exact lowercase literal used by the frontend contract.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::Default => "default",
            Self::Granted => "granted",
            Self::Denied => "denied",
        }
    }

    /// Returns whether this permission allows a notification post.
    #[must_use]
    pub const fn is_granted(self) -> bool {
        matches!(self, Self::Granted)
    }
}

/// Normalizes a permission string according to the host contract.
///
/// The browser returns `granted` or `denied` for the two explicit answers.
/// Any other readable host string, including an empty or future value, is
/// conservatively represented as `default`.
#[must_use]
pub fn normalize_permission(value: &str) -> SystemNotificationPermission {
    match value {
        "granted" => SystemNotificationPermission::Granted,
        "denied" => SystemNotificationPermission::Denied,
        _ => SystemNotificationPermission::Default,
    }
}

/// Converts a host API observation into the contract permission value.
///
/// A missing API is unsupported regardless of an accompanying stale value.
/// An API whose permission cannot be read is also treated as unsupported; it
/// must not accidentally enable posting.
#[must_use]
pub fn permission_for_api(
    api_available: bool,
    value: Option<&str>,
) -> SystemNotificationPermission {
    if !api_available {
        return SystemNotificationPermission::Unsupported;
    }

    match value {
        Some(value) => normalize_permission(value),
        None => SystemNotificationPermission::Unsupported,
    }
}

/// The outcome reported after a host permission request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionRequestOutcome<'a> {
    /// The host answered with its permission string.
    Answered(&'a str),
    /// The host request failed; retain the most recently readable permission.
    Failed,
    /// The request failed and the adapter supplied a fresh readable snapshot.
    ///
    /// This variant lets an adapter model the TypeScript fallback's second
    /// read in one transition without this policy calling a browser API.
    FailedWithReadable {
        /// Whether the host API was present for the fallback read.
        api_available: bool,
        /// The raw permission read from the host, when readable.
        value: Option<&'a str>,
    },
}

/// The opaque identity of one host notification instance.
///
/// The durable notification ID is reused for replacements. The generation is
/// unique for each accepted post, allowing late host callbacks to be rejected
/// after the durable ID has been reused.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct WebNotificationHandle {
    /// The durable notification identity.
    pub id: String,
    /// The instance generation allocated for one post attempt.
    pub generation: u64,
}

impl WebNotificationHandle {
    /// Creates a handle from an ID and an already allocated generation.
    #[must_use]
    pub fn new(id: impl Into<String>, generation: u64) -> Self {
        Self {
            id: id.into(),
            generation,
        }
    }

    /// Returns the durable ID authenticated by this handle.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the instance generation authenticated by this handle.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }
}

/// Alias for callers that describe a host notification identity as an
/// instance rather than a handle.
pub type WebNotificationInstance = WebNotificationHandle;

/// The exact payload sent to the host notification constructor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebNotificationPayload {
    /// The notification title.
    pub title: String,
    /// The notification body.
    pub body: String,
    /// The semantic Artisan icon asset identifier.
    pub icon: &'static str,
    /// The host replacement tag, equal to the durable notification ID.
    pub tag: String,
}

impl WebNotificationPayload {
    /// Builds a host payload without loading an asset or calling the browser.
    #[must_use]
    pub fn from_notification(notification: &SystemNotification) -> Self {
        Self {
            title: notification.title.clone(),
            body: notification.body.clone(),
            icon: ARTISAN_NOTIFICATION_ICON,
            tag: notification.id.clone(),
        }
    }
}

/// An input event or command for the presenter policy.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WebNotificationPresenterAction<'a> {
    /// Applies a permission observation from an available or missing API.
    ReadPermission {
        /// Whether the host notification API exists.
        api_available: bool,
        /// The raw host permission, if it could be read.
        value: Option<&'a str>,
    },
    /// Records that the host permission read failed.
    ReadPermissionFailed,
    /// Requests permission from an available host API.
    RequestPermission,
    /// Applies the result of a permission request.
    RequestResolved {
        /// The host request outcome.
        outcome: PermissionRequestOutcome<'a>,
    },
    /// Applies a failed request together with its fresh readable fallback.
    ///
    /// This is equivalent to `RequestResolved` with
    /// [`PermissionRequestOutcome::FailedWithReadable`], and is convenient at
    /// adapters that keep request failure and permission reads as separate
    /// events.
    RequestFailed {
        /// Whether the host notification API exists for the fallback read.
        api_available: bool,
        /// The raw permission read from the host, when it could be read.
        value: Option<&'a str>,
    },
    /// Attempts to show a system notification.
    Show {
        /// The notification to present.
        notification: SystemNotification,
    },
    /// Reports that the current post was successfully constructed and posted.
    PostSucceeded {
        /// The exact post instance acknowledged by the host adapter.
        handle: WebNotificationHandle,
    },
    /// Reports that host construction or posting failed.
    PostFailed {
        /// The exact post instance that failed.
        handle: WebNotificationHandle,
    },
    /// Reports that the host closed an exact notification instance.
    Closed {
        /// The callback's authenticated notification handle.
        handle: WebNotificationHandle,
    },
    /// Reports activation of an exact notification instance.
    Clicked {
        /// The callback's authenticated notification handle.
        handle: WebNotificationHandle,
    },
    /// Explicitly dismisses the current notification for a durable ID.
    Dismiss {
        /// The durable notification ID to dismiss.
        id: String,
    },
    /// Dismisses only the exact instance identified by a callback/timer handle.
    DismissHandle {
        /// The exact notification instance to dismiss.
        handle: WebNotificationHandle,
    },
}

/// A deterministic host effect produced by one presenter transition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WebNotificationPresenterHostAction {
    /// Requests the host to close one exact notification instance.
    Dismiss {
        /// The exact host notification instance to close.
        handle: WebNotificationHandle,
    },
    /// Constructs and posts a host notification with this payload.
    Post {
        /// The title, body, icon, and replacement tag for the host.
        payload: WebNotificationPayload,
        /// The handle the adapter must return with post/callback results.
        handle: WebNotificationHandle,
    },
    /// Activates the exact application notification associated with a click.
    Activate {
        /// The notification payload carried to the application layer.
        notification: SystemNotification,
    },
    /// Requests permission from the host notification API.
    RequestPermission,
}

/// Host effects produced by one presenter state transition.
#[must_use = "a presenter transition contains host actions to dispatch"]
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WebNotificationPresenterTransition {
    actions: Vec<WebNotificationPresenterHostAction>,
}

impl WebNotificationPresenterTransition {
    fn from_actions(actions: Vec<WebNotificationPresenterHostAction>) -> Self {
        Self { actions }
    }

    /// Returns host actions in their required dispatch order.
    #[must_use]
    pub fn actions(&self) -> &[WebNotificationPresenterHostAction] {
        &self.actions
    }

    /// Consumes the transition and returns its host actions.
    #[must_use]
    pub fn into_actions(self) -> Vec<WebNotificationPresenterHostAction> {
        self.actions
    }

    /// Returns whether the transition has no host effects.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingPost {
    handle: WebNotificationHandle,
    notification: SystemNotification,
}

/// Deterministic state for the web notification presenter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WebNotificationPresenterState {
    api_available: bool,
    permission: SystemNotificationPermission,
    readable_permission: SystemNotificationPermission,
    live: BTreeMap<String, SystemNotification>,
    live_generations: BTreeMap<String, u64>,
    pending_posts: BTreeMap<String, PendingPost>,
    next_generation: u64,
}

impl WebNotificationPresenterState {
    /// Creates presenter state from the current host API and raw permission.
    #[must_use]
    pub fn new(api_available: bool, raw_permission: Option<&str>) -> Self {
        let permission = permission_for_api(api_available, raw_permission);
        Self {
            api_available,
            permission,
            readable_permission: permission,
            live: BTreeMap::new(),
            live_generations: BTreeMap::new(),
            pending_posts: BTreeMap::new(),
            next_generation: 0,
        }
    }

    /// Returns whether the host notification API is available.
    #[must_use]
    pub const fn api_available(&self) -> bool {
        self.api_available
    }

    /// Returns the current normalized permission used for show decisions.
    #[must_use]
    pub const fn permission(&self) -> SystemNotificationPermission {
        self.permission
    }

    /// Returns the latest permission snapshot readable from the host.
    #[must_use]
    pub const fn readable_permission(&self) -> SystemNotificationPermission {
        self.readable_permission
    }

    /// Returns successfully posted notifications keyed by durable ID.
    ///
    /// Pending or failed posts are intentionally absent from this map.
    #[must_use]
    pub const fn live_notifications(&self) -> &BTreeMap<String, SystemNotification> {
        &self.live
    }

    /// Returns the current live handle for a durable ID, if any.
    #[must_use]
    pub fn live_handle(&self, id: &str) -> Option<WebNotificationHandle> {
        self.live_generations
            .get(id)
            .copied()
            .map(|generation| WebNotificationHandle::new(id, generation))
    }

    /// Returns whether a durable ID currently has a successfully posted entry.
    #[must_use]
    pub fn is_live(&self, id: &str) -> bool {
        self.live.contains_key(id)
    }

    /// Applies one presenter event and returns host effects in dispatch order.
    pub fn apply(
        &mut self,
        action: WebNotificationPresenterAction<'_>,
    ) -> WebNotificationPresenterTransition {
        match action {
            WebNotificationPresenterAction::ReadPermission {
                api_available,
                value,
            }
            | WebNotificationPresenterAction::RequestFailed {
                api_available,
                value,
            } => {
                self.apply_permission_read(api_available, value);
                WebNotificationPresenterTransition::default()
            }
            WebNotificationPresenterAction::ReadPermissionFailed => {
                self.apply_permission_read(false, None);
                WebNotificationPresenterTransition::default()
            }
            WebNotificationPresenterAction::RequestPermission => {
                if self.api_available {
                    WebNotificationPresenterTransition::from_actions(vec![
                        WebNotificationPresenterHostAction::RequestPermission,
                    ])
                } else {
                    self.apply_permission_read(false, None);
                    WebNotificationPresenterTransition::default()
                }
            }
            WebNotificationPresenterAction::RequestResolved { outcome } => {
                self.apply_request_outcome(outcome);
                WebNotificationPresenterTransition::default()
            }
            WebNotificationPresenterAction::Show { notification } => self.show(&notification),
            WebNotificationPresenterAction::PostSucceeded { handle } => {
                self.post_succeeded(&handle);
                WebNotificationPresenterTransition::default()
            }
            WebNotificationPresenterAction::PostFailed { handle } => {
                self.post_failed(&handle);
                WebNotificationPresenterTransition::default()
            }
            WebNotificationPresenterAction::Closed { handle } => {
                self.close_live(&handle);
                WebNotificationPresenterTransition::default()
            }
            WebNotificationPresenterAction::Clicked { handle } => self.click(&handle),
            WebNotificationPresenterAction::Dismiss { id } => self.dismiss_id(&id),
            WebNotificationPresenterAction::DismissHandle { handle } => {
                self.dismiss_handle(&handle)
            }
        }
    }

    fn apply_permission_read(&mut self, api_available: bool, value: Option<&str>) {
        let permission = permission_for_api(api_available, value);
        self.api_available = api_available;
        self.permission = permission;
        self.readable_permission = permission;
    }

    fn apply_request_outcome(&mut self, outcome: PermissionRequestOutcome<'_>) {
        match outcome {
            PermissionRequestOutcome::Answered(value) => {
                let permission = if self.api_available {
                    normalize_permission(value)
                } else {
                    SystemNotificationPermission::Unsupported
                };
                self.permission = permission;
                self.readable_permission = permission;
            }
            PermissionRequestOutcome::Failed => {
                self.permission = self.readable_permission;
            }
            PermissionRequestOutcome::FailedWithReadable {
                api_available,
                value,
            } => self.apply_permission_read(api_available, value),
        }
    }

    fn show(&mut self, notification: &SystemNotification) -> WebNotificationPresenterTransition {
        if !self.api_available || !self.permission.is_granted() {
            return WebNotificationPresenterTransition::default();
        }

        let id = notification.id.clone();
        let Some(handle) = self.allocate_handle(&id) else {
            return WebNotificationPresenterTransition::default();
        };
        let mut actions = Vec::new();

        if let Some(generation) = self.live_generations.remove(&id) {
            // The generation map is the authentication source. Removing the
            // payload as part of the same transition keeps both maps bounded
            // by the live durable IDs and preserves their lockstep invariant.
            let _ = self.live.remove(&id);
            actions.push(WebNotificationPresenterHostAction::Dismiss {
                handle: WebNotificationHandle::new(id.clone(), generation),
            });
        }

        // A pending post has never become live, so it receives no host close
        // action. Its handle is replaced and any late result is ignored.
        self.pending_posts.remove(&id);
        self.pending_posts.insert(
            id.clone(),
            PendingPost {
                handle: handle.clone(),
                notification: notification.clone(),
            },
        );
        actions.push(WebNotificationPresenterHostAction::Post {
            payload: WebNotificationPayload::from_notification(notification),
            handle,
        });
        WebNotificationPresenterTransition::from_actions(actions)
    }

    fn allocate_handle(&mut self, id: &str) -> Option<WebNotificationHandle> {
        let generation = self.next_generation.checked_add(1)?;
        self.next_generation = generation;
        Some(WebNotificationHandle::new(id, generation))
    }

    fn post_succeeded(&mut self, handle: &WebNotificationHandle) {
        let Some(pending) = self.pending_posts.get(handle.id()) else {
            return;
        };
        if pending.handle != *handle {
            return;
        }

        let Some(pending) = self.pending_posts.remove(handle.id()) else {
            return;
        };
        self.live.insert(handle.id.clone(), pending.notification);
        self.live_generations
            .insert(handle.id.clone(), handle.generation);
    }

    fn post_failed(&mut self, handle: &WebNotificationHandle) {
        let Some(pending) = self.pending_posts.get(handle.id()) else {
            return;
        };
        if pending.handle == *handle {
            self.pending_posts.remove(handle.id());
        }
    }

    fn close_live(&mut self, handle: &WebNotificationHandle) {
        if self.live_matches(handle) {
            self.remove_live(handle.id());
        }
    }

    fn click(&mut self, handle: &WebNotificationHandle) -> WebNotificationPresenterTransition {
        let Some(notification) = self.take_live(handle) else {
            return WebNotificationPresenterTransition::default();
        };

        WebNotificationPresenterTransition::from_actions(vec![
            WebNotificationPresenterHostAction::Dismiss {
                handle: handle.clone(),
            },
            WebNotificationPresenterHostAction::Activate { notification },
        ])
    }

    fn dismiss_id(&mut self, id: &str) -> WebNotificationPresenterTransition {
        self.pending_posts.remove(id);
        let Some(handle) = self.live_handle(id) else {
            return WebNotificationPresenterTransition::default();
        };
        self.remove_live(id);
        WebNotificationPresenterTransition::from_actions(vec![
            WebNotificationPresenterHostAction::Dismiss { handle },
        ])
    }

    fn dismiss_handle(
        &mut self,
        handle: &WebNotificationHandle,
    ) -> WebNotificationPresenterTransition {
        let pending_matches = self
            .pending_posts
            .get(handle.id())
            .is_some_and(|pending| pending.handle == *handle);
        if pending_matches {
            self.pending_posts.remove(handle.id());
        }

        if !self.live_matches(handle) {
            return WebNotificationPresenterTransition::default();
        }
        self.remove_live(handle.id());
        WebNotificationPresenterTransition::from_actions(vec![
            WebNotificationPresenterHostAction::Dismiss {
                handle: handle.clone(),
            },
        ])
    }

    fn live_matches(&self, handle: &WebNotificationHandle) -> bool {
        self.live_generations.get(handle.id()) == Some(&handle.generation)
    }

    fn take_live(&mut self, handle: &WebNotificationHandle) -> Option<SystemNotification> {
        if !self.live_matches(handle) {
            return None;
        }
        self.live_generations.remove(handle.id());
        self.live.remove(handle.id())
    }

    fn remove_live(&mut self, id: &str) {
        self.live_generations.remove(id);
        self.live.remove(id);
    }
}
