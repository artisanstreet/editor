//! Listener construction with explicit interface selection and unchanged authentication.
use super::{
    CommandOrigin, CredentialAuthority, ForgeListener, LifecycleController, ListenerError,
    ListenerLimits, LocalCapability, NonZeroU32, ServerConfig, SocketAddr, TransportError,
    apply_approved_pending_peer_limits,
};

impl ForgeListener {
    /// Validates limits, applies the approved pending-peer bounds, and binds
    /// the loopback endpoint.
    ///
    /// The supplied configuration keeps every caller-selected TLS and
    /// established-transport setting; before binding, exactly three approved
    /// pending-peer mutations are applied (`max_incoming(8)`,
    /// `incoming_buffer_size(65_536)`,
    /// `incoming_buffer_size_total(524_288)` — methods verified against the
    /// pinned Quinn sources). They bound outstanding queued peers and their
    /// buffered bytes; each queued peer's first packet and all other
    /// allocation overhead are excluded, so this is not a total
    /// endpoint-memory cap.
    ///
    /// # Errors
    ///
    /// Returns [`ListenerError::UnrepresentableLimits`] before binding when
    /// any limit cannot produce a future instant, and
    /// [`ListenerError::Bind`] when the loopback socket cannot be bound.
    pub fn bind(
        server_config: ServerConfig,
        bootstrap: LocalCapability,
        origin: Box<dyn CommandOrigin>,
        limits: ListenerLimits,
        admission_capacity: NonZeroU32,
        requests_per_connection: NonZeroU32,
    ) -> Result<Self, ListenerError> {
        Self::bind_with_lifecycle(
            server_config,
            bootstrap,
            origin,
            limits,
            admission_capacity,
            requests_per_connection,
            LifecycleController::new(),
        )
    }

    /// Binds a listener with a crate-local lifecycle controller.
    pub(crate) fn bind_with_lifecycle(
        server_config: ServerConfig,
        bootstrap: LocalCapability,
        origin: Box<dyn CommandOrigin>,
        limits: ListenerLimits,
        admission_capacity: NonZeroU32,
        requests_per_connection: NonZeroU32,
        lifecycle: LifecycleController,
    ) -> Result<Self, ListenerError> {
        Self::bind_at_with_lifecycle(
            server_config,
            bootstrap,
            origin,
            limits,
            admission_capacity,
            requests_per_connection,
            lifecycle,
            "127.0.0.1:0".parse().expect("local bind address"),
        )
    }

    /// Binds the explicitly configured interface using the same authentication and bounds.
    #[expect(
        clippy::too_many_arguments,
        reason = "explicit listener policy and lifecycle plus bind address"
    )]
    pub(crate) fn bind_at_with_lifecycle(
        server_config: ServerConfig,
        bootstrap: LocalCapability,
        origin: Box<dyn CommandOrigin>,
        limits: ListenerLimits,
        admission_capacity: NonZeroU32,
        requests_per_connection: NonZeroU32,
        lifecycle: LifecycleController,
        address: SocketAddr,
    ) -> Result<Self, ListenerError> {
        if !limits.representable() {
            return Err(ListenerError::UnrepresentableLimits);
        }

        let bounded_config = apply_approved_pending_peer_limits(server_config);

        let endpoint = quinn::Endpoint::server(bounded_config, address)
            .map_err(TransportError::Bind)
            .map_err(ListenerError::Bind)?;

        Ok(Self {
            endpoint,
            authority: CredentialAuthority::new(bootstrap),
            origin,
            limits,
            admission_remaining: admission_capacity.get(),
            requests_per_connection,
            lifecycle,
        })
    }
}
