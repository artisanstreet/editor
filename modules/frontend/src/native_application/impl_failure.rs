//! Failure-state methods for [`NativeApplication`].
//!
//! Extracted verbatim from `native_application.rs` during the phase-5 module
//! split; visibility was widened to `pub(super)` for parent-owned methods.

use super::*;

impl NativeApplication {
    pub(super) fn set_failure(&mut self, failure: ServiceFailure, cx: &mut Context<Self>) {
        #[cfg(feature = "flight-recorder")]
        artisan_tracing::instant!("ui", "application.failure", "stage" => failure.stage.to_string(), "category" => failure.category.to_string());
        #[cfg(feature = "flight-recorder")]
        self.trace_state();
        #[cfg(feature = "flight-recorder")]
        artisan_tracing::incident!("application.failure");
        self.state = NativeViewState::Failure(failure);
        self.sync_composer_availability(cx);
        cx.notify();
    }
}
