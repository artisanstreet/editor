//! Owned endpoint-and-connection pair with deterministic teardown.
//!
//! [`SessionLink`] exists so no awaited stage can escape the transport it
//! created: the pair is created before the first network await of a
//! session establishment, and its [`Drop`] closes the connection
//! synchronously whenever an error, an early return, or a dropped future
//! abandons the stage. Closing is infallible and never overwrites a
//! primary typed error — closing an already-closed connection is simply
//! ignored — and no drain is promised on this path. The awaited drain is
//! [`ClientSession::shutdown`](super::ClientSession::shutdown)'s separate
//! contract.
//!
//! Close codes and reasons are fixed private constants: they identify
//! this leaf's teardown across processes without encoding any payload,
//! credential, or session state.

use quinn::{ClientConfig, Connection, Endpoint, VarInt};

use crate::TransportError;

/// Fixed application close code sent when a session abandons its
/// connection without a graceful shutdown: every local error path and
/// every drop that finds a live connection uses this code.
///
/// These codes deliberately identify this leaf's teardown across
/// processes, so they are stable contract even though the constants
/// stay private.
pub(super) const ABANDON_CODE: u32 = 1;

/// Fixed application close code for a normal, awaited session shutdown.
pub(super) const SHUTDOWN_CODE: u32 = 2;

/// Fixed private code used to RESET one abandoned, unfinished outbound
/// stream. Send-stream drop alone would FIN the output, which is not a
/// reset, so guarded abandonment resets explicitly with this code.
pub(super) const STREAM_RESET_CODE: u32 = 3;

/// Fixed private code used to STOP inbound delivery on one abandoned
/// stream.
pub(super) const STREAM_STOP_CODE: u32 = 4;

/// Fixed non-secret reason carried with [`ABANDON_CODE`].
pub(super) const ABANDON_REASON: &[u8] = b"artisan client session abandoned";

/// Fixed non-secret reason carried with [`SHUTDOWN_CODE`].
pub(super) const SHUTDOWN_REASON: &[u8] = b"artisan client session shutdown";

/// Converts one numeric close code to its QUIC varint form.
pub(super) fn close_code(code: u32) -> VarInt {
    VarInt::from_u32(code)
}

/// One freshly bound loopback client endpoint plus, once installed, its
/// single connection.
///
/// Deliberately implements neither [`Clone`] nor [`Copy`]: the pair is
/// the exclusive transport owned by one session establishment or, after
/// [`SessionLink::disband`], one live session.
pub(super) struct SessionLink {
    /// Privately owned client endpoint bound to `127.0.0.1:0`. Held as
    /// an option only so [`SessionLink::disband`] can move both fields
    /// out of a [`Drop`] type; it is always present until then.
    endpoint: Option<Endpoint>,
    /// The established connection, present from installation onward and
    /// absent again once disbanded.
    connection: Option<Connection>,
}

impl SessionLink {
    /// Binds the session's private client endpoint.
    ///
    /// # Errors
    ///
    /// Returns [`TransportError::Bind`] when the loopback socket cannot
    /// be bound.
    pub(super) fn bind(config: ClientConfig) -> Result<Self, TransportError> {
        Ok(Self {
            endpoint: Some(crate::bind_loopback_client(config)?),
            connection: None,
        })
    }

    /// Borrows the bound endpoint.
    pub(super) fn endpoint(&self) -> &Endpoint {
        self.endpoint
            .as_ref()
            .expect("the endpoint is bound at construction")
    }

    /// Installs the established connection so every later escape from a
    /// guarded stage closes it.
    ///
    /// # Panics
    ///
    /// Panics if called twice, which would mean a stage installed two
    /// connections into one link; no stage does.
    pub(super) fn install(&mut self, connection: Connection) {
        assert!(
            self.connection.is_none(),
            "a session link installs exactly one connection"
        );
        self.connection = Some(connection);
    }

    /// Borrows the installed connection.
    ///
    /// # Panics
    ///
    /// Panics if no connection was installed yet; guarded stages install
    /// before their first use of the connection.
    pub(super) fn connection(&self) -> &Connection {
        self.connection
            .as_ref()
            .expect("connection installed before use")
    }

    /// Dismantles a successfully established link into the session's
    /// owned fields without closing anything.
    ///
    /// Taking both fields leaves the guard empty, so its later [`Drop`]
    /// performs no teardown: the live session owns the resources now and
    /// closes them through its own drop and shutdown paths.
    ///
    /// # Panics
    ///
    /// Panics if either resource is missing, which no successful
    /// establishment reaches.
    pub(super) fn disband(mut self) -> (Endpoint, Connection) {
        let endpoint = self.endpoint.take().expect("endpoint present");
        let connection = self.connection.take().expect("connection installed");
        (endpoint, connection)
    }
}

impl Drop for SessionLink {
    fn drop(&mut self) {
        // Synchronous teardown only: the close frames are emitted by the
        // background driver, and no drain is awaited here. Closing an
        // already-closed connection is ignored and preserves whatever
        // primary error preceded cleanup.
        if let Some(connection) = &self.connection {
            connection.close(close_code(ABANDON_CODE), ABANDON_REASON);
        }
        // The endpoint is closed explicitly rather than left to handle
        // drop: dropping the last endpoint handle only wakes the driver,
        // while `Endpoint::close` deterministically closes every
        // connection this private endpoint owns.
        if let Some(endpoint) = self.endpoint.take() {
            endpoint.close(close_code(ABANDON_CODE), ABANDON_REASON);
        }
    }
}
