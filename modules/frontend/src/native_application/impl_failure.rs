//! Failure-state and retry-affordance methods for [`NativeApplication`].
//!
//! Extracted verbatim from `native_application.rs` during the phase-5 module
//! split; visibility was widened to `pub(super)` for parent-owned methods.

use super::*;

impl NativeApplication {
    #[cfg(test)]
    pub(super) fn message_retry_button(&mut self, cx: &mut Context<Self>) -> Button {
        let enabled = self.message_retry_is_admissible(cx);
        self.message_retry_focus_handle = self.message_retry_focus_handle.clone().tab_stop(enabled);
        let application = cx.entity().downgrade();
        Button::new(
            NATIVE_MESSAGE_RETRY_SELECTOR,
            self.message_retry_focus_handle.clone(),
            self.theme,
            MotionPolicy::Reduced,
            ButtonVariant::Ghost,
            ButtonSize::Small,
            ButtonContent::text(NATIVE_MESSAGE_RETRY_LABEL),
        )
        .expect("the native message retry button configuration is valid")
        .focus_visibility(FocusVisibility::Visible)
        .disabled(!enabled)
        .debug_selector(NATIVE_MESSAGE_RETRY_SELECTOR)
        .on_activate(move |_, _, app| {
            let _ = application.update(app, |application, cx| {
                application.activate_message_retry(cx);
            });
        })
    }

    pub(super) fn set_failure(&mut self, failure: ServiceFailure, cx: &mut Context<Self>) {
        self.clear_message_retry();
        self.state = NativeViewState::Failure(failure);
        self.sync_composer_availability(cx);
        cx.notify();
    }
}
