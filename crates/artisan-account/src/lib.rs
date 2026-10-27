//! AST account grants, enrollment, and Relay admission framing.
//!
//! Follows the root-reconciled contract (pending Core `docs/contracts.md`
//! update): EdDSA JWT with `kid`, `sub` = account, `mid` = explicit target
//! machine for ALL relay grants (including Editor), `v: 1`,
//! `role: client | forge`, `cnf.jwk` Ed25519, `iat`/`nbf`/`exp`/`jti`/
//! `iss`/`aud`, `aud` in {`ast-relay`, `ast-core`}, scopes
//! `relay:connect`, `forge:execute`, `machine:sync`, TTL max 300 s.
//! Machine `cnf` matches the enrolled key; Editor PoP keys are ephemeral.
//! Admission JWT travels in-band (`hello`), never in URL or WS headers.
//! Core binding prevents cross-account and cross-machine routes.
//!
//! Explicitly NOT here: the Relay custom dual-DH "HPKE-like" KEM/envelope.
//! Root rejected that freeze. Data-channel crypto waits for the RFC9180
//! HPKE-or-Noise decision (directional keys, replay protection, accurate
//! forward secrecy). This crate carries opaque `data` bodies only.

mod discovery;
mod enrollment;
mod frames;
mod grant;
mod machine_proof;
pub mod network;
mod noise;
mod pop;
mod relay;
pub mod relay_client;
mod signin;
mod sync;

pub use discovery::Discovery;
pub use sync::{
    ConfigRevision, MachineRecord, RouteConditions, RoutingDecision, VaultItem, decide_route,
};
pub use enrollment::{
    Fingerprint, KeyError, MachineKeypair, PairStatus, enrollment_confirm_body,
    enrollment_request_body, normalise_code, sign_enrollment_proof, verify_enrollment_proof,
};
pub use frames::{
    AckFrame, AuthenticateFrame, Body, ChallengeFrame, ChannelClosedFrame, ChannelId,
    ChannelOpenedFrame, ClientFrame, CloseFrame, DataFrame, ErrorFrame, HelloFrame, OpenFrame,
    PingFrame, PongFrame, PresenceFrame, ReadyFrame, ServerFrame, decode_client_frame,
    is_valid_channel, read_server_frame, seq_advances,
};
pub use grant::{
    AUD_CORE, AUD_RELAY, AccountId, Aud, CnfHolder, CnfJwk, GrantClaims, GrantError, GrantErrorKind,
    MachineId, Role, Scope, VerifyParams, cnf_public_key, is_account_id, is_core_id, is_machine_id,
    is_valid_id, parse_and_verify_jwt, unverified_jti, unverified_kid,
    verify_claims,
};
pub use pop::{POP_DOMAIN, generate_ephemeral, pop_message, pop_sign, pop_verify};
pub use machine_proof::{
    MachineProof, MachineProofError, body_hash_hex, canonical_request, sign_proof,
    verify_machine_proof, EMPTY_BODY_HASH, MIN_NONCE_BYTES, PROOF_TIMESTAMP_WINDOW_S,
};
pub use noise::{
    APP_MAGIC, APP_VERSION, BIND_DOMAIN, InitiatorConfirm, KIND_CONFIRM, KIND_DATA,
    MAX_APP_PLAINTEXT, MAX_GRANT_JWT, MAX_SESSION_AGE_MS, NOISE_PROTOCOL, PROLOGUE_DOMAIN,
    ROLE_INITIATOR, ROLE_RESPONDER, ROTATE_AFTER_MESSAGES, ChannelCtx, NoiseChannel, NoiseError,
    Phase, StaticKeypair, bind_message, bind_sign, bind_verify, frame, grant_cnf_matches,
    parse_frame, prologue_bytes, verify_inner_grant_for_execution,
};
pub use relay::{
    CHALLENGE_TTL_MS, HELLO_TIMEOUT_MS, MAX_BODY_B64, MAX_CLIENTS, MAX_QUEUE_PER_CHANNEL,
    PING_INTERVAL_MS, PONG_TIMEOUT_MS, RELAY_AUD, RoutePath, route_path,
};
pub use relay_client::{RelayClient, RelayDialContext, RelayEventStream, relay_base_url_from_env};
pub use network::{
    AccountProfile, BundleHead, ClaimRequest, ClaimedEnrollment, CoreSessionClient, GrantCnf,
    GrantIssueRequest, GrantToken, Jwk, MachineEntry, SessionEntry, SessionToken, TotpDisableRequest,
    TotpEnrollResponse, TotpVerifyRequest, NetworkError,
};
pub use signin::{
    MagicConsumeRequest, MagicConsumeResponse, MagicFinalizeRequest, MagicFinalizeResponse,
    MagicRequest, MagicRequestResponse, MagicStatusRequest, MagicStatusResponse, SessionInfo,
};

/// The reconciled protocol version. A bump is required for new fields.
pub const PROTOCOL_VERSION: u32 = 1;

/// Maximum grant lifetime in seconds.
pub const GRANT_MAX_TTL_S: i64 = 300;
