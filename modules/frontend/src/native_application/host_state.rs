//! Connection-scoped state the Forge pushes over the delivery stream.
//!
//! The Forge pushes every engine's usage with its readiness verdict, the
//! user's preferences, and the subscribed thread's display title whenever
//! one changes.

use crate::native_transport_service::HostStateEvent;

use super::*;

impl NativeApplication {
    /// Applies one pushed host-state value.
    pub(super) fn apply_host_state(&mut self, state: HostStateEvent, cx: &mut Context<Self>) {
        match state {
            HostStateEvent::Preferences(preferences) => {
                self.apply_forge_preferences(&preferences, cx);
            }
            HostStateEvent::AccountUsage(_) | HostStateEvent::ThreadRetitled(_) => {}
        }
        cx.notify();
    }
}
