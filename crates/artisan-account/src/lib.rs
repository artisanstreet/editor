#![expect(
    clippy::inconsistent_struct_constructor,
    clippy::partial_pub_fields,
    clippy::renamed_function_params,
    clippy::unused_result_ok,
    let_underscore_drop,
    missing_debug_implementations,
    unreachable_pub,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#![allow(
    clippy::allow_attributes,
    clippy::assertions_on_result_states,
    unused_qualifications,
    reason = "pre-existing violations that only occur in some builds, cleared in plan.md phase 2"
)]
pub mod browser;
mod enrollment;
mod frames;
mod grant;
mod machine_proof;
pub mod network;
mod noise;
mod pop;
pub mod relay_client;
mod signin;

pub use enrollment::{
    MachineKeyError, MachineKeypair, PairStatus, enrollment_confirm_body, enrollment_request_body,
    normalise_code, sign_enrollment_proof, verify_enrollment_proof,
};
pub use frames::{
    AckFrame, AuthenticateFrame, ChallengeFrame, ChannelClosedFrame, ChannelOpenedFrame,
    ClientFrame, CloseFrame, DataFrame, ErrorFrame, HelloFrame, OpenFrame, PingFrame, PongFrame,
    PresenceFrame, ReadyFrame, ServerFrame, read_server_frame,
};
pub use grant::{
    AUD_RELAY, Aud, CnfHolder, CnfJwk, GrantClaims, GrantError, GrantErrorKind, Role, Scope,
    UnverifiedGrantError, VerifyParams, cnf_public_key, is_account_id, is_machine_id,
    parse_and_verify_jwt, unverified_jti, unverified_kid,
};
pub use machine_proof::{
    MachineProof, MachineProofError, body_hash_hex, canonical_request, sign_proof,
    verify_machine_proof,
};
pub use network::{AccountProfile, CoreSessionClient, SessionToken};
pub use noise::{ChannelCtx, NoiseChannel, NoiseError, NoiseFailure, Phase};
pub use pop::pop_sign;

const PROTOCOL_VERSION: u32 = 1;

pub const GRANT_MAX_TTL_S: i64 = 300;
