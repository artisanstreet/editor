//! Extension and global prompt delivery: a connection that read the
//! extension catalog or the global prompt (or changed them) receives every
//! later change.

use std::time::Duration;

use artisan_domain::Event;
use artisan_transport::{CancelHandle, DeadlineError};

use crate::connection::{RequestStageError, ServerFrameStamp};

use super::ConversationDeliveryDriver;

impl ConversationDeliveryDriver {
    /// Pushes the extension catalog and the global prompt when they changed
    /// since this connection last read or received them. The extension sync
    /// wakes every connection through the host-state notifier on each change.
    pub(super) async fn deliver_extensions<F>(
        &mut self,
        stamp: &mut F,
        limit: Duration,
        cancel: &CancelHandle,
    ) -> Result<(), DeadlineError<RequestStageError>>
    where
        F: FnMut() -> Result<ServerFrameStamp, RequestStageError>,
    {
        let Some(sync) = self.context.extension_sync() else {
            return Ok(());
        };
        let catalog = self.extensions.is_some().then(|| sync.catalog());
        let prompt = self.global_prompt.is_some().then(|| sync.global_prompt());
        if let Some(current) = catalog
            && self.extensions.as_ref() != Some(&current)
        {
            self.send_state_event(Event::Extensions(current.clone()), stamp, limit, cancel)
                .await?;
            self.extensions = Some(current);
        }
        if let Some(current) = prompt
            && self.global_prompt.as_ref() != Some(&current)
        {
            self.send_state_event(Event::GlobalPrompt(current.clone()), stamp, limit, cancel)
                .await?;
            self.global_prompt = Some(current);
        }
        Ok(())
    }
}
