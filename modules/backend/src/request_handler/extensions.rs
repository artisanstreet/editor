//! Agent extensions and the global prompt: reads answer the last
//! reconciled state, and every change goes through the Forge's extension
//! sync, which applies it to the engines before answering.

use artisan_domain::{Command, Query, RequestId};
use artisan_protocol::{ErrorCode, ProtocolFailure, ResponsePayload, ServerResponse};

use super::RequestHandler;
use super::failures::{outcome, typed_failure};
use crate::extension_sync::{ExtensionSync, SyncRefusal};

impl RequestHandler {
    /// Answers the extension catalog or global prompt read.
    pub(super) fn extension_query_outcome(
        &self,
        request_id: &RequestId,
        query: &Query,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let sync = self.extension_sync_for(request_id)?;
        let payload = match query {
            Query::ReadGlobalPrompt(_) => ResponsePayload::GlobalPrompt(sync.global_prompt()),
            _ => ResponsePayload::Extensions(sync.catalog()),
        };
        Ok(outcome(request_id, payload))
    }

    /// Applies an extension or global prompt change and answers the result.
    pub(super) async fn extension_command_outcome(
        &self,
        request_id: &RequestId,
        command: &Command,
    ) -> Result<ServerResponse, ProtocolFailure> {
        let sync = self.extension_sync_for(request_id)?;
        let refused = |refusal: SyncRefusal| {
            let code = if refusal.retryable {
                ErrorCode::Internal
            } else {
                ErrorCode::InvalidInput
            };
            typed_failure(code, refusal.message, refusal.retryable, request_id)
        };
        let payload = match command {
            Command::SetExtensionEnabled(change) => ResponsePayload::Extensions(
                sync.set_enabled(&change.extension_id, change.enabled)
                    .await
                    .map_err(refused)?,
            ),
            Command::AdoptExtension(adopt) => {
                ResponsePayload::Extensions(sync.adopt(&adopt.extension_id).await.map_err(refused)?)
            }
            Command::SaveGlobalPrompt(save) => ResponsePayload::GlobalPrompt(
                sync.save_prompt(save.base_revision, &save.text)
                    .await
                    .map_err(refused)?,
            ),
            Command::ResolveGlobalPrompt(resolve) => ResponsePayload::GlobalPrompt(
                sync.resolve_prompt(&resolve.resolution)
                    .await
                    .map_err(refused)?,
            ),
            _ => {
                return Err(typed_failure(
                    ErrorCode::InvalidInput,
                    "not an extension or global prompt command",
                    false,
                    request_id,
                ));
            }
        };
        Ok(outcome(request_id, payload))
    }

    fn extension_sync_for(
        &self,
        request_id: &RequestId,
    ) -> Result<&ExtensionSync, ProtocolFailure> {
        self.extension_sync.as_deref().ok_or_else(|| {
            typed_failure(
                ErrorCode::UnsupportedFeature,
                "This Forge does not manage engine extensions.",
                false,
                request_id,
            )
        })
    }
}
