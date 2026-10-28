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
    KeyError, MachineKeypair, PairStatus, enrollment_confirm_body, enrollment_request_body,
    normalise_code, sign_enrollment_proof, verify_enrollment_proof,
};
pub use frames::{
    AckFrame, AuthenticateFrame, ChallengeFrame, ChannelClosedFrame, ChannelOpenedFrame,
    ClientFrame, CloseFrame, DataFrame, ErrorFrame, HelloFrame, OpenFrame, PingFrame, PongFrame,
    PresenceFrame, ReadyFrame, ServerFrame, read_server_frame,
};
pub use grant::{
    AUD_RELAY, Aud, CnfHolder, CnfJwk, GrantClaims, GrantError, GrantErrorKind, Role, Scope,
    VerifyParams, cnf_public_key, is_account_id, is_machine_id, parse_and_verify_jwt,
    unverified_jti, unverified_kid,
};
pub use machine_proof::{
    MachineProof, MachineProofError, body_hash_hex, canonical_request, sign_proof,
    verify_machine_proof,
};
pub use network::{AccountProfile, CoreSessionClient, SessionToken};
pub use noise::{ChannelCtx, NoiseChannel, Phase};
pub use pop::pop_sign;

const PROTOCOL_VERSION: u32 = 1;

pub const GRANT_MAX_TTL_S: i64 = 300;
