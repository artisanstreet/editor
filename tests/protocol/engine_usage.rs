//! Owned-protocol coverage for the provider-account usage packet.
//!
//! Query and snapshot arms round-trip field-for-field through the owned
//! codec; malformed, oversized, non-finite, and unknown-discriminant frames
//! are rejected with typed errors. No engine is launched and no provider is
//! contacted.

use artisan_domain::{
    ENGINE_USAGE_ENGINES_MAX, ENGINE_USAGE_WINDOWS_MAX_PER_ENGINE, EngineReadiness,
    EngineReadinessVerdict, EngineUsageAuth, EngineUsageAuthentication, EngineUsageReport,
    EngineUsageSnapshot, EngineUsageWindow, EngineUsageWindowKind, Query, QuotaSurface,
    ReadAccountUsage, RequestId, UnixMillis,
};
use artisan_protocol::artisan_capnp::{envelope, response};
use artisan_protocol::{
    ClientRequest, FrameId, ProtocolDecodeError, ResponsePayload, ServerResponse, WireEnvelope,
    WireEnvelopeBody, decode_envelope, encode_envelope,
};
use capnp::message::{Builder, HeapAllocator};
use capnp::serialize;

fn request_id(value: &str) -> RequestId {
    RequestId::parse(value).expect("request id is valid")
}

fn query_envelope(frame_id: &str, query: Query) -> WireEnvelope {
    WireEnvelope {
        protocol_version: artisan_protocol::ProtocolVersion::V1,
        frame_id: FrameId::parse(frame_id).expect("frame id is valid"),
        sent_at: UnixMillis::from_millis(42),
        body: WireEnvelopeBody::Request(ClientRequest::Query(query)),
    }
}

fn response_envelope(frame_id: &str, request: &str, payload: ResponsePayload) -> WireEnvelope {
    WireEnvelope {
        protocol_version: artisan_protocol::ProtocolVersion::V1,
        frame_id: FrameId::parse(frame_id).expect("frame id is valid"),
        sent_at: UnixMillis::from_millis(42),
        body: WireEnvelopeBody::Response(ServerResponse {
            request_id: request_id(request),
            payload,
        }),
    }
}

fn auth(state: EngineUsageAuthentication) -> EngineUsageAuth {
    EngineUsageAuth::new(state, None).expect("auth is valid")
}

fn window(
    id: &str,
    kind: EngineUsageWindowKind,
    percent: f64,
    resets_at: Option<&str>,
    minutes: Option<u32>,
) -> EngineUsageWindow {
    EngineUsageWindow::new(
        id.to_owned(),
        kind,
        None,
        percent,
        resets_at.map(str::to_owned),
        minutes,
    )
    .expect("fixture window is valid")
}

fn snapshot_fixture() -> EngineUsageSnapshot {
    let codex = EngineUsageReport::new(
        Some("owner@example.test".to_owned()),
        auth(EngineUsageAuthentication::Authenticated),
        "Codex".to_owned(),
        "codex".to_owned(),
        None,
        Some(QuotaSurface::Supported),
        vec![
            window(
                "five_hour",
                EngineUsageWindowKind::Session,
                42.5,
                Some("2026-09-09T12:00:00Z"),
                Some(300),
            ),
            EngineUsageWindow::new(
                "seven_day:fable".to_owned(),
                EngineUsageWindowKind::Weekly,
                Some("Fable".to_owned()),
                63.0,
                None,
                Some(10_080),
            )
            .expect("labeled window is valid"),
        ],
    )
    .expect("codex report is valid");
    let grok = EngineUsageReport::new(
        None,
        EngineUsageAuth::new(
            EngineUsageAuthentication::Unknown,
            Some("Grok Build exposes no account-usage surface.".to_owned()),
        )
        .expect("auth is valid"),
        "Grok Build".to_owned(),
        "grok".to_owned(),
        Some("Grok Build exposes no account-usage surface.".to_owned()),
        Some(QuotaSurface::Unsupported),
        Vec::new(),
    )
    .expect("unsupported report is valid");
    EngineUsageSnapshot::new(vec![codex, grok], "2026-09-09T12:00:00Z".to_owned())
        .expect("snapshot is valid")
}

#[test]
fn account_usage_query_arms_round_trip() {
    for (frame, query) in [
        (
            "usage-query-all",
            ReadAccountUsage::new(None, false).expect("query is valid"),
        ),
        (
            "usage-query-one",
            ReadAccountUsage::new(Some("codex".to_owned()), true).expect("query is valid"),
        ),
    ] {
        let frame = query_envelope(frame, Query::ReadAccountUsage(query.clone()));
        let decoded = decode_envelope(&encode_envelope(&frame).expect("query encodes"))
            .expect("query decodes");
        assert!(decoded == frame);
        match decoded.body {
            WireEnvelopeBody::Request(ClientRequest::Query(Query::ReadAccountUsage(decoded))) => {
                assert_eq!(decoded, query);
            }
            _ => panic!("expected readAccountUsage body"),
        }
    }
}

#[test]
fn account_usage_snapshot_round_trips_field_for_field() {
    let snapshot = snapshot_fixture();
    let frame = response_envelope(
        "server-usage-frame",
        "usage-request",
        ResponsePayload::AccountUsage(snapshot.clone()),
    );
    let decoded = decode_envelope(&encode_envelope(&frame).expect("snapshot encodes"))
        .expect("snapshot decodes");
    assert!(decoded == frame);
    match decoded.body {
        WireEnvelopeBody::Response(response) => {
            assert_eq!(response.request_id, request_id("usage-request"));
            match response.payload {
                ResponsePayload::AccountUsage(decoded) => {
                    assert_eq!(decoded, snapshot);
                    assert_eq!(decoded.engines().len(), 2);
                    assert_eq!(decoded.engines()[0].windows().len(), 2);
                    assert_eq!(decoded.engines()[0].windows()[1].label(), Some("Fable"));
                    assert_eq!(
                        decoded.engines()[1].quota_surface(),
                        Some(QuotaSurface::Unsupported)
                    );
                    assert_eq!(decoded.fetched_at(), "2026-09-09T12:00:00Z");
                }
                _ => panic!("expected accountUsage payload"),
            }
        }
        _ => panic!("expected response body"),
    }
}

#[test]
fn forge_readiness_verdicts_round_trip_with_their_reasons() {
    let base = snapshot_fixture();
    let verdicts = [
        EngineReadiness::ready(),
        EngineReadiness::new(
            EngineReadinessVerdict::NeedsSignIn,
            Some("Codex account sign-in is required.".to_owned()),
        )
        .expect("verdict"),
        EngineReadiness::new(
            EngineReadinessVerdict::Checking,
            Some("Checking the Codex account status.".to_owned()),
        )
        .expect("verdict"),
        EngineReadiness::new(EngineReadinessVerdict::NotReady, None).expect("verdict"),
    ];
    for readiness in verdicts {
        let engines = base
            .engines()
            .iter()
            .cloned()
            .map(|report| report.with_readiness(readiness.clone()))
            .collect();
        let snapshot = EngineUsageSnapshot::new(engines, base.fetched_at()).expect("snapshot");
        let frame = response_envelope(
            "server-usage-readiness",
            "usage-readiness",
            ResponsePayload::AccountUsage(snapshot.clone()),
        );
        let decoded = decode_envelope(&encode_envelope(&frame).expect("snapshot encodes"))
            .expect("snapshot decodes");
        assert!(decoded == frame);
        let WireEnvelopeBody::Response(ServerResponse {
            payload: ResponsePayload::AccountUsage(decoded),
            ..
        }) = decoded.body
        else {
            panic!("expected accountUsage payload");
        };
        assert_eq!(decoded.engines()[0].readiness(), &readiness);
    }
}

#[test]
fn a_report_without_a_verdict_decodes_as_not_ready() {
    assert_eq!(
        snapshot_fixture().engines()[0].readiness().verdict(),
        EngineReadinessVerdict::NotReady
    );
    let bytes = raw_account_usage_frame("usage-no-verdict", |response| {
        let mut snapshot = response.init_account_usage();
        snapshot.set_fetched_at("2026-09-09T12:00:00Z");
        let mut report = snapshot.init_engines(1).get(0);
        report.set_engine_id("codex");
        report.set_display_name("Codex");
        report.reborrow().init_quota_surface().set_absent(());
    });
    let decoded = decode_envelope(&bytes).expect("a legacy report decodes");
    let WireEnvelopeBody::Response(ServerResponse {
        payload: ResponsePayload::AccountUsage(snapshot),
        ..
    }) = decoded.body
    else {
        panic!("expected accountUsage payload");
    };
    assert!(!snapshot.engines()[0].readiness().is_ready());
}

#[test]
fn absent_optionals_survive_as_empty_wire_values() {
    let report = EngineUsageReport::new(
        None,
        auth(EngineUsageAuthentication::Unknown),
        "Cursor".to_owned(),
        "cursor".to_owned(),
        None,
        None,
        vec![window(
            "cursor:included-usage",
            EngineUsageWindowKind::Monthly,
            0.0,
            None,
            None,
        )],
    )
    .expect("minimal report is valid");
    let snapshot = EngineUsageSnapshot::new(vec![report], "2026-09-09T12:00:00Z".to_owned())
        .expect("snapshot is valid");
    let frame = response_envelope(
        "server-usage-absent",
        "usage-absent",
        ResponsePayload::AccountUsage(snapshot.clone()),
    );
    let decoded = decode_envelope(&encode_envelope(&frame).expect("snapshot encodes"))
        .expect("snapshot decodes");
    assert!(decoded == frame);
}

fn raw_account_usage_frame(frame_id: &str, fill: impl FnOnce(response::Builder<'_>)) -> Vec<u8> {
    let mut message = Builder::new(HeapAllocator::new());
    let mut root = message.init_root::<envelope::Builder>();
    root.set_protocol_version(1);
    root.set_message_id(frame_id);
    root.set_sent_at_millis(42);
    let mut response = root.init_body().init_response();
    response.set_request_id("usage-raw-request");
    fill(response);
    serialize::write_message_to_words(&message)
}

fn raw_snapshot_with_window(
    frame_id: &str,
    window_id: &str,
    percent: f64,
    resets_at: &str,
    minutes: u32,
    fetched_at: &str,
) -> Vec<u8> {
    raw_account_usage_frame(frame_id, |response| {
        let mut snapshot = response.init_account_usage();
        snapshot.set_fetched_at(fetched_at);
        let mut engines = snapshot.init_engines(1);
        let mut engine = engines.reborrow().get(0);
        engine.set_engine_id("codex");
        engine.set_display_name("Codex");
        engine.set_authentication(
            artisan_protocol::artisan_capnp::EngineUsageAuthentication::Authenticated,
        );
        engine
            .reborrow()
            .init_quota_surface()
            .set_present(artisan_protocol::artisan_capnp::QuotaSurface::Supported);
        let mut windows = engine.reborrow().init_windows(1);
        let mut window = windows.reborrow().get(0);
        window.set_id(window_id);
        window.set_kind(artisan_protocol::artisan_capnp::EngineUsageWindowKind::Session);
        window.set_percent_used(percent);
        window.set_resets_at(resets_at);
        window.set_window_minutes(minutes);
    })
}

#[test]
fn malformed_windows_are_rejected_without_guessing() {
    let empty_id = raw_snapshot_with_window(
        "usage-empty-id",
        "",
        10.0,
        "2026-09-09T12:00:00Z",
        300,
        "2026-09-09T12:00:00Z",
    );
    assert!(matches!(
        decode_envelope(&empty_id),
        Err(ProtocolDecodeError::EngineUsage { .. })
    ));

    let bad_resets = raw_snapshot_with_window(
        "usage-bad-resets",
        "five_hour",
        10.0,
        "soon",
        300,
        "2026-09-09T12:00:00Z",
    );
    assert!(matches!(
        decode_envelope(&bad_resets),
        Err(ProtocolDecodeError::EngineUsage { .. })
    ));

    let non_finite = raw_snapshot_with_window(
        "usage-nan",
        "five_hour",
        f64::NAN,
        "2026-09-09T12:00:00Z",
        300,
        "2026-09-09T12:00:00Z",
    );
    assert!(matches!(
        decode_envelope(&non_finite),
        Err(ProtocolDecodeError::EngineUsage { .. })
    ));

    let bad_fetch = raw_snapshot_with_window(
        "usage-bad-fetch",
        "five_hour",
        10.0,
        "2026-09-09T12:00:00Z",
        300,
        "yesterday",
    );
    assert!(matches!(
        decode_envelope(&bad_fetch),
        Err(ProtocolDecodeError::EngineUsage { .. })
    ));
}

#[test]
fn oversized_collections_are_rejected_before_allocation() {
    let too_many_windows = raw_account_usage_frame("usage-many-windows", |response| {
        let mut snapshot = response.init_account_usage();
        snapshot.set_fetched_at("2026-09-09T12:00:00Z");
        let mut engines = snapshot.init_engines(1);
        let mut engine = engines.reborrow().get(0);
        engine.set_engine_id("codex");
        engine.set_display_name("Codex");
        engine.set_authentication(
            artisan_protocol::artisan_capnp::EngineUsageAuthentication::Authenticated,
        );
        let count =
            u32::try_from(ENGINE_USAGE_WINDOWS_MAX_PER_ENGINE + 1).expect("fixture count fits");
        let mut windows = engine.reborrow().init_windows(count);
        for index in 0..=ENGINE_USAGE_WINDOWS_MAX_PER_ENGINE {
            let mut window = windows
                .reborrow()
                .get(u32::try_from(index).expect("fixture index fits"));
            window.set_id(format!("window-{index}").as_str());
            window.set_percent_used(1.0);
        }
    });
    assert!(matches!(
        decode_envelope(&too_many_windows),
        Err(ProtocolDecodeError::EngineUsage { .. })
    ));

    let too_many_engines = raw_account_usage_frame("usage-many-engines", |response| {
        let mut snapshot = response.init_account_usage();
        snapshot.set_fetched_at("2026-09-09T12:00:00Z");
        let count = u32::try_from(ENGINE_USAGE_ENGINES_MAX + 1).expect("fixture count fits");
        let mut engines = snapshot.init_engines(count);
        for index in 0..=ENGINE_USAGE_ENGINES_MAX {
            let mut engine = engines
                .reborrow()
                .get(u32::try_from(index).expect("fixture index fits"));
            engine.set_engine_id(format!("engine-{index}").as_str());
            engine.set_display_name("Engine");
            engine.set_authentication(
                artisan_protocol::artisan_capnp::EngineUsageAuthentication::Unknown,
            );
        }
    });
    assert!(matches!(
        decode_envelope(&too_many_engines),
        Err(ProtocolDecodeError::EngineUsage { .. })
    ));
}

fn raw_snapshot_with_kind(frame_id: &str, session: bool) -> Vec<u8> {
    raw_account_usage_frame(frame_id, |response| {
        let mut snapshot = response.init_account_usage();
        snapshot.set_fetched_at("2026-09-09T12:00:00Z");
        let mut engines = snapshot.init_engines(1);
        let mut engine = engines.reborrow().get(0);
        engine.set_engine_id("codex");
        engine.set_display_name("Codex");
        engine.set_authentication(
            artisan_protocol::artisan_capnp::EngineUsageAuthentication::Authenticated,
        );
        let mut windows = engine.reborrow().init_windows(1);
        let mut window = windows.reborrow().get(0);
        window.set_id("five_hour");
        window.set_kind(if session {
            artisan_protocol::artisan_capnp::EngineUsageWindowKind::Session
        } else {
            artisan_protocol::artisan_capnp::EngineUsageWindowKind::Weekly
        });
        window.set_percent_used(10.0);
    })
}

#[test]
fn unknown_window_kind_discriminant_is_typed() {
    let mut malformed = raw_snapshot_with_kind("usage-kind-flip", true);
    let comparison = raw_snapshot_with_kind("usage-kind-flip", false);
    let differing: Vec<usize> = malformed
        .iter()
        .zip(comparison)
        .enumerate()
        .filter_map(|(index, (left, right))| (left != &right).then_some(index))
        .collect();
    assert_eq!(differing.len(), 1, "only the kind ordinal should differ");
    malformed[differing[0]] = u8::MAX;
    assert!(matches!(
        decode_envelope(&malformed),
        Err(ProtocolDecodeError::UnknownDiscriminant { value: 255 })
    ));
}

#[test]
fn trailing_bytes_after_a_usage_frame_are_rejected() {
    let snapshot = snapshot_fixture();
    let mut bytes = encode_envelope(&response_envelope(
        "server-usage-trailing",
        "usage-trailing",
        ResponsePayload::AccountUsage(snapshot),
    ))
    .expect("snapshot encodes");
    bytes.push(0);
    assert!(matches!(
        decode_envelope(&bytes),
        Err(ProtocolDecodeError::TrailingBytes { length: 1 })
    ));
}
