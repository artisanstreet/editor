//! Owned protocol coverage for Forge-managed engine installs: the status read
//! and push, the version listing, and version changes.

use std::error::Error;

use artisan_domain::{
    ChangeEngineVersion, EngineInstallPhase, EngineInstallSnapshot, EngineInstallStatus,
    EngineIntegrity, EngineVersionChange, EngineVersionEntry, EngineVersionList,
    EngineVersionSelection, Event, ListEngineVersions, Query, ReadEngineInstalls, RequestId,
    UnixMillis,
};
use artisan_protocol::{
    ClientRequest, EventCursor, FrameId, ProtocolVersion, ResponsePayload, ServerEvent,
    ServerResponse, WireEnvelope, WireEnvelopeBody, decode_envelope, encode_envelope,
};

fn round_trip(body: WireEnvelopeBody, frame: &str) -> Result<(), Box<dyn Error>> {
    let envelope = WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse(frame)?,
        sent_at: UnixMillis::from_millis(11),
        body,
    };
    assert!(decode_envelope(&encode_envelope(&envelope)?)? == envelope);
    Ok(())
}

fn snapshot() -> Result<EngineInstallSnapshot, Box<dyn Error>> {
    Ok(EngineInstallSnapshot::new(vec![
        EngineInstallStatus {
            engine_id: "claude".into(),
            phase: EngineInstallPhase::Installing,
            active_version: Some("2.1.282".into()),
            held_version: None,
            latest_version: Some("2.1.283".into()),
            pending_version: Some("2.1.283".into()),
            rollback_version: Some("2.1.281".into()),
            progress_percent: Some(42),
            reason: None,
            overridden: false,
            integrity: EngineIntegrity::VendorChecksum,
            trusted_since: None,
            vendor_version_list: true,
        },
        EngineInstallStatus {
            engine_id: "grok".into(),
            phase: EngineInstallPhase::Unsupported,
            active_version: None,
            held_version: None,
            latest_version: None,
            pending_version: None,
            rollback_version: None,
            progress_percent: None,
            reason: Some("Grok Build is not managed here.".into()),
            overridden: true,
            integrity: EngineIntegrity::TrustOnFirstDownload,
            trusted_since: Some("2026-09-26T09:30:00.000Z".into()),
            vendor_version_list: false,
        },
        EngineInstallStatus {
            engine_id: "codex".into(),
            phase: EngineInstallPhase::Ready,
            active_version: Some("0.157.1".into()),
            held_version: Some("0.157.1".into()),
            latest_version: None,
            pending_version: None,
            rollback_version: None,
            progress_percent: Some(0),
            reason: None,
            overridden: false,
            integrity: EngineIntegrity::VendorChecksum,
            trusted_since: None,
            vendor_version_list: true,
        },
    ])?)
}

#[test]
fn engine_install_requests_round_trip() -> Result<(), Box<dyn Error>> {
    round_trip(
        WireEnvelopeBody::Request(ClientRequest::Query(Query::ReadEngineInstalls(
            ReadEngineInstalls,
        ))),
        "read-engine-installs",
    )?;
    round_trip(
        WireEnvelopeBody::Request(ClientRequest::Query(Query::ListEngineVersions(
            ListEngineVersions {
                engine_id: "codex".into(),
            },
        ))),
        "list-engine-versions",
    )?;
    for change in [
        EngineVersionChange::Select(EngineVersionSelection::Latest),
        EngineVersionChange::Select(EngineVersionSelection::Version("2.1.282".into())),
        EngineVersionChange::Rollback,
    ] {
        round_trip(
            WireEnvelopeBody::Request(ClientRequest::Query(Query::ChangeEngineVersion(
                ChangeEngineVersion {
                    engine_id: "claude".into(),
                    change,
                },
            ))),
            "change-engine-version",
        )?;
    }
    Ok(())
}

#[test]
fn engine_install_answers_and_push_round_trip() -> Result<(), Box<dyn Error>> {
    for snapshot in [EngineInstallSnapshot::default(), snapshot()?] {
        round_trip(
            WireEnvelopeBody::Response(ServerResponse {
                request_id: RequestId::parse("read-engine-installs")?,
                payload: ResponsePayload::EngineInstalls(snapshot),
            }),
            "engine-installs",
        )?;
    }
    round_trip(
        WireEnvelopeBody::Response(ServerResponse {
            request_id: RequestId::parse("list-engine-versions")?,
            payload: ResponsePayload::EngineVersions(EngineVersionList::new(
                "codex".into(),
                vec![
                    EngineVersionEntry {
                        version: "0.157.1".into(),
                        installed: true,
                        active: true,
                        below_floor: false,
                    },
                    EngineVersionEntry {
                        version: "0.100.0".into(),
                        installed: false,
                        active: false,
                        below_floor: true,
                    },
                ],
            )?),
        }),
        "engine-versions",
    )?;
    round_trip(
        WireEnvelopeBody::Event(ServerEvent {
            cursor: EventCursor::new(3)?,
            event: Event::EngineInstalls(snapshot()?),
        }),
        "engine-installs-push",
    )
}

#[test]
fn invalid_engine_requests_do_not_decode() -> Result<(), Box<dyn Error>> {
    let envelope = WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("bad-engine")?,
        sent_at: UnixMillis::from_millis(11),
        body: WireEnvelopeBody::Request(ClientRequest::Query(Query::ChangeEngineVersion(
            ChangeEngineVersion {
                engine_id: "claude code".into(),
                change: EngineVersionChange::Rollback,
            },
        ))),
    };
    assert!(decode_envelope(&encode_envelope(&envelope)?).is_err());
    Ok(())
}
