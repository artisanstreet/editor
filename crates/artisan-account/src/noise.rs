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

#[derive(Debug, thiserror::Error)]
#[error("Noise channel {channel}")]
pub struct NoiseError {
    channel: String,
    #[source]
    failure: NoiseFailure,
}

#[derive(Debug, thiserror::Error)]
pub enum NoiseFailure {
    #[error("the channel name is not 1 to 64 bytes")]
    ChannelName,
    #[error("set up the Noise handshake")]
    Setup(#[source] snow::Error),
    #[error("the frame header is not AR1 version 1")]
    FrameHeader,
    #[error("the frame is cut short")]
    ShortFrame,
    #[error("the frame is for another channel")]
    OtherChannel,
    #[error("the frame kind {kind} is unknown")]
    UnknownKind { kind: u8 },
    #[error("encrypt a Noise message")]
    Encrypt(#[source] snow::Error),
    #[error("decrypt a Noise message")]
    Decrypt(#[source] snow::Error),
    #[error("the Noise handshake is over")]
    HandshakeOver,
    #[error("write a Noise handshake message")]
    WriteHandshake(#[source] snow::Error),
    #[error("read a Noise handshake message")]
    ReadHandshake(#[source] snow::Error),
    #[error("the peer's Noise key is not the expected one")]
    PeerKeyMismatch,
    #[error("finish the Noise handshake")]
    FinishHandshake(#[source] snow::Error),
    #[error("the Noise session is not open")]
    NotOpen,
    #[error("the Noise session is not confirmed")]
    NotConfirmed,
    #[error("the grant of {bytes} bytes is empty or too large for the confirm")]
    GrantSize { bytes: usize },
    #[error("this side's confirm was already sent")]
    ConfirmAlreadySent,
    #[error("the peer already confirmed")]
    ConfirmAlreadyReceived,
    #[error("expected the peer's confirm, got data")]
    ExpectedConfirm,
    #[error("the confirm is cut short")]
    ShortConfirm,
    #[error("the confirm is for another handshake")]
    ConfirmForAnotherHandshake,
    #[error("the confirm names a grant of {bytes} bytes")]
    ConfirmGrantSize { bytes: usize },
    #[error("the initiator confirm is not signed by the grant key")]
    InitiatorSignature,
    #[error("the Forge confirm is not signed by its enrolled key")]
    ForgeSignature,
    #[error("a confirm arrived on an open session")]
    UnexpectedConfirm,
    #[error("a message of {bytes} bytes is too large for one Noise frame")]
    TooLarge { bytes: usize },
}

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

fn frame(kind: u8, channel: &str, body: &[u8]) -> Result<Vec<u8>, NoiseFailure> {
    let channel_len = u8::try_from(channel.len())
        .ok()
        .filter(|len| (1..=64).contains(len))
        .ok_or(NoiseFailure::ChannelName)?;
    Ok([
        APP_MAGIC,
        &[APP_VERSION, channel_len],
        channel.as_bytes(),
        &[kind],
        body,
    ]
    .concat())
}

fn parse_frame<'a>(plaintext: &'a [u8], channel: &str) -> Result<(u8, &'a [u8]), NoiseFailure> {
    let rest = plaintext
        .strip_prefix(APP_MAGIC)
        .and_then(|rest| rest.strip_prefix(&[APP_VERSION]))
        .ok_or(NoiseFailure::FrameHeader)?;
    let (&channel_len, rest) = rest.split_first().ok_or(NoiseFailure::ShortFrame)?;
    let (name, rest) = rest
        .split_at_checked(usize::from(channel_len))
        .ok_or(NoiseFailure::ShortFrame)?;
    if name != channel.as_bytes() {
        return Err(NoiseFailure::OtherChannel);
    }
    let (&kind, body) = rest.split_first().ok_or(NoiseFailure::ShortFrame)?;
    if kind != KIND_CONFIRM && kind != KIND_DATA {
        return Err(NoiseFailure::UnknownKind { kind });
    }
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
    jwt: Vec<u8>,
    signature: [u8; 64],
}

impl InitiatorConfirm {
    #[must_use]
    pub fn jwt(&self) -> &[u8] {
        &self.jwt
    }
}

impl std::fmt::Debug for InitiatorConfirm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InitiatorConfirm")
            .field("jwt_bytes", &self.jwt.len())
            .finish_non_exhaustive()
    }
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

    fn seal(&mut self, channel: &str, kind: u8, body: &[u8]) -> Result<Vec<u8>, NoiseFailure> {
        let framed = frame(kind, channel, body)?;
        let mut buf = vec![0_u8; framed.len() + 16];
        let len = self
            .transport
            .write_message(&framed, &mut buf)
            .map_err(NoiseFailure::Encrypt)?;
        buf.truncate(len);
        Ok(buf)
    }

    fn open(&mut self, channel: &str, ciphertext: &[u8]) -> Result<(u8, Vec<u8>), NoiseFailure> {
        let mut buf = vec![0_u8; ciphertext.len()];
        let len = self
            .transport
            .read_message(ciphertext, &mut buf)
            .map_err(NoiseFailure::Decrypt)?;
        let (kind, body) = parse_frame(&buf[..len], channel)?;
        Ok((kind, body.to_vec()))
    }
}

enum State {
    Handshake(Box<snow::HandshakeState>),
    Open(Box<Open>),
    Closed,
}

pub struct NoiseChannel {
    state: State,
    channel: String,
    expected_peer_static: Option<[u8; 32]>,
}

impl std::fmt::Debug for NoiseChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NoiseChannel")
            .field("channel", &self.channel)
            .field("phase", &self.phase())
            .finish_non_exhaustive()
    }
}

impl NoiseChannel {
    pub fn initiator(
        static_secret: &[u8; 32],
        peer_static: Option<[u8; 32]>,
        ctx: ChannelCtx,
    ) -> Result<Self, NoiseError> {
        Self::build(true, static_secret, peer_static, ctx, None)
    }

    pub fn responder(
        static_secret: &[u8; 32],
        peer_static: Option<[u8; 32]>,
        ctx: ChannelCtx,
    ) -> Result<Self, NoiseError> {
        Self::build(false, static_secret, peer_static, ctx, None)
    }

    fn build(
        initiator: bool,
        static_secret: &[u8; 32],
        expected_peer_static: Option<[u8; 32]>,
        ctx: ChannelCtx,
        fixed_ephemeral: Option<&[u8; 32]>,
    ) -> Result<Self, NoiseError> {
        let setup = |failure| NoiseError {
            channel: ctx.channel.clone(),
            failure,
        };
        let prologue = prologue_bytes(&ctx);
        let protocol = NOISE_PROTOCOL
            .parse()
            .map_err(|source| setup(NoiseFailure::Setup(source)))?;
        let mut builder = snow::Builder::new(protocol)
            .prologue(&prologue)
            .and_then(|builder| builder.local_private_key(static_secret))
            .map_err(|source| setup(NoiseFailure::Setup(source)))?;
        if let Some(ephemeral) = fixed_ephemeral {
            builder = builder.fixed_ephemeral_key_for_testing_only(ephemeral);
        }
        let state = if initiator {
            builder.build_initiator()
        } else {
            builder.build_responder()
        }
        .map_err(|source| setup(NoiseFailure::Setup(source)))?;
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

    fn failed(&self, failure: NoiseFailure) -> NoiseError {
        NoiseError {
            channel: self.channel.clone(),
            failure,
        }
    }

    pub fn next_handshake_message(&mut self, out: &mut Vec<u8>) -> Result<(), NoiseError> {
        let written = match &mut self.state {
            State::Handshake(hand) => {
                let mut buf = vec![0_u8; MAX_MESSAGE];
                hand.write_message(&[], &mut buf)
                    .map(|len| out.extend_from_slice(&buf[..len]))
                    .map_err(NoiseFailure::WriteHandshake)
            }
            State::Open(_) | State::Closed => Err(NoiseFailure::HandshakeOver),
        };
        written
            .and_then(|()| self.finish_handshake())
            .map_err(|failure| self.failed(failure))
    }

    pub fn receive_handshake_message(&mut self, message: &[u8]) -> Result<(), NoiseError> {
        let read = match &mut self.state {
            State::Handshake(hand) => {
                let mut buf = vec![0_u8; message.len()];
                match hand.read_message(message, &mut buf) {
                    Err(source) => Err(NoiseFailure::ReadHandshake(source)),
                    Ok(_) => match self.expected_peer_static {
                        Some(expected)
                            if hand
                                .get_remote_static()
                                .is_some_and(|remote| remote != expected.as_slice()) =>
                        {
                            Err(NoiseFailure::PeerKeyMismatch)
                        }
                        Some(_) | None => Ok(()),
                    },
                }
            }
            State::Open(_) | State::Closed => Err(NoiseFailure::HandshakeOver),
        };
        if matches!(read, Err(NoiseFailure::PeerKeyMismatch)) {
            self.state = State::Closed;
        }
        read.and_then(|()| self.finish_handshake())
            .map_err(|failure| self.failed(failure))
    }

    fn finish_handshake(&mut self) -> Result<(), NoiseFailure> {
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
            .map_err(NoiseFailure::FinishHandshake)?;
        self.state = State::Open(Box::new(Open {
            transport,
            handshake_hash,
            confirm_sent: false,
            confirm_received: false,
        }));
        Ok(())
    }

    fn with_open<T>(
        &mut self,
        step: impl FnOnce(&mut Open, &str) -> Result<T, NoiseFailure>,
    ) -> Result<T, NoiseError> {
        let outcome = match &mut self.state {
            State::Open(open) => step(open, &self.channel),
            State::Handshake(_) | State::Closed => Err(NoiseFailure::NotOpen),
        };
        outcome.map_err(|failure| self.failed(failure))
    }

    fn closing_on_error<T>(
        &mut self,
        step: impl FnOnce(&mut Open, &str) -> Result<T, NoiseFailure>,
    ) -> Result<T, NoiseError> {
        let opened = matches!(self.state, State::Open(_));
        let outcome = self.with_open(step);
        if opened && outcome.is_err() {
            self.state = State::Closed;
        }
        outcome
    }

    pub fn emit_initiator_confirm(
        &mut self,
        own_cnf_secret: &[u8; 32],
        grant_jwt: &[u8],
    ) -> Result<Vec<u8>, NoiseError> {
        self.with_open(|open, channel| {
            let jwt_len = u16::try_from(grant_jwt.len())
                .ok()
                .filter(|len| (1..=MAX_GRANT_JWT).contains(&usize::from(*len)))
                .ok_or(NoiseFailure::GrantSize {
                    bytes: grant_jwt.len(),
                })?;
            if open.confirm_sent {
                return Err(NoiseFailure::ConfirmAlreadySent);
            }
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
        })
    }

    pub fn receive_initiator_confirm(
        &mut self,
        ciphertext: &[u8],
    ) -> Result<InitiatorConfirm, NoiseError> {
        self.closing_on_error(|open, channel| {
            let (kind, body) = open.open(channel, ciphertext)?;
            if kind != KIND_CONFIRM {
                return Err(NoiseFailure::ExpectedConfirm);
            }
            let (hash, rest) = body
                .split_at_checked(HASH_LEN)
                .ok_or(NoiseFailure::ShortConfirm)?;
            if hash != open.handshake_hash.as_slice() {
                return Err(NoiseFailure::ConfirmForAnotherHandshake);
            }
            let (len, rest) = rest
                .split_first_chunk::<2>()
                .ok_or(NoiseFailure::ShortConfirm)?;
            let jwt_len = usize::from(u16::from_be_bytes(*len));
            if !(1..=MAX_GRANT_JWT).contains(&jwt_len) {
                return Err(NoiseFailure::ConfirmGrantSize { bytes: jwt_len });
            }
            let (jwt, signature) = rest
                .split_at_checked(jwt_len)
                .ok_or(NoiseFailure::ShortConfirm)?;
            Ok(InitiatorConfirm {
                jwt: jwt.to_vec(),
                signature: signature
                    .try_into()
                    .map_err(|_short: std::array::TryFromSliceError| NoiseFailure::ShortConfirm)?,
            })
        })
    }

    pub fn accept_initiator_confirm(
        &mut self,
        peer_cnf_public: &[u8; 32],
        confirm: &InitiatorConfirm,
    ) -> Result<(), NoiseError> {
        self.closing_on_error(|open, _| {
            if open.confirm_received {
                return Err(NoiseFailure::ConfirmAlreadyReceived);
            }
            if !bind_verify(
                peer_cnf_public,
                ROLE_INITIATOR,
                &open.handshake_hash,
                &confirm.signature,
            ) {
                return Err(NoiseFailure::InitiatorSignature);
            }
            open.confirm_received = true;
            Ok(())
        })
    }

    pub fn emit_responder_confirm(
        &mut self,
        own_cnf_secret: &[u8; 32],
    ) -> Result<Vec<u8>, NoiseError> {
        self.with_open(|open, channel| {
            if open.confirm_sent {
                return Err(NoiseFailure::ConfirmAlreadySent);
            }
            let signature = bind_sign(own_cnf_secret, ROLE_RESPONDER, &open.handshake_hash);
            let body = [open.handshake_hash.as_slice(), &signature].concat();
            let sealed = open.seal(channel, KIND_CONFIRM, &body)?;
            open.confirm_sent = true;
            Ok(sealed)
        })
    }

    pub fn receive_responder_confirm(
        &mut self,
        ciphertext: &[u8],
        forge_enrolled_public: &[u8; 32],
    ) -> Result<(), NoiseError> {
        self.closing_on_error(|open, channel| {
            if open.confirm_received {
                return Err(NoiseFailure::ConfirmAlreadyReceived);
            }
            let (kind, body) = open.open(channel, ciphertext)?;
            if kind != KIND_CONFIRM {
                return Err(NoiseFailure::ExpectedConfirm);
            }
            let (hash, signature) = body
                .split_at_checked(HASH_LEN)
                .ok_or(NoiseFailure::ShortConfirm)?;
            if hash != open.handshake_hash.as_slice() {
                return Err(NoiseFailure::ConfirmForAnotherHandshake);
            }
            if !bind_verify(
                forge_enrolled_public,
                ROLE_RESPONDER,
                &open.handshake_hash,
                signature,
            ) {
                return Err(NoiseFailure::ForgeSignature);
            }
            open.confirm_received = true;
            Ok(())
        })
    }

    pub fn seal(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, NoiseError> {
        self.with_open(|open, channel| {
            if plaintext.len() > MAX_APP_PLAINTEXT {
                return Err(NoiseFailure::TooLarge {
                    bytes: plaintext.len(),
                });
            }
            if !open.confirmed() {
                return Err(NoiseFailure::NotConfirmed);
            }
            open.seal(channel, KIND_DATA, plaintext)
        })
    }

    pub fn receive_data(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, NoiseError> {
        self.closing_on_error(|open, channel| {
            if !open.confirmed() {
                return Err(NoiseFailure::NotConfirmed);
            }
            let (kind, body) = open.open(channel, ciphertext)?;
            if kind != KIND_DATA {
                return Err(NoiseFailure::UnexpectedConfirm);
            }
            Ok(body)
        })
    }
}

#[cfg(test)]
#[path = "../tests/account/noise.rs"]
mod tests;
