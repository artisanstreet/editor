//! Noise XX channel with the Relay's application framing and identity
//! binding (`relay/contracts/e2e.md`); byte-exact with `fixtures/noise-xx.json`.

use anyhow::{Context as _, Result, bail, ensure};

const NOISE_PROTOCOL: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2b";
const PROLOGUE_DOMAIN: &str = "ast-relay-noise-v1";
const BIND_DOMAIN: &str = "ast-relay-bind-v1";
const ROLE_INITIATOR: u8 = 0x01;
const ROLE_RESPONDER: u8 = 0x02;
const APP_MAGIC: &[u8] = b"AR1";
const APP_VERSION: u8 = 0x01;
const KIND_CONFIRM: u8 = 0x00;
const KIND_DATA: u8 = 0x01;
const MAX_APP_PLAINTEXT: usize = 60_000;
const MAX_GRANT_JWT: usize = 8192;
const HASH_LEN: usize = 64;
const MAX_MESSAGE: usize = 65_535;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelCtx {
    pub account: String,
    pub machine: String,
    pub channel: String,
}

fn prologue_bytes(ctx: &ChannelCtx) -> Vec<u8> {
    [
        PROLOGUE_DOMAIN.as_bytes(),
        b"\0",
        ctx.account.as_bytes(),
        b"\0",
        ctx.machine.as_bytes(),
        b"\0",
        ctx.channel.as_bytes(),
    ]
    .concat()
}

fn frame(kind: u8, channel: &str, body: &[u8]) -> Result<Vec<u8>> {
    let channel_len = u8::try_from(channel.len())
        .ok()
        .filter(|len| (1..=64).contains(len))
        .context("bad Noise channel name")?;
    Ok([
        APP_MAGIC,
        &[APP_VERSION, channel_len],
        channel.as_bytes(),
        &[kind],
        body,
    ]
    .concat())
}

fn parse_frame<'a>(plaintext: &'a [u8], channel: &str) -> Result<(u8, &'a [u8])> {
    let rest = plaintext
        .strip_prefix(APP_MAGIC)
        .and_then(|rest| rest.strip_prefix(&[APP_VERSION]))
        .context("bad Noise frame header")?;
    let (&channel_len, rest) = rest.split_first().context("short Noise frame")?;
    let (name, rest) = rest
        .split_at_checked(usize::from(channel_len))
        .context("short Noise frame")?;
    ensure!(
        name == channel.as_bytes(),
        "Noise frame for another channel"
    );
    let (&kind, body) = rest.split_first().context("short Noise frame")?;
    ensure!(
        kind == KIND_CONFIRM || kind == KIND_DATA,
        "unknown Noise frame kind"
    );
    Ok((kind, body))
}

fn bind_message(role: u8, handshake_hash: &[u8]) -> Vec<u8> {
    [BIND_DOMAIN.as_bytes(), &[role], handshake_hash].concat()
}

fn bind_sign(ed_secret: &[u8; 32], role: u8, handshake_hash: &[u8]) -> [u8; 64] {
    use ed25519_dalek::Signer as _;
    ed25519_dalek::SigningKey::from_bytes(ed_secret)
        .sign(&bind_message(role, handshake_hash))
        .to_bytes()
}

fn bind_verify(ed_public: &[u8; 32], role: u8, handshake_hash: &[u8], signature: &[u8]) -> bool {
    let (Ok(key), Ok(signature)) = (
        ed25519_dalek::VerifyingKey::from_bytes(ed_public),
        ed25519_dalek::Signature::try_from(signature),
    ) else {
        return false;
    };
    key.verify_strict(&bind_message(role, handshake_hash), &signature)
        .is_ok()
}

pub struct InitiatorConfirm {
    pub jwt: Vec<u8>,
    signature: [u8; 64],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Handshake,
    Confirm,
    Open,
    Closed,
}

struct Open {
    transport: snow::TransportState,
    handshake_hash: Vec<u8>,
    confirm_sent: bool,
    confirm_received: bool,
}

impl Open {
    const fn confirmed(&self) -> bool {
        self.confirm_sent && self.confirm_received
    }

    fn seal(&mut self, channel: &str, kind: u8, body: &[u8]) -> Result<Vec<u8>> {
        let framed = frame(kind, channel, body)?;
        let mut buf = vec![0_u8; framed.len() + 16];
        let len = self
            .transport
            .write_message(&framed, &mut buf)
            .context("encrypt a Noise message")?;
        buf.truncate(len);
        Ok(buf)
    }

    fn open(&mut self, channel: &str, ciphertext: &[u8]) -> Result<(u8, Vec<u8>)> {
        let mut buf = vec![0_u8; ciphertext.len()];
        let len = self
            .transport
            .read_message(ciphertext, &mut buf)
            .context("decrypt a Noise message")?;
        let (kind, body) = parse_frame(&buf[..len], channel)?;
        Ok((kind, body.to_vec()))
    }
}

enum State {
    Handshake(Box<snow::HandshakeState>),
    Open(Box<Open>),
    Closed,
}

/// One ordered E2E channel. Identity binds at the confirm step, so callers
/// pass `None` for the peer static; `Some` additionally pins the peer's
/// Noise key and aborts the handshake on mismatch.
pub struct NoiseChannel {
    state: State,
    channel: String,
    expected_peer_static: Option<[u8; 32]>,
}

impl NoiseChannel {
    pub fn initiator(
        static_secret: &[u8; 32],
        peer_static: Option<[u8; 32]>,
        ctx: ChannelCtx,
    ) -> Result<Self> {
        Self::build(true, static_secret, peer_static, ctx, None)
    }

    pub fn responder(
        static_secret: &[u8; 32],
        peer_static: Option<[u8; 32]>,
        ctx: ChannelCtx,
    ) -> Result<Self> {
        Self::build(false, static_secret, peer_static, ctx, None)
    }

    fn build(
        initiator: bool,
        static_secret: &[u8; 32],
        expected_peer_static: Option<[u8; 32]>,
        ctx: ChannelCtx,
        fixed_ephemeral: Option<&[u8; 32]>,
    ) -> Result<Self> {
        let prologue = prologue_bytes(&ctx);
        let mut builder =
            snow::Builder::new(NOISE_PROTOCOL.parse().context("read the Noise protocol")?)
                .prologue(&prologue)
                .context("set the Noise prologue")?
                .local_private_key(static_secret)
                .context("set the Noise static key")?;
        if let Some(ephemeral) = fixed_ephemeral {
            builder = builder.fixed_ephemeral_key_for_testing_only(ephemeral);
        }
        let state = if initiator {
            builder.build_initiator()
        } else {
            builder.build_responder()
        }
        .context("start the Noise handshake")?;
        Ok(Self {
            state: State::Handshake(Box::new(state)),
            channel: ctx.channel,
            expected_peer_static,
        })
    }

    #[must_use]
    pub fn phase(&self) -> Phase {
        match &self.state {
            State::Handshake(_) => Phase::Handshake,
            State::Open(open) if open.confirmed() => Phase::Open,
            State::Open(_) => Phase::Confirm,
            State::Closed => Phase::Closed,
        }
    }

    /// Writes our next handshake message; fails when it is the peer's turn.
    pub fn next_handshake_message(&mut self, out: &mut Vec<u8>) -> Result<()> {
        let State::Handshake(hand) = &mut self.state else {
            bail!("the Noise handshake is over");
        };
        let mut buf = vec![0_u8; MAX_MESSAGE];
        let len = hand
            .write_message(&[], &mut buf)
            .context("write a Noise handshake message")?;
        out.extend_from_slice(&buf[..len]);
        self.finish_handshake()
    }

    pub fn receive_handshake_message(&mut self, message: &[u8]) -> Result<()> {
        let State::Handshake(hand) = &mut self.state else {
            bail!("the Noise handshake is over");
        };
        let mut buf = vec![0_u8; message.len()];
        hand.read_message(message, &mut buf)
            .context("read a Noise handshake message")?;
        if let Some(expected) = self.expected_peer_static
            && hand
                .get_remote_static()
                .is_some_and(|remote| remote != expected.as_slice())
        {
            self.state = State::Closed;
            bail!("the peer's Noise key is not the expected one");
        }
        self.finish_handshake()
    }

    fn finish_handshake(&mut self) -> Result<()> {
        let State::Handshake(hand) = &self.state else {
            return Ok(());
        };
        if !hand.is_handshake_finished() {
            return Ok(());
        }
        let State::Handshake(hand) = std::mem::replace(&mut self.state, State::Closed) else {
            return Ok(());
        };
        let handshake_hash = hand.get_handshake_hash().to_vec();
        let transport = (*hand)
            .into_transport_mode()
            .context("finish the Noise handshake")?;
        self.state = State::Open(Box::new(Open {
            transport,
            handshake_hash,
            confirm_sent: false,
            confirm_received: false,
        }));
        Ok(())
    }

    fn open_mut(&mut self) -> Result<(&mut Open, &str)> {
        match &mut self.state {
            State::Open(open) => Ok((open, &self.channel)),
            State::Handshake(_) | State::Closed => bail!("the Noise session is not open"),
        }
    }

    /// Runs one receive step; any failure closes the session for good.
    fn closing_on_error<T>(
        &mut self,
        step: impl FnOnce(&mut Open, &str) -> Result<T>,
    ) -> Result<T> {
        let (open, channel) = self.open_mut()?;
        let outcome = step(open, channel);
        if outcome.is_err() {
            self.state = State::Closed;
        }
        outcome
    }

    /// `handshake_hash || jwt_len(u16be) || grant_jwt || bind_sig`.
    pub fn emit_initiator_confirm(
        &mut self,
        own_cnf_secret: &[u8; 32],
        grant_jwt: &[u8],
    ) -> Result<Vec<u8>> {
        let jwt_len = u16::try_from(grant_jwt.len())
            .ok()
            .filter(|len| (1..=MAX_GRANT_JWT).contains(&usize::from(*len)))
            .context("the grant is empty or too large for the confirm")?;
        let (open, channel) = self.open_mut()?;
        ensure!(!open.confirm_sent, "the initiator confirm was already sent");
        let signature = bind_sign(own_cnf_secret, ROLE_INITIATOR, &open.handshake_hash);
        let body = [
            open.handshake_hash.as_slice(),
            &jwt_len.to_be_bytes(),
            grant_jwt,
            &signature,
        ]
        .concat();
        let sealed = open.seal(channel, KIND_CONFIRM, &body)?;
        open.confirm_sent = true;
        Ok(sealed)
    }

    /// Decrypts the initiator confirm; the caller verifies the grant and
    /// finishes with [`NoiseChannel::accept_initiator_confirm`].
    pub fn receive_initiator_confirm(&mut self, ciphertext: &[u8]) -> Result<InitiatorConfirm> {
        self.closing_on_error(|open, channel| {
            let (kind, body) = open.open(channel, ciphertext)?;
            ensure!(kind == KIND_CONFIRM, "expected the initiator confirm");
            let (hash, rest) = body.split_at_checked(HASH_LEN).context("short confirm")?;
            ensure!(
                hash == open.handshake_hash.as_slice(),
                "the confirm is for another handshake"
            );
            let (len, rest) = rest.split_first_chunk::<2>().context("short confirm")?;
            let jwt_len = usize::from(u16::from_be_bytes(*len));
            ensure!(
                (1..=MAX_GRANT_JWT).contains(&jwt_len),
                "bad grant length in the confirm"
            );
            let (jwt, signature) = rest.split_at_checked(jwt_len).context("short confirm")?;
            Ok(InitiatorConfirm {
                jwt: jwt.to_vec(),
                signature: signature.try_into().context("bad confirm signature")?,
            })
        })
    }

    pub fn accept_initiator_confirm(
        &mut self,
        peer_cnf_public: &[u8; 32],
        confirm: &InitiatorConfirm,
    ) -> Result<()> {
        self.closing_on_error(|open, _| {
            ensure!(!open.confirm_received, "the initiator already confirmed");
            ensure!(
                bind_verify(
                    peer_cnf_public,
                    ROLE_INITIATOR,
                    &open.handshake_hash,
                    &confirm.signature
                ),
                "the initiator confirm is not signed by the grant key"
            );
            open.confirm_received = true;
            Ok(())
        })
    }

    /// `handshake_hash || bind_sig` signed with the enrolled machine key.
    pub fn emit_responder_confirm(&mut self, own_cnf_secret: &[u8; 32]) -> Result<Vec<u8>> {
        let (open, channel) = self.open_mut()?;
        ensure!(!open.confirm_sent, "the responder confirm was already sent");
        let signature = bind_sign(own_cnf_secret, ROLE_RESPONDER, &open.handshake_hash);
        let body = [open.handshake_hash.as_slice(), &signature].concat();
        let sealed = open.seal(channel, KIND_CONFIRM, &body)?;
        open.confirm_sent = true;
        Ok(sealed)
    }

    pub fn receive_responder_confirm(
        &mut self,
        ciphertext: &[u8],
        forge_enrolled_public: &[u8; 32],
    ) -> Result<()> {
        self.closing_on_error(|open, channel| {
            ensure!(!open.confirm_received, "the responder already confirmed");
            let (kind, body) = open.open(channel, ciphertext)?;
            ensure!(kind == KIND_CONFIRM, "expected the responder confirm");
            let (hash, signature) = body.split_at_checked(HASH_LEN).context("short confirm")?;
            ensure!(
                hash == open.handshake_hash.as_slice(),
                "the confirm is for another handshake"
            );
            ensure!(
                bind_verify(
                    forge_enrolled_public,
                    ROLE_RESPONDER,
                    &open.handshake_hash,
                    signature
                ),
                "the Forge confirm is not signed by its enrolled key"
            );
            open.confirm_received = true;
            Ok(())
        })
    }

    pub fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>> {
        ensure!(
            plaintext.len() <= MAX_APP_PLAINTEXT,
            "message too large for one Noise frame"
        );
        let (open, channel) = self.open_mut()?;
        ensure!(open.confirmed(), "the Noise session is not confirmed");
        open.seal(channel, KIND_DATA, plaintext)
    }

    pub fn receive_data(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>> {
        self.closing_on_error(|open, channel| {
            ensure!(open.confirmed(), "the Noise session is not confirmed");
            let (kind, body) = open.open(channel, ciphertext)?;
            ensure!(kind == KIND_DATA, "unexpected confirm on an open session");
            Ok(body)
        })
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::too_many_lines,
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

    fn noise_public(secret: [u8; 32]) -> [u8; 32] {
        x25519_dalek::PublicKey::from(&x25519_dalek::StaticSecret::from(secret)).to_bytes()
    }

    fn cnf_public(secret: &[u8; 32]) -> [u8; 32] {
        ed25519_dalek::SigningKey::from_bytes(secret)
            .verifying_key()
            .to_bytes()
    }

    const INIT_STATIC: [u8; 32] = [0x11; 32];
    const RESP_STATIC: [u8; 32] = [0x22; 32];
    const CNF_INIT: [u8; 32] = [0x33; 32];
    const CNF_RESP: [u8; 32] = [0x44; 32];
    const GRANT_JWT: &[u8] = b"test-grant.jwt.body";

    fn shake(init: &mut NoiseChannel, resp: &mut NoiseChannel) -> Result<()> {
        let mut m1 = Vec::new();
        init.next_handshake_message(&mut m1)?;
        resp.receive_handshake_message(&m1)?;
        let mut m2 = Vec::new();
        resp.next_handshake_message(&mut m2)?;
        init.receive_handshake_message(&m2)?;
        let mut m3 = Vec::new();
        init.next_handshake_message(&mut m3)?;
        resp.receive_handshake_message(&m3)
    }

    fn shaken() -> (NoiseChannel, NoiseChannel) {
        let mut init = NoiseChannel::initiator(&INIT_STATIC, None, ctx()).unwrap();
        let mut resp = NoiseChannel::responder(&RESP_STATIC, None, ctx()).unwrap();
        shake(&mut init, &mut resp).unwrap();
        assert_eq!(
            (init.phase(), resp.phase()),
            (Phase::Confirm, Phase::Confirm),
            "handshake done"
        );
        (init, resp)
    }

    fn open_pair() -> (NoiseChannel, NoiseChannel) {
        let (mut init, mut resp) = shaken();
        let c1 = init.emit_initiator_confirm(&CNF_INIT, GRANT_JWT).unwrap();
        let parsed = resp.receive_initiator_confirm(&c1).unwrap();
        assert_eq!(parsed.jwt, GRANT_JWT, "grant travels in the confirm");
        resp.accept_initiator_confirm(&cnf_public(&CNF_INIT), &parsed)
            .unwrap();
        let c2 = resp.emit_responder_confirm(&CNF_RESP).unwrap();
        init.receive_responder_confirm(&c2, &cnf_public(&CNF_RESP))
            .unwrap();
        assert_eq!(
            (init.phase(), resp.phase()),
            (Phase::Open, Phase::Open),
            "both open"
        );
        (init, resp)
    }

    #[test]
    fn loopback_opens_and_carries_data_both_ways() {
        let (mut init, mut resp) = open_pair();
        let ct = init.seal(b"run this").unwrap();
        assert_eq!(resp.receive_data(&ct).unwrap(), b"run this", "to responder");
        let back = resp.seal(b"done").unwrap();
        assert_eq!(init.receive_data(&back).unwrap(), b"done", "to initiator");
    }

    #[test]
    fn data_before_confirmation_is_rejected() {
        let (mut init, mut resp) = shaken();
        assert!(init.seal(b"x").is_err(), "initiator sealed before confirm");
        assert!(resp.seal(b"x").is_err(), "responder sealed before confirm");
    }

    #[test]
    fn a_pinned_peer_static_mismatch_aborts_the_handshake() {
        let mut init = NoiseChannel::initiator(&INIT_STATIC, Some([0x99; 32]), ctx()).unwrap();
        let mut resp = NoiseChannel::responder(&RESP_STATIC, None, ctx()).unwrap();
        assert!(shake(&mut init, &mut resp).is_err(), "wrong peer accepted");
        assert_eq!(init.phase(), Phase::Closed, "session left open");
    }

    #[test]
    fn replay_destroys_the_session() {
        let (mut init, mut resp) = open_pair();
        let ct = init.seal(b"once").unwrap();
        resp.receive_data(&ct).unwrap();
        assert!(resp.receive_data(&ct).is_err(), "replay accepted");
        assert_eq!(resp.phase(), Phase::Closed, "session survived a replay");
    }

    #[test]
    fn tampered_confirm_is_rejected() {
        let (mut init, mut resp) = shaken();
        let mut c1 = init.emit_initiator_confirm(&CNF_INIT, GRANT_JWT).unwrap();
        let last = c1.len() - 1;
        c1[last] ^= 0x01;
        assert!(
            resp.receive_initiator_confirm(&c1).is_err(),
            "tampered confirm accepted"
        );
        assert_eq!(resp.phase(), Phase::Closed, "session survived tampering");
    }

    #[test]
    fn wrong_binding_key_is_rejected() {
        let (mut init, mut resp) = shaken();
        let c1 = init.emit_initiator_confirm(&CNF_INIT, GRANT_JWT).unwrap();
        let parsed = resp.receive_initiator_confirm(&c1).unwrap();
        assert!(
            resp.accept_initiator_confirm(&[0x09; 32], &parsed).is_err(),
            "wrong key accepted"
        );
        assert_eq!(
            resp.phase(),
            Phase::Closed,
            "session survived a bad binding"
        );
    }

    #[test]
    fn a_frame_for_another_channel_is_rejected() {
        let framed = frame(KIND_DATA, "exec-1", b"hi").unwrap();
        assert_eq!(
            parse_frame(&framed, "exec-1").unwrap(),
            (KIND_DATA, b"hi".as_slice()),
            "own channel"
        );
        assert!(
            parse_frame(&framed, "other").is_err(),
            "foreign channel accepted"
        );
    }

    #[test]
    fn binding_signatures_separate_roles_and_transcripts() {
        let hash = [0xAB; 64];
        let ed = [0x55; 32];
        let public = cnf_public(&ed);
        let signature = bind_sign(&ed, ROLE_INITIATOR, &hash);
        assert!(
            bind_verify(&public, ROLE_INITIATOR, &hash, &signature),
            "own binding"
        );
        assert!(
            !bind_verify(&public, ROLE_RESPONDER, &hash, &signature),
            "role swap"
        );
        assert!(
            !bind_verify(&public, ROLE_INITIATOR, &[0; 64], &signature),
            "other transcript"
        );
    }

    #[test]
    fn relay_vectors_reproduce_byte_exact() {
        let path = std::env::var("NOISE_FIXTURE_PATH").map_or_else(
            |_unset| {
                std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/noise-xx.json")
            },
            std::path::PathBuf::from,
        );
        let fixture: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let field = |name: &str| fixture[name].as_str().unwrap().to_owned();
        assert_eq!(fixture["protocol"], NOISE_PROTOCOL, "protocol");
        let ctx = ChannelCtx {
            account: fixture["ctx"]["account"].as_str().unwrap().into(),
            machine: fixture["ctx"]["machine"].as_str().unwrap().into(),
            channel: fixture["ctx"]["channel"].as_str().unwrap().into(),
        };
        assert_eq!(
            hex::encode(prologue_bytes(&ctx)),
            field("prologue_hex"),
            "prologue"
        );
        let init_pub = noise_public(INIT_STATIC);
        let resp_pub = noise_public(RESP_STATIC);
        assert_eq!(
            hex::encode(init_pub),
            field("init_static_pub"),
            "initiator static"
        );
        assert_eq!(
            hex::encode(resp_pub),
            field("resp_static_pub"),
            "responder static"
        );

        let mut init = NoiseChannel::build(
            true,
            &INIT_STATIC,
            Some(resp_pub),
            ctx.clone(),
            Some(&[0x33; 32]),
        )
        .unwrap();
        let mut resp = NoiseChannel::build(
            false,
            &RESP_STATIC,
            Some(init_pub),
            ctx.clone(),
            Some(&[0x44; 32]),
        )
        .unwrap();
        let mut m1 = Vec::new();
        init.next_handshake_message(&mut m1).unwrap();
        assert_eq!(hex::encode(&m1), field("m1"), "m1");
        resp.receive_handshake_message(&m1).unwrap();
        let mut m2 = Vec::new();
        resp.next_handshake_message(&mut m2).unwrap();
        assert_eq!(hex::encode(&m2), field("m2"), "m2");
        init.receive_handshake_message(&m2).unwrap();
        let mut m3 = Vec::new();
        init.next_handshake_message(&mut m3).unwrap();
        assert_eq!(hex::encode(&m3), field("m3"), "m3");
        resp.receive_handshake_message(&m3).unwrap();

        let grant_jwt = field("grant_jwt");
        let c1 = init
            .emit_initiator_confirm(&[0x55; 32], grant_jwt.as_bytes())
            .unwrap();
        assert_eq!(
            hex::encode(&c1),
            field("confirm_init_ct"),
            "initiator confirm"
        );
        let parsed = resp.receive_initiator_confirm(&c1).unwrap();
        let issuer: [u8; 32] = {
            use base64::Engine as _;
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(fixture["jwks"]["keys"][0]["x"].as_str().unwrap())
                .unwrap()
                .try_into()
                .unwrap()
        };
        let params = crate::grant::VerifyParams {
            expected_iss: "https://core.test/ast".into(),
            expected_aud: crate::grant::AUD_RELAY.into(),
            max_ttl_s: crate::GRANT_MAX_TTL_S,
            expected_sub: ctx.account.clone(),
            expected_mid: ctx.machine,
            now_sec: 2_000_000_000,
        };
        let claims = crate::grant::parse_and_verify_jwt(
            std::str::from_utf8(&parsed.jwt).unwrap(),
            &issuer,
            &params,
        )
        .unwrap();
        let cnf = crate::grant::cnf_public_key(&claims).unwrap();
        assert_eq!(hex::encode(cnf), field("init_cnf_pub"), "grant cnf");
        resp.accept_initiator_confirm(&cnf, &parsed).unwrap();

        let c2 = resp.emit_responder_confirm(&[0x66; 32]).unwrap();
        assert_eq!(
            hex::encode(&c2),
            field("confirm_resp_ct"),
            "responder confirm"
        );
        let forge_enrolled = cnf_public(&[0x66; 32]);
        assert_eq!(
            hex::encode(forge_enrolled),
            field("resp_cnf_pub"),
            "enrolled key"
        );
        init.receive_responder_confirm(&c2, &forge_enrolled)
            .unwrap();

        let a = field("plaintext_a");
        let ct_init = init.seal(a.as_bytes()).unwrap();
        assert_eq!(
            hex::encode(&ct_init),
            field("data_init_ct"),
            "initiator data"
        );
        assert_eq!(
            resp.receive_data(&ct_init).unwrap(),
            a.as_bytes(),
            "initiator data opens"
        );
        let b = field("plaintext_b");
        let ct_resp = resp.seal(b.as_bytes()).unwrap();
        assert_eq!(
            hex::encode(&ct_resp),
            field("data_resp_ct"),
            "responder data"
        );
        assert_eq!(
            init.receive_data(&ct_resp).unwrap(),
            b.as_bytes(),
            "responder data opens"
        );
    }
}
