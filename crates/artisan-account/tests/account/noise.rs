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

fn shake(init: &mut NoiseChannel, resp: &mut NoiseChannel) -> Result<(), NoiseError> {
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
