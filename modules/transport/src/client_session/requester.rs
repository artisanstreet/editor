//! [`SessionRequester`]: requests on a session's connection beside its
//! owner.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use artisan_domain::RequestId;
use artisan_protocol::{ConnectionId, ProtocolVersion, WireEnvelope};
use quinn::Connection;

use super::exchange::{open_and_send, receive_reply};
use super::{ClientRequestError, lock, settle_reply, validate_outbound};
use crate::request_lifecycle::{ClientRequestLifecycle, OutcomeWaiter, ResolvedRequest};
use crate::{CancelHandle, DeadlineError, OperationKind, run_with_deadline};

/// Runs requests on one session's connection beside its owner; see the
/// [module documentation](super#concurrent-requests).
///
/// Cloning a requester is cheap; every clone shares the session's
/// correlation registry and its [`CONCURRENT_REQUEST_CAPACITY`](super::CONCURRENT_REQUEST_CAPACITY).
#[derive(Clone, Debug)]
pub struct SessionRequester {
    pub(super) connection: Connection,
    pub(super) lifecycle: Arc<Mutex<ClientRequestLifecycle>>,
    pub(super) concurrent: Arc<tokio::sync::Semaphore>,
    pub(super) negotiated_version: ProtocolVersion,
    pub(super) connection_id: ConnectionId,
    pub(super) lifecycle_control_supported: bool,
    /// Bounds opening the stream and sending the request.
    pub(super) send_limit: Duration,
}

impl SessionRequester {
    /// Returns the negotiated application protocol revision.
    #[must_use]
    pub const fn protocol_version(&self) -> ProtocolVersion {
        self.negotiated_version
    }

    /// Returns the server-assigned identity of the session's connection.
    #[must_use]
    pub fn connection_id(&self) -> &ConnectionId {
        &self.connection_id
    }

    /// Whether the session's connection has closed, for whatever reason.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.connection.close_reason().is_some()
    }

    /// Runs one request on its own stream and returns its settled outcome.
    ///
    /// Waits for one of the [`CONCURRENT_REQUEST_CAPACITY`](super::CONCURRENT_REQUEST_CAPACITY) turns, then
    /// validates and admits the request exactly as
    /// [`ClientSession::request`](super::ClientSession::request) does. Opening the stream and sending the
    /// request are bounded by the session's request limit; the reply is
    /// awaited for as long as the peer takes to answer, until `cancel` or
    /// the connection ends it. Dropping the future, and every error,
    /// abandons the request: its stream is reset and stopped and its
    /// identity stays retired. Nothing here closes the connection.
    ///
    /// # Errors
    ///
    /// Returns the same typed errors as [`ClientSession::request`](super::ClientSession::request); a
    /// cancelled wait for a turn is [`ClientRequestError::Exchange`] with
    /// [`DeadlineError::Cancelled`].
    pub async fn request(
        &self,
        envelope: WireEnvelope,
        cancel: &CancelHandle,
    ) -> Result<ResolvedRequest, ClientRequestError> {
        let request_id = validate_outbound(
            &envelope,
            self.negotiated_version,
            self.lifecycle_control_supported,
        )?;
        let cancelled = || {
            ClientRequestError::Exchange(DeadlineError::Cancelled {
                operation: OperationKind::Receive,
            })
        };
        let _turn = tokio::select! {
            biased;
            () = cancel.wait() => return Err(cancelled()),
            turn = self.concurrent.acquire() => turn.map_err(|_| cancelled())?,
        };
        let waiter = lock(&self.lifecycle)
            .admit(request_id.clone())
            .map_err(ClientRequestError::Admission)?;
        let mut admitted = Admitted {
            lifecycle: &self.lifecycle,
            request_id: &request_id,
            waiter,
            settled: false,
        };
        let mut streams = run_with_deadline(
            OperationKind::Send,
            self.send_limit,
            cancel,
            open_and_send(&self.connection, &envelope),
        )
        .await
        .map_err(ClientRequestError::Exchange)?;
        streams.finish_outbound();
        let reply = tokio::select! {
            biased;
            () = cancel.wait() => return Err(cancelled()),
            reply = receive_reply(&mut streams) => reply.map_err(|error| {
                ClientRequestError::Exchange(DeadlineError::Peer {
                    operation: OperationKind::Receive,
                    error,
                })
            })?,
        };
        drop(streams);
        settle_reply(
            &self.lifecycle,
            &reply,
            self.negotiated_version,
            &request_id,
        )?;
        admitted.settled = true;
        Ok(admitted
            .waiter
            .take_outcome()
            .expect("the admitted request settled exactly once into its private waiter"))
    }
}

/// One requester request admitted to the shared registry; abandoned there
/// unless it settled.
struct Admitted<'a> {
    lifecycle: &'a Mutex<ClientRequestLifecycle>,
    request_id: &'a RequestId,
    waiter: OutcomeWaiter,
    settled: bool,
}

impl Drop for Admitted<'_> {
    fn drop(&mut self) {
        if !self.settled {
            let _abandoned = lock(self.lifecycle).abandon(self.request_id);
        }
    }
}
