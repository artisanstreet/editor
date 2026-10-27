//! Ordered Noise transport for the Relay path.
//!
//! Suite: `Noise_XX_25519_ChaChaPoly_BLAKE2b` via the established `snow`
//! crate (Rust) matching Relay `contracts/e2e.md` + `src/channel.ts` (JS:
//! `noise-handshake`). This module adds only the same application
//! conventions ABOVE Noise: prologue format, optional static pinning,
//! mutual key confirmation with Core-identity binding signatures, framing,
//! rotation policy. No handshake crypto here.
//!
//! Byte-exact rules (`e2e.md` §1–§3):
//! - Prologue: `domain NUL account NUL machine NUL channel`.
//! - Frame: `"AR1" || 0x01 || ch_len(u8) || channel || kind(u8) || body`.
//!   No app-level sequence space: ordering and replay protection come
//!   from the sequential Noise transport nonces (fail-closed).
//! - Initiator confirm:
//!   `handshake_hash(64) || jwt_len(u16be) || grant_jwt || bind_sig(64)`,
//!   `bind_sig = Ed25519(cnf_sk, "ast-relay-bind-v1" || 0x01 || hash)`.
//! - Responder confirm: `handshake_hash(64) || bind_sig(64)`,
//!   `bind_sig = Ed25519(enrolled_sk, "ast-relay-bind-v1" || 0x02 || hash)`.
//! - The handshake hash already covers prologue and every handshake byte,
//!   so signing it binds the exact negotiated transcript; the role byte
//!   separates directions. Confirm frames are single-shot per direction.
//!
//! Identity binding: the grant JWT travels INSIDE the initiator's
//! encrypted confirm. The responder verifies that JWT itself (Core JWKS)
//! and takes ONLY the verified `cnf` key for the binding check; the
//! initiator checks the responder signature against the Forge enrolled
//! key from the authenticated Core machine record. No static-key
//! registry, no per-session publishing step. Relay admission alone never
//! authorizes execution: the verified grant must also carry
//! `forge:execute` before the Forge runs anything.

use std::time::{SystemTime, UNIX_EPOCH};

use sha2::Digest as _;

/// Must match Relay `contracts/e2e.md` §1.
pub const NOISE_PROTOCOL: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2b";
/// Prologue domain binding account/machine/channel into the handshake hash.
pub const PROLOGUE_DOMAIN: &str = "ast-relay-noise-v1";
/// Domain binding a Noise session to the Core Ed25519 identity.
pub const BIND_DOMAIN: &str = "ast-relay-bind-v1";
/// Initiator role byte in binding messages.
pub const ROLE_INITIATOR: u8 = 0x01;
/// Responder role byte in binding messages.
pub const ROLE_RESPONDER: u8 = 0x02;
/// Application framing magic.
pub const APP_MAGIC: &[u8; 3] = b"AR1";
/// Application frame version.
pub const APP_VERSION: u8 = 0x01;
/// Key-confirmation frame kind.
pub const KIND_CONFIRM: u8 = 0x00;
/// Data frame kind.
pub const KIND_DATA: u8 = 0x01;
/// Plaintext cap per transport message (Noise cap 65535 incl. 16B tag).
pub const MAX_APP_PLAINTEXT: usize = 60_000;
/// In-confirm JWT cap (grants are small; bounds confirm parsing).
pub const MAX_GRANT_JWT: usize = 8192;
/// Rotate (new channel + handshake) after this many transport messages.
pub const ROTATE_AFTER_MESSAGES: u64 = 1 << 20;
/// Rotate after this age even if quiet (ms).
pub const MAX_SESSION_AGE_MS: u64 = 24 * 60 * 60 * 1000;

/// Handshake hash length for BLAKE2b (enforced, matches `channel.ts`).
pub const HANDSHAKE_HASH_LEN: usize = 64;
/// Ed25519 signature length.
pub const SIGNATURE_LEN: usize = 64;

/// Handshake + transport errors. Never carries key material or plaintext.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum NoiseError {
    #[error("unsupported Noise protocol")]
    Protocol,
    #[error("bad handshake turn or length")]
    Handshake,
    #[error("peer static key mismatch: unknown peer, session aborted")]
    PeerStatic,
    #[error("key confirmation failed: transcript mismatch, session aborted")]
    Confirm,
    #[error("transport decrypt failed")]
    Decrypt,
    #[error("bad frame")]
    Frame,
    #[error("channel mismatch")]
    Channel,
    #[error("message too large, chunk it")]
    TooLarge,
    #[error("session not ready for transport")]
    NotReady,
    #[error("session not open")]
    NotOpen,
    #[error("session closed")]
    Closed,
    #[error("bad key")]
    Key,
}

/// Channel context bound into the prologue and every frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelCtx {
    pub account: String,
    pub machine: String,
    pub channel: String,
}

/// Prologue: `domain NUL account NUL machine NUL channel` (matches `e2e.md`).
#[must_use]
pub fn prologue_bytes(ctx: &ChannelCtx) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(PROLOGUE_DOMAIN.as_bytes());
    out.push(0x00);
    out.extend_from_slice(ctx.account.as_bytes());
    out.push(0x00);
    out.extend_from_slice(ctx.machine.as_bytes());
    out.push(0x00);
    out.extend_from_slice(ctx.channel.as_bytes());
    out
}

/// Frame: `AR1 ver ch_len ch kind body` (matches `channel.ts`; no sequence —
/// ordering comes from the sequential Noise nonces).
pub fn frame(kind: u8, channel: &str, body: &[u8]) -> Result<Vec<u8>, NoiseError> {
    if kind != KIND_CONFIRM && kind != KIND_DATA {
        return Err(NoiseError::Frame);
    }
    if channel.is_empty() || channel.len() > 64 {
        return Err(NoiseError::Frame);
    }
    let mut out = Vec::with_capacity(3 + 1 + 1 + channel.len() + 1 + body.len());
    out.extend_from_slice(APP_MAGIC);
    out.push(APP_VERSION);
    let ch_len =
        u8::try_from(channel.len()).map_err(|_| NoiseError::Frame)?;
    out.push(ch_len);
    out.extend_from_slice(channel.as_bytes());
    out.push(kind);
    out.extend_from_slice(body);
    Ok(out)
}

/// Parse a frame, checking magic/version/channel/kind (not order).
pub fn parse_frame(
    plaintext: &[u8],
    expected_channel: &str,
) -> Result<(u8, Vec<u8>), NoiseError> {
    if plaintext.len() < 3 + 1 + 1 + 1 {
        return Err(NoiseError::Frame);
    }
    if plaintext[0..3] != *APP_MAGIC || plaintext[3] != APP_VERSION {
        return Err(NoiseError::Frame);
    }
    let ch_len = usize::from(plaintext[4]);
    if ch_len == 0 || ch_len > 64 || 5 + ch_len + 1 > plaintext.len() {
        return Err(NoiseError::Frame);
    }
    if plaintext[5..5 + ch_len] != *expected_channel.as_bytes() {
        return Err(NoiseError::Channel);
    }
    let kind = plaintext[5 + ch_len];
    if kind != KIND_CONFIRM && kind != KIND_DATA {
        return Err(NoiseError::Frame);
    }
    Ok((kind, plaintext[5 + ch_len + 1..].to_vec()))
}

/// Binding message: `BIND_DOMAIN || role || handshake_hash(64)`.
/// The hash already covers prologue and every handshake byte.
pub fn bind_message(role: u8, handshake_hash: &[u8]) -> Result<Vec<u8>, NoiseError> {
    if role != ROLE_INITIATOR && role != ROLE_RESPONDER {
        return Err(NoiseError::Key);
    }
    if handshake_hash.len() != HANDSHAKE_HASH_LEN {
        return Err(NoiseError::Key);
    }
    let mut out = Vec::with_capacity(BIND_DOMAIN.len() + 1 + HANDSHAKE_HASH_LEN);
    out.extend_from_slice(BIND_DOMAIN.as_bytes());
    out.push(role);
    out.extend_from_slice(handshake_hash);
    Ok(out)
}

/// An X25519 static keypair for Noise. Fresh per channel in production
/// (pure DH randomness); fixed only under the fixture hook below.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaticKeypair {
    secret: [u8; 32],
    public: [u8; 32],
}

impl StaticKeypair {
    pub fn generate() -> Result<Self, crate::enrollment::KeyError> {
        let mut secret = [0_u8; 32];
        getrandom::fill(&mut secret).map_err(|_| crate::enrollment::KeyError)?;
        Self::from_secret(secret)
    }

    pub fn from_secret(secret: [u8; 32]) -> Result<Self, crate::enrollment::KeyError> {
        let secret_key = x25519_dalek::StaticSecret::from(secret);
        let public = x25519_dalek::PublicKey::from(&secret_key);
        Ok(Self {
            secret,
            public: *public.as_bytes(),
        })
    }

    #[must_use]
    pub const fn public(&self) -> &[u8; 32] {
        &self.public
    }

    #[must_use]
    pub const fn secret(&self) -> &[u8; 32] {
        &self.secret
    }

    /// Base64url-no-pad SHA-256 of the static pubkey (peer fingerprint).
    #[must_use]
    pub fn fingerprint(&self) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(sha2::Sha256::digest(self.public))
    }
}

/// A parsed initiator confirm: the grant JWT bytes plus its binding
/// signature. The hash was already checked against the session transcript.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InitiatorConfirm {
    pub jwt: Vec<u8>,
    pub signature: [u8; 64],
}

/// Sign a session binding with a Core Ed25519 identity (`cnf`) key.
pub fn bind_sign(
    ed_secret: &[u8; 32],
    role: u8,
    handshake_hash: &[u8],
) -> Result<[u8; 64], NoiseError> {
    use ed25519_dalek::Signer as _;
    let key = ed25519_dalek::SigningKey::from_bytes(ed_secret);
    Ok(key.sign(&bind_message(role, handshake_hash)?).to_bytes())
}

/// Verify a session binding against a Core-distributed Ed25519 `cnf` key.
#[must_use]
pub fn bind_verify(
    ed_public: &[u8; 32],
    role: u8,
    handshake_hash: &[u8],
    signature: &[u8],
) -> bool {
    let message = match bind_message(role, handshake_hash) {
        Ok(message) => message,
        Err(_) => return false,
    };
    let Ok(key) = ed25519_dalek::VerifyingKey::from_bytes(ed_public) else {
        return false;
    };
    let Ok(signature) = ed25519_dalek::Signature::try_from(signature) else {
        return false;
    };
    key.verify_strict(&message, &signature).is_ok()
}

/// Check a grant's `cnf.jwk.x` equals the key that verified the binding.
#[must_use]
pub fn grant_cnf_matches(
    claims: &crate::grant::GrantClaims,
    peer_cnf_pubkey: &[u8; 32],
) -> bool {
    crate::grant::cnf_public_key(claims).is_ok_and(|key| key == *peer_cnf_pubkey)
}

/// Verify an inner grant extracted from an initiator confirm frame.
///
/// Same rules as the channel verifier (`channel-verify.ts` + Core §4):
/// Core JWT signature, `iss`/`aud`, `sub` == account, `mid` == machine,
/// current `exp`/`nbf`, scope including `relay:connect`. The caller then
/// checks [`grant_cnf_matches`] against the key that verified the binding
/// signature, and requires `forge:execute` in the stored claims before
/// running anything.
pub fn verify_inner_grant_for_execution(
    token: &str,
    issuer_pubkey: &[u8; 32],
    ctx: &ChannelCtx,
    now_sec: i64,
) -> Result<crate::grant::GrantClaims, crate::grant::GrantError> {
    use crate::grant::{Role, Scope, VerifyParams, AUD_RELAY};
    let Some(iss) = unverified_iss(token) else {
        return Err(crate::grant::GrantError {
            kind: crate::grant::GrantErrorKind::Issuer,
        });
    };
    let params = VerifyParams {
        expected_iss: iss,
        expected_aud: AUD_RELAY.into(),
        max_ttl_s: crate::GRANT_MAX_TTL_S,
        expected_sub: ctx.account.clone(),
        expected_mid: ctx.machine.clone(),
        now_sec,
    };
    let claims = crate::grant::parse_and_verify_jwt(token, issuer_pubkey, &params)?;
    if claims.role != Role::Client {
        return Err(crate::grant::GrantError {
            kind: crate::grant::GrantErrorKind::Binding,
        });
    }
    // Channel open requires `relay:connect` (mirrors `channel-verify.ts`).
    // `forge:execute` is enforced separately at dispatch time against the
    // stored verified claims.
    if !claims.scope.contains(&Scope::RelayConnect) {
        return Err(crate::grant::GrantError {
            kind: crate::grant::GrantErrorKind::Scope,
        });
    }
    Ok(claims)
}

fn unverified_iss(compact: &str) -> Option<String> {
    use base64::Engine as _;
    let payload = compact.split('.').nth(1)?;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.as_bytes())
        .ok()?;
    let value: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    value.get("iss")?.as_str().map(str::to_owned)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Handshake,
    Confirm,
    Open,
    Closed,
}

struct Handshake {
    state: snow::HandshakeState,
    sent: u64,
    received: u64,
}

struct Open {
    transport: snow::TransportState,
    handshake_hash: Vec<u8>,
    /// The peer Noise static actually negotiated (pinned when expected).
    peer_static: [u8; 32],
    confirm_sent: bool,
    confirm_received: bool,
    sent_count: u64,
    recv_count: u64,
}

enum State {
    Hand(Handshake),
    Open(Open),
    Closed,
}

/// One ordered E2E channel. The opener (always the client/Editor in the
/// Relay topology) constructs with `initiator`; Forge is responder.
/// Reconnect = a new channel (no resumption, no replay window).
///
/// Peer authentication is two-layered: when the peer Noise static is known
/// ahead, pass it as `Some` and the handshake aborts on mismatch (pinning
/// hardening). The responder (Editor keys are ephemeral, never registered)
/// passes `None` and MUST verify the in-confirm binding signature against
/// the independently verified grant `cnf` key before opening.
pub struct NoiseChannel {
    state: State,
    ctx: ChannelCtx,
    initiator: bool,
    own_static: [u8; 32],
    expected_peer_static: Option<[u8; 32]>,
    born_ms: u64,
}

fn now_ms() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis())
            .unwrap_or(0),
    )
    .unwrap_or(0)
}

impl NoiseChannel {
    fn build(
        initiator: bool,
        static_secret: &[u8; 32],
        peer_static: Option<[u8; 32]>,
        ctx: ChannelCtx,
    ) -> Result<Self, NoiseError> {
        Self::build_inner(initiator, static_secret, peer_static, ctx, None)
    }

    fn build_inner(
        initiator: bool,
        static_secret: &[u8; 32],
        peer_static: Option<[u8; 32]>,
        ctx: ChannelCtx,
        fixed_ephemeral: Option<&'static [u8; 32]>,
    ) -> Result<Self, NoiseError> {
        let params: snow::params::NoiseParams =
            NOISE_PROTOCOL.parse().map_err(|_| NoiseError::Protocol)?;
        let prologue = prologue_bytes(&ctx);
        let builder = snow::Builder::new(params);
        let builder = builder.prologue(&prologue).map_err(|_| NoiseError::Key)?;
        let mut builder = builder
            .local_private_key(static_secret)
            .map_err(|_| NoiseError::Key)?;
        if let Some(ephemeral) = fixed_ephemeral {
            builder = builder.fixed_ephemeral_key_for_testing_only(ephemeral);
        }
        let state = if initiator {
            builder.build_initiator().map_err(|_| NoiseError::Key)?
        } else {
            builder.build_responder().map_err(|_| NoiseError::Key)?
        };
        Ok(Self {
            state: State::Hand(Handshake {
                state,
                sent: 0,
                received: 0,
            }),
            ctx,
            initiator,
            own_static: x25519_dalek::PublicKey::from(&x25519_dalek::StaticSecret::from(
                *static_secret,
            ))
            .to_bytes(),
            expected_peer_static: peer_static,
            born_ms: now_ms(),
        })
    }

    pub fn initiator(
        static_secret: &[u8; 32],
        peer_static: Option<[u8; 32]>,
        ctx: ChannelCtx,
    ) -> Result<Self, NoiseError> {
        Self::build_inner(true, static_secret, peer_static, ctx, None)
    }

    pub fn responder(
        static_secret: &[u8; 32],
        peer_static: Option<[u8; 32]>,
        ctx: ChannelCtx,
    ) -> Result<Self, NoiseError> {
        Self::build_inner(false, static_secret, peer_static, ctx, None)
    }

    /// Fixture hook only (mirrors the JS `noiseOpts` hook): deterministic
    /// ephemeral for byte-frozen vectors. Production MUST NOT use this;
    /// live interop always uses library randomness.
    #[doc(hidden)]
    pub fn initiator_with_ephemeral(
        static_secret: &[u8; 32],
        peer_static: Option<[u8; 32]>,
        ctx: ChannelCtx,
        ephemeral: [u8; 32],
    ) -> Result<Self, NoiseError> {
        let leaked: &'static [u8; 32] = Box::leak(Box::new(ephemeral));
        Self::build_inner(true, static_secret, peer_static, ctx, Some(leaked))
    }

    /// Fixture hook only (mirrors the JS `noiseOpts` hook). See above.
    #[doc(hidden)]
    pub fn responder_with_ephemeral(
        static_secret: &[u8; 32],
        peer_static: Option<[u8; 32]>,
        ctx: ChannelCtx,
        ephemeral: [u8; 32],
    ) -> Result<Self, NoiseError> {
        let leaked: &'static [u8; 32] = Box::leak(Box::new(ephemeral));
        Self::build_inner(false, static_secret, peer_static, ctx, Some(leaked))
    }

    #[must_use]
    pub const fn phase(&self) -> Phase {
        match &self.state {
            State::Hand(_) => Phase::Handshake,
            State::Open(open) => {
                if open.confirm_sent && open.confirm_received {
                    Phase::Open
                } else {
                    Phase::Confirm
                }
            }
            State::Closed => Phase::Closed,
        }
    }

    /// Produce the next handshake message (XX turn order enforced).
    pub fn next_handshake_message(&mut self, out: &mut Vec<u8>) -> Result<(), NoiseError> {
        let State::Hand(hand) = &mut self.state else {
            return Err(NoiseError::Handshake);
        };
        let expect_send = (self.initiator && hand.sent == hand.received)
            || (!self.initiator && hand.sent < hand.received);
        if !expect_send {
            return Err(NoiseError::Handshake);
        }
        let mut buf = vec![0_u8; 65535];
        let len = hand
            .state
            .write_message(&[], &mut buf)
            .map_err(|_| NoiseError::Handshake)?;
        buf.truncate(len);
        out.extend_from_slice(&buf);
        hand.sent = hand.sent.saturating_add(1);
        if hand.state.is_handshake_finished() {
            self.complete_handshake()?;
        }
        Ok(())
    }

    /// Consume a peer handshake message. Fails fast when the peer's static
    /// key is already known and mismatched.
    pub fn receive_handshake_message(&mut self, msg: &[u8]) -> Result<(), NoiseError> {
        if msg.is_empty() || msg.len() > 65535 {
            return Err(NoiseError::Handshake);
        }
        let State::Hand(hand) = &mut self.state else {
            return Err(NoiseError::Handshake);
        };
        let mut buf = vec![0_u8; 65535];
        hand.state
            .read_message(msg, &mut buf)
            .map_err(|_| NoiseError::Handshake)?;
        hand.received = hand.received.saturating_add(1);
        if let Some(remote) = hand.state.get_remote_static()
            && remote.len() == 32
            && let Some(expected) = self.expected_peer_static
            && !static_eq(remote, &expected)
        {
            self.state = State::Closed;
            return Err(NoiseError::PeerStatic);
        }
        if hand.state.is_handshake_finished() {
            self.complete_handshake()?;
        }
        Ok(())
    }

    fn complete_handshake(&mut self) -> Result<(), NoiseError> {
        let State::Hand(hand) = &self.state else {
            return Err(NoiseError::Handshake);
        };
        let remote = hand.state.get_remote_static().ok_or(NoiseError::PeerStatic)?;
        if remote.len() != 32 {
            self.state = State::Closed;
            return Err(NoiseError::PeerStatic);
        }
        let mut peer = [0_u8; 32];
        peer.copy_from_slice(remote);
        if let Some(expected) = self.expected_peer_static
            && !static_eq(remote, &expected)
        {
            self.state = State::Closed;
            return Err(NoiseError::PeerStatic);
        }
        let hash = hand.state.get_handshake_hash().to_vec();
        let State::Hand(hand) =
            std::mem::replace(&mut self.state, State::Closed)
        else {
            return Err(NoiseError::Handshake);
        };
        let transport = hand
            .state
            .into_transport_mode()
            .map_err(|_| NoiseError::Handshake)?;
        self.state = State::Open(Open {
            transport,
            handshake_hash: hash,
            peer_static: peer,
            confirm_sent: false,
            confirm_received: false,
            sent_count: 0,
            recv_count: 0,
        });
        Ok(())
    }

    /// Emit the initiator key confirmation carrying the Core grant JWT:
    /// `hash(64) || jwt_len(u16be) || jwt || sig(64)`. Returns the
    /// ciphertext to send as the Relay data body.
    pub fn emit_initiator_confirm(
        &mut self,
        own_cnf_secret: &[u8; 32],
        grant_jwt: &[u8],
    ) -> Result<Vec<u8>, NoiseError> {
        if grant_jwt.is_empty() || grant_jwt.len() > MAX_GRANT_JWT {
            return Err(NoiseError::Confirm);
        }
        let channel = self.ctx.channel.clone();
        let State::Open(open) = &mut self.state else {
            return Err(NoiseError::NotReady);
        };
        if open.confirm_sent {
            return Err(NoiseError::Confirm);
        }
        let signature = bind_sign(own_cnf_secret, ROLE_INITIATOR, &open.handshake_hash)?;
        let mut body = open.handshake_hash.clone();
        body.extend_from_slice(
            &u16::try_from(grant_jwt.len()).map_err(|_| NoiseError::Confirm)?.to_be_bytes(),
        );
        body.extend_from_slice(grant_jwt);
        body.extend_from_slice(&signature);
        let framed = frame(KIND_CONFIRM, &channel, &body)?;
        let mut buf = vec![0_u8; framed.len() + 16];
        let len = open
            .transport
            .write_message(&framed, &mut buf)
            .map_err(|_| NoiseError::Confirm)?;
        buf.truncate(len);
        open.sent_count = open.sent_count.saturating_add(1);
        open.confirm_sent = true;
        Ok(buf)
    }

    /// Consume the initiator's confirmation: decrypts, checks the hash and
    /// JWT framing, and returns the grant JWT plus its binding signature.
    /// The caller verifies the JWT itself (Core JWKS) and completes with
    /// [`NoiseChannel::accept_initiator_confirm`]. Single-shot: any failure
    /// destroys the session.
    pub fn receive_initiator_confirm(
        &mut self,
        ct: &[u8],
    ) -> Result<InitiatorConfirm, NoiseError> {
        if ct.is_empty() || ct.len() > 65535 {
            self.destroy();
            return Err(NoiseError::Confirm);
        }
        if !matches!(self.state, State::Open(_)) {
            return Err(NoiseError::NotReady);
        }
        let outcome = self.receive_initiator_confirm_inner(ct);
        if outcome.is_err() {
            self.destroy();
        }
        outcome
    }

    fn receive_initiator_confirm_inner(
        &mut self,
        ct: &[u8],
    ) -> Result<InitiatorConfirm, NoiseError> {
        let channel = self.ctx.channel.clone();
        let State::Open(open) = &mut self.state else {
            return Err(NoiseError::NotReady);
        };
        let mut buf = vec![0_u8; ct.len().saturating_add(65535)];
        let len = open
            .transport
            .read_message(ct, &mut buf)
            .map_err(|_| NoiseError::Decrypt)?;
        buf.truncate(len);
        let (kind, body) = parse_frame(&buf, &channel)?;
        if kind != KIND_CONFIRM {
            return Err(NoiseError::Confirm);
        }
        if body.len() < HANDSHAKE_HASH_LEN + 2 + 16 + SIGNATURE_LEN {
            return Err(NoiseError::Confirm);
        }
        if body[..HANDSHAKE_HASH_LEN] != open.handshake_hash {
            return Err(NoiseError::Confirm);
        }
        let jwt_len =
            usize::from(u16::from_be_bytes([body[64], body[65]]));
        if jwt_len == 0
            || jwt_len > MAX_GRANT_JWT
            || HANDSHAKE_HASH_LEN + 2 + jwt_len + SIGNATURE_LEN != body.len()
        {
            return Err(NoiseError::Confirm);
        }
        let jwt = body[66..66 + jwt_len].to_vec();
        let mut signature = [0_u8; SIGNATURE_LEN];
        signature.copy_from_slice(&body[66 + jwt_len..]);
        open.recv_count = open.recv_count.saturating_add(1);
        Ok(InitiatorConfirm { jwt, signature })
    }

    /// Complete the initiator confirmation with the independently verified
    /// grant `cnf` key: checks the role-0x01 binding signature, then marks
    /// the peer confirmed.
    pub fn accept_initiator_confirm(
        &mut self,
        peer_cnf_public: &[u8; 32],
        confirm: &InitiatorConfirm,
    ) -> Result<(), NoiseError> {
        let State::Open(open) = &mut self.state else {
            return Err(NoiseError::NotReady);
        };
        if open.confirm_received {
            self.destroy();
            return Err(NoiseError::Confirm);
        }
        if !bind_verify(
            peer_cnf_public,
            ROLE_INITIATOR,
            &open.handshake_hash,
            &confirm.signature,
        ) {
            self.destroy();
            return Err(NoiseError::Confirm);
        }
        open.confirm_received = true;
        Ok(())
    }

    /// Emit the responder key confirmation:
    /// `hash(64) || sig(64)` with the enrolled machine key (role 0x02).
    pub fn emit_responder_confirm(
        &mut self,
        own_cnf_secret: &[u8; 32],
    ) -> Result<Vec<u8>, NoiseError> {
        let channel = self.ctx.channel.clone();
        let State::Open(open) = &mut self.state else {
            return Err(NoiseError::NotReady);
        };
        if open.confirm_sent {
            return Err(NoiseError::Confirm);
        }
        let signature = bind_sign(own_cnf_secret, ROLE_RESPONDER, &open.handshake_hash)?;
        let mut body = open.handshake_hash.clone();
        body.extend_from_slice(&signature);
        let framed = frame(KIND_CONFIRM, &channel, &body)?;
        let mut buf = vec![0_u8; framed.len() + 16];
        let len = open
            .transport
            .write_message(&framed, &mut buf)
            .map_err(|_| NoiseError::Confirm)?;
        buf.truncate(len);
        open.sent_count = open.sent_count.saturating_add(1);
        open.confirm_sent = true;
        Ok(buf)
    }

    /// Consume the responder's confirmation, checking hash equality and the
    /// role-0x02 binding signature against the Forge enrolled key from the
    /// authenticated Core machine record. Single-shot.
    pub fn receive_responder_confirm(
        &mut self,
        ct: &[u8],
        forge_enrolled_public: &[u8; 32],
    ) -> Result<(), NoiseError> {
        if ct.is_empty() || ct.len() > 65535 {
            self.destroy();
            return Err(NoiseError::Confirm);
        }
        if !matches!(self.state, State::Open(_)) {
            return Err(NoiseError::NotReady);
        }
        let outcome = self.receive_responder_confirm_inner(ct, forge_enrolled_public);
        if outcome.is_err() {
            self.destroy();
        }
        outcome
    }

    fn receive_responder_confirm_inner(
        &mut self,
        ct: &[u8],
        forge_enrolled_public: &[u8; 32],
    ) -> Result<(), NoiseError> {
        let channel = self.ctx.channel.clone();
        let State::Open(open) = &mut self.state else {
            return Err(NoiseError::NotReady);
        };
        if open.confirm_received {
            return Err(NoiseError::Confirm);
        }
        let mut buf = vec![0_u8; ct.len().saturating_add(65535)];
        let len = open
            .transport
            .read_message(ct, &mut buf)
            .map_err(|_| NoiseError::Decrypt)?;
        buf.truncate(len);
        let (kind, body) = parse_frame(&buf, &channel)?;
        if kind != KIND_CONFIRM {
            return Err(NoiseError::Confirm);
        }
        if body.len() != HANDSHAKE_HASH_LEN + SIGNATURE_LEN
            || body[..HANDSHAKE_HASH_LEN] != open.handshake_hash
        {
            return Err(NoiseError::Confirm);
        }
        if !bind_verify(
            forge_enrolled_public,
            ROLE_RESPONDER,
            &open.handshake_hash,
            &body[HANDSHAKE_HASH_LEN..],
        ) {
            return Err(NoiseError::Confirm);
        }
        open.recv_count = open.recv_count.saturating_add(1);
        open.confirm_received = true;
        Ok(())
    }

    /// Seal app data. Requires an open session; rotation bound enforced.
    pub fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, NoiseError> {
        if plaintext.len() > MAX_APP_PLAINTEXT {
            return Err(NoiseError::TooLarge);
        }
        let State::Open(open) = &mut self.state else {
            return Err(NoiseError::NotOpen);
        };
        if !open.confirm_sent || !open.confirm_received {
            return Err(NoiseError::NotOpen);
        }
        if open.sent_count.saturating_add(open.recv_count) >= ROTATE_AFTER_MESSAGES {
            return Err(NoiseError::NotOpen);
        }
        let channel = self.ctx.channel.clone();
        let framed = frame(KIND_DATA, &channel, plaintext)?;
        let mut buf = vec![0_u8; framed.len() + 16];
        let len = open
            .transport
            .write_message(&framed, &mut buf)
            .map_err(|_| NoiseError::Closed)?;
        buf.truncate(len);
        open.sent_count = open.sent_count.saturating_add(1);
        Ok(buf)
    }

    /// Open one data message. DATA before confirmation completes, an
    /// unexpected CONFIRM, or any AEAD failure destroys the session.
    pub fn receive_data(&mut self, ct: &[u8]) -> Result<Vec<u8>, NoiseError> {
        if ct.is_empty() || ct.len() > 65535 {
            self.destroy();
            return Err(NoiseError::Decrypt);
        }
        if !matches!(self.state, State::Open(_)) {
            return Err(NoiseError::NotOpen);
        }
        let outcome = self.receive_data_inner(ct);
        if outcome.is_err() {
            self.destroy();
        }
        outcome
    }

    fn receive_data_inner(&mut self, ct: &[u8]) -> Result<Vec<u8>, NoiseError> {
        let channel = self.ctx.channel.clone();
        let State::Open(open) = &mut self.state else {
            return Err(NoiseError::NotOpen);
        };
        let mut buf = vec![0_u8; ct.len().saturating_add(65535)];
        let len = open
            .transport
            .read_message(ct, &mut buf)
            .map_err(|_| NoiseError::Decrypt)?;
        buf.truncate(len);
        let (kind, body) = parse_frame(&buf, &channel)?;
        if kind != KIND_DATA {
            return Err(NoiseError::Confirm);
        }
        if !open.confirm_sent || !open.confirm_received {
            return Err(NoiseError::NotOpen);
        }
        open.recv_count = open.recv_count.saturating_add(1);
        Ok(body)
    }

    /// Whether the session must rotate (new channel + handshake).
    #[must_use]
    pub fn needs_rotation(&self) -> bool {
        match &self.state {
            State::Open(open) => {
                open.sent_count.saturating_add(open.recv_count) >= ROTATE_AFTER_MESSAGES
                    || now_ms().saturating_sub(self.born_ms) >= MAX_SESSION_AGE_MS
            }
            State::Hand(_) | State::Closed => false,
        }
    }

    /// Fail-closed teardown. Drops transport keys.
    pub fn destroy(&mut self) {
        self.state = State::Closed;
    }
}

fn static_eq(a: &[u8], b: &[u8; 32]) -> bool {
    if a.len() != 32 {
        return false;
    }
    let mut diff = 0_u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "test fixtures; failures fail the test"
)]
mod tests {
    use super::*;

    fn ctx() -> ChannelCtx {
        ChannelCtx {
            account: "acct_00000000000000000000000000".into(),
            machine: "mch_11111111111111111111111111".into(),
            channel: "exec-1".into(),
        }
    }

    fn statics() -> ([u8; 32], [u8; 32], [u8; 32], [u8; 32]) {
        let init_keys = StaticKeypair::from_secret([0x11_u8; 32]).unwrap();
        let resp_keys = StaticKeypair::from_secret([0x22_u8; 32]).unwrap();
        let init_pub = *init_keys.public();
        let resp_pub = *resp_keys.public();
        ([0x11_u8; 32], init_pub, [0x22_u8; 32], resp_pub)
    }

    const CNF_INIT: [u8; 32] = [0x33_u8; 32];
    const CNF_RESP: [u8; 32] = [0x44_u8; 32];
    const GRANT_JWT: &[u8] = b"test-grant.jwt.body";

    fn cnf_public(secret: &[u8; 32]) -> [u8; 32] {
        ed25519_dalek::SigningKey::from_bytes(secret)
            .verifying_key()
            .to_bytes()
    }

    fn open_pair() -> (NoiseChannel, NoiseChannel) {
        let (init_sec, init_pub, resp_sec, resp_pub) = statics();
        let mut init = NoiseChannel::initiator(&init_sec, Some(resp_pub), ctx()).unwrap();
        let mut resp = NoiseChannel::responder(&resp_sec, Some(init_pub), ctx()).unwrap();
        let mut m1 = Vec::new();
        init.next_handshake_message(&mut m1).unwrap();
        resp.receive_handshake_message(&m1).unwrap();
        let mut m2 = Vec::new();
        resp.next_handshake_message(&mut m2).unwrap();
        init.receive_handshake_message(&m2).unwrap();
        let mut m3 = Vec::new();
        init.next_handshake_message(&mut m3).unwrap();
        resp.receive_handshake_message(&m3).unwrap();
        assert_eq!(init.phase(), Phase::Confirm);
        assert_eq!(resp.phase(), Phase::Confirm);
        let c1 = init.emit_initiator_confirm(&CNF_INIT, GRANT_JWT).unwrap();
        let parsed = resp.receive_initiator_confirm(&c1).unwrap();
        assert_eq!(parsed.jwt, GRANT_JWT);
        resp.accept_initiator_confirm(&cnf_public(&CNF_INIT), &parsed)
            .unwrap();
        let c2 = resp.emit_responder_confirm(&CNF_RESP).unwrap();
        init.receive_responder_confirm(&c2, &cnf_public(&CNF_RESP))
            .unwrap();
        assert_eq!(init.phase(), Phase::Open);
        assert_eq!(resp.phase(), Phase::Open);
        (init, resp)
    }

    #[test]
    fn loopback_opens_and_carries_data_both_ways() {
        let (mut init, mut resp) = open_pair();
        let ct = init.seal(b"run this").unwrap();
        assert_eq!(resp.receive_data(&ct).unwrap(), b"run this");
        let back = resp.seal(b"done").unwrap();
        assert_eq!(init.receive_data(&back).unwrap(), b"done");
    }

    #[test]
    fn data_before_confirmation_is_rejected() {
        let (init_sec, _, resp_sec, resp_pub) = statics();
        let mut init = NoiseChannel::initiator(&init_sec, Some(resp_pub), ctx()).unwrap();
        let init_pub = *StaticKeypair::from_secret(init_sec).unwrap().public();
        let mut resp = NoiseChannel::responder(&resp_sec, Some(init_pub), ctx()).unwrap();
        let mut m1 = Vec::new();
        init.next_handshake_message(&mut m1).unwrap();
        resp.receive_handshake_message(&m1).unwrap();
        let mut m2 = Vec::new();
        resp.next_handshake_message(&mut m2).unwrap();
        init.receive_handshake_message(&mut m2).unwrap();
        let mut m3 = Vec::new();
        init.next_handshake_message(&mut m3).unwrap();
        resp.receive_handshake_message(&m3).unwrap();
        assert_eq!(init.seal(b"x").unwrap_err(), NoiseError::NotOpen);
        assert_eq!(resp.seal(b"x").unwrap_err(), NoiseError::NotOpen);
    }

    #[test]
    fn peer_static_mismatch_aborts_before_open() {
        let (init_sec, init_pub, resp_sec, _) = statics();
        let wrong = [0x99_u8; 32];
        let mut init = NoiseChannel::initiator(&init_sec, Some(wrong), ctx()).unwrap();
        let mut resp = NoiseChannel::responder(&resp_sec, Some(init_pub), ctx()).unwrap();
        let mut m1 = Vec::new();
        init.next_handshake_message(&mut m1).unwrap();
        resp.receive_handshake_message(&m1).unwrap();
        let mut m2 = Vec::new();
        resp.next_handshake_message(&mut m2).unwrap();
        assert_eq!(
            init.receive_handshake_message(&m2).unwrap_err(),
            NoiseError::PeerStatic
        );
        assert_eq!(init.phase(), Phase::Closed);
    }

    #[test]
    fn replay_destroys_the_session() {
        let (mut init, mut resp) = open_pair();
        let ct = init.seal(b"once").unwrap();
        resp.receive_data(&ct).unwrap();
        assert_eq!(resp.receive_data(&ct).unwrap_err(), NoiseError::Decrypt);
        assert_eq!(resp.phase(), Phase::Closed);
    }

    #[test]
    fn tampered_confirm_is_rejected() {
        let (init_sec, init_pub, resp_sec, resp_pub) = statics();
        let mut init = NoiseChannel::initiator(&init_sec, Some(resp_pub), ctx()).unwrap();
        let mut resp = NoiseChannel::responder(&resp_sec, Some(init_pub), ctx()).unwrap();
        let mut m1 = Vec::new();
        init.next_handshake_message(&mut m1).unwrap();
        resp.receive_handshake_message(&m1).unwrap();
        let mut m2 = Vec::new();
        resp.next_handshake_message(&mut m2).unwrap();
        init.receive_handshake_message(&m2).unwrap();
        let mut m3 = Vec::new();
        init.next_handshake_message(&mut m3).unwrap();
        resp.receive_handshake_message(&m3).unwrap();
        let mut c1 = init.emit_initiator_confirm(&CNF_INIT, GRANT_JWT).unwrap();
        let last = c1.len() - 1;
        c1[last] ^= 0x01;
        assert!(resp.receive_initiator_confirm(&c1).is_err());
        assert_eq!(resp.phase(), Phase::Closed);
    }

    #[test]
    fn wrong_binding_key_is_rejected() {
        let (init_sec, init_pub, resp_sec, resp_pub) = statics();
        let mut init = NoiseChannel::initiator(&init_sec, Some(resp_pub), ctx()).unwrap();
        let mut resp = NoiseChannel::responder(&resp_sec, Some(init_pub), ctx()).unwrap();
        let mut m1 = Vec::new();
        init.next_handshake_message(&mut m1).unwrap();
        resp.receive_handshake_message(&m1).unwrap();
        let mut m2 = Vec::new();
        resp.next_handshake_message(&mut m2).unwrap();
        init.receive_handshake_message(&m2).unwrap();
        let mut m3 = Vec::new();
        init.next_handshake_message(&mut m3).unwrap();
        resp.receive_handshake_message(&m3).unwrap();
        let c1 = init.emit_initiator_confirm(&CNF_INIT, GRANT_JWT).unwrap();
        let parsed = resp.receive_initiator_confirm(&c1).unwrap();
        assert_eq!(
            resp.accept_initiator_confirm(&[0x09_u8; 32], &parsed)
                .unwrap_err(),
            NoiseError::Confirm
        );
        assert_eq!(resp.phase(), Phase::Closed);
    }

    #[test]
    fn prologue_frame_and_binding_vectors_are_stable() {
        let prologue = prologue_bytes(&ctx());
        assert_eq!(
            prologue,
            b"ast-relay-noise-v1\0acct_00000000000000000000000000\0mch_11111111111111111111111111\0exec-1"
                .to_vec()
        );
        let framed = frame(KIND_DATA, "exec-1", b"hi").unwrap();
        let (kind, body) = parse_frame(&framed, "exec-1").unwrap();
        assert_eq!((kind, body), (KIND_DATA, b"hi".to_vec()));
        assert_eq!(parse_frame(&framed, "other").unwrap_err(), NoiseError::Channel);
        let hash = [0xAB_u8; 64];
        let message = bind_message(ROLE_INITIATOR, &hash).unwrap();
        assert_eq!(message[..BIND_DOMAIN.len()], *BIND_DOMAIN.as_bytes());
        assert_eq!(message[BIND_DOMAIN.len()], ROLE_INITIATOR);
        assert_eq!(message[BIND_DOMAIN.len() + 1..], hash);
        assert!(bind_message(ROLE_INITIATOR, &[0_u8; 32]).is_err());
    }

    #[test]
    fn binding_ties_sessions_to_core_identity() {
        let hash = [0xAB_u8; 64];
        let ed = [0x55_u8; 32];
        let public = cnf_public(&ed);
        let signature = bind_sign(&ed, ROLE_INITIATOR, &hash).unwrap();
        assert!(bind_verify(&public, ROLE_INITIATOR, &hash, &signature));
        assert!(!bind_verify(&public, ROLE_RESPONDER, &hash, &signature));
        assert!(!bind_verify(&public, ROLE_INITIATOR, &[0_u8; 64], &signature));
    }
}

/// Byte-exact cross-language vectors (`relay/contracts/fixtures/noise-xx.json`).
/// Fixed statics + fixed ephemerals reproduce every handshake byte, the
/// handshake hash, both confirms, and both data directions exactly.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "test fixtures; failures fail the test"
)]
mod relay_fixture_tests {
    use super::*;

    fn fixture_path() -> std::path::PathBuf {
        std::env::var("NOISE_FIXTURE_PATH")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("..")
                    .join("..")
                    .join("..")
                    .join("relay/contracts/fixtures/noise-xx.json")
            })
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn relay_vectors_reproduce_byte_exact() {
        let raw = std::fs::read_to_string(fixture_path()).unwrap();
        let fixture: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(fixture["protocol"], NOISE_PROTOCOL);
        let ctx = ChannelCtx {
            account: "fixture-acct".into(),
            machine: "fixture-forge".into(),
            channel: "fixture-ch".into(),
        };
        assert_eq!(hex(&prologue_bytes(&ctx)), fixture["prologue_hex"].as_str().unwrap());

        let init_static = [0x11_u8; 32];
        let resp_static = [0x22_u8; 32];
        let init_pub = *StaticKeypair::from_secret(init_static).unwrap().public();
        let resp_pub = *StaticKeypair::from_secret(resp_static).unwrap().public();
        assert_eq!(hex(&init_pub), fixture["init_static_pub"].as_str().unwrap());
        assert_eq!(hex(&resp_pub), fixture["resp_static_pub"].as_str().unwrap());

        let mut init = NoiseChannel::initiator_with_ephemeral(
            &init_static,
            Some(resp_pub),
            ctx.clone(),
            [0x33_u8; 32],
        )
        .unwrap();
        let mut resp = NoiseChannel::responder_with_ephemeral(
            &resp_static,
            Some(init_pub),
            ctx.clone(),
            [0x44_u8; 32],
        )
        .unwrap();
        let mut m1 = Vec::new();
        init.next_handshake_message(&mut m1).unwrap();
        assert_eq!(hex(&m1), fixture["m1"].as_str().unwrap());
        resp.receive_handshake_message(&m1).unwrap();
        let mut m2 = Vec::new();
        resp.next_handshake_message(&mut m2).unwrap();
        assert_eq!(hex(&m2), fixture["m2"].as_str().unwrap());
        init.receive_handshake_message(&m2).unwrap();
        let mut m3 = Vec::new();
        init.next_handshake_message(&mut m3).unwrap();
        assert_eq!(hex(&m3), fixture["m3"].as_str().unwrap());
        resp.receive_handshake_message(&m3).unwrap();
        assert_eq!(init.phase(), Phase::Confirm);
        assert_eq!(resp.phase(), Phase::Confirm);

        // Initiator confirm carries the fixture grant JWT.
        let cnf_init = [0x55_u8; 32];
        let cnf_resp = [0x66_u8; 32];
        let grant_jwt = fixture["grant_jwt"].as_str().unwrap().as_bytes();
        let c1 = init.emit_initiator_confirm(&cnf_init, grant_jwt).unwrap();
        assert_eq!(hex(&c1), fixture["confirm_init_ct"].as_str().unwrap());

        // Responder verifies the fixture grant itself (JWKS from fixture).
        let parsed = resp.receive_initiator_confirm(&c1).unwrap();
        assert_eq!(parsed.jwt, grant_jwt);
        let jwks_x = fixture["jwks"]["keys"][0]["x"].as_str().unwrap();
        let issuer = {
            use base64::Engine as _;
            let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(jwks_x.as_bytes())
                .unwrap();
            <[u8; 32]>::try_from(raw).unwrap()
        };
        let params = crate::grant::VerifyParams {
            expected_iss: "https://core.test/ast".into(),
            expected_aud: crate::grant::AUD_RELAY.into(),
            max_ttl_s: crate::GRANT_MAX_TTL_S,
            expected_sub: "fixture-acct".into(),
            expected_mid: "fixture-forge".into(),
            now_sec: 2_000_000_000,
        };
        let claims =
            crate::grant::parse_and_verify_jwt(std::str::from_utf8(&parsed.jwt).unwrap(), &issuer, &params)
                .unwrap();
        assert_eq!(claims.role, crate::grant::Role::Client);
        let cnf = crate::grant::cnf_public_key(&claims).unwrap();
        assert_eq!(hex(&cnf), fixture["init_cnf_pub"].as_str().unwrap());
        resp.accept_initiator_confirm(&cnf, &parsed).unwrap();

        let c2 = resp.emit_responder_confirm(&cnf_resp).unwrap();
        assert_eq!(hex(&c2), fixture["confirm_resp_ct"].as_str().unwrap());
        let forge_enrolled = {
            let key = ed25519_dalek::SigningKey::from_bytes(&cnf_resp);
            key.verifying_key().to_bytes()
        };
        assert_eq!(hex(&forge_enrolled), fixture["resp_cnf_pub"].as_str().unwrap());
        init.receive_responder_confirm(&c2, &forge_enrolled).unwrap();
        assert_eq!(init.phase(), Phase::Open);
        assert_eq!(resp.phase(), Phase::Open);

        // Data both directions, byte-exact.
        let ct_init = init.seal(fixture["plaintext_a"].as_str().unwrap().as_bytes()).unwrap();
        assert_eq!(hex(&ct_init), fixture["data_init_ct"].as_str().unwrap());
        assert_eq!(resp.receive_data(&ct_init).unwrap(), fixture["plaintext_a"].as_str().unwrap().as_bytes());
        let ct_resp = resp.seal(fixture["plaintext_b"].as_str().unwrap().as_bytes()).unwrap();
        assert_eq!(hex(&ct_resp), fixture["data_resp_ct"].as_str().unwrap());
        assert_eq!(init.receive_data(&ct_resp).unwrap(), fixture["plaintext_b"].as_str().unwrap().as_bytes());
    }
}
