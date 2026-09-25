//! Durable thread engine-configuration mutations.

use artisan_database::SetThreadEngineConfigInput;
use artisan_domain::{RequestId, SetThreadEngineConfig};
use artisan_protocol::{
    ProtocolFailure, ResponsePayload, ServerResponse, SetThreadEngineConfigResult,
};

use super::failures::{outcome, repository_failure};
use super::{RequestHandler, origin_clock_failure};

impl RequestHandler {
    /// Answers a durable thread engine-configuration mutation. Receipt
    /// lookup is deliberately before the acceptance clock so exact replays
    /// never consult fresh admission state.
    pub(super) async fn set_thread_engine_config_outcome(
        &self,
        request_id: &RequestId,
        config: &SetThreadEngineConfig,
    ) -> Result<ServerResponse, ProtocolFailure> {
        if let Some(replay) = self
            .repository
            .lookup_set_thread_engine_config(
                config.request_id(),
                config.thread_id(),
                config.precondition(),
                config.config(),
            )
            .await
            .map_err(|error| repository_failure(&error, request_id))?
        {
            return Ok(set_thread_engine_config_response(request_id, &replay));
        }
        let accepted_at = self
            .origin
            .acceptance_instant()
            .map_err(|error| origin_clock_failure(error, request_id))?;
        let result = self
            .repository
            .set_thread_engine_config(SetThreadEngineConfigInput {
                request_id: config.request_id().clone(),
                thread_id: config.thread_id().clone(),
                precondition: config.precondition(),
                config: config.config().clone(),
                accepted_at,
            })
            .await
            .map_err(|error| repository_failure(&error, request_id))?;
        Ok(set_thread_engine_config_response(request_id, &result))
    }
}

fn set_thread_engine_config_response(
    request_id: &RequestId,
    result: &artisan_database::SetThreadEngineConfigResult,
) -> ServerResponse {
    outcome(
        request_id,
        ResponsePayload::ThreadEngineConfigSet(SetThreadEngineConfigResult {
            request_id: result.receipt().request_id.clone(),
            thread_id: result.thread_id().clone(),
            revision: result.revision(),
            disposition: result.receipt().disposition,
        }),
    )
}
