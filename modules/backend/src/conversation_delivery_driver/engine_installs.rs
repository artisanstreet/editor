//! Engine-install delivery: a connection that read the engine installs (or
//! changed an engine's version) receives every later status change.

use std::time::Duration;

use artisan_domain::Event;
use artisan_transport::{CancelHandle, DeadlineError};

use crate::connection::{RequestStageError, ServerFrameStamp};

use super::ConversationDeliveryDriver;

impl ConversationDeliveryDriver {
    /// Pushes the engine install snapshot when it changed since this
    /// connection last read or received it. The engine manager wakes every
    /// connection through the host-state notifier on each change.
    pub(super) async fn deliver_engine_installs<F>(
        &mut self,
        stamp: &mut F,
        limit: Duration,
        cancel: &CancelHandle,
    ) -> Result<(), DeadlineError<RequestStageError>>
    where
        F: FnMut() -> Result<ServerFrameStamp, RequestStageError>,
    {
        if self.engines.is_none() {
            return Ok(());
        }
        let Some(current) = self
            .context
            .account_usage()
            .and_then(crate::account_usage_service::AccountUsageService::engine_installs)
        else {
            return Ok(());
        };
        if self.engines.as_ref() == Some(&current) {
            return Ok(());
        }
        let event = Event::EngineInstalls(current.clone());
        self.send_state_event(event, stamp, limit, cancel).await?;
        self.engines = Some(current);
        Ok(())
    }
}
