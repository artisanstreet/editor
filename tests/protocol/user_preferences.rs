//! Owned protocol coverage for the Forge user's preferences, navigation
//! record, and account profile (stateless Editor step 7).

use std::error::Error;

use artisan_domain::{
    AccountProfile, ApprovalMode, ByteLimit, CatalogOptionId, CatalogSelection, Command,
    CountLimit, DisplayName, EngineAgentId, EngineModelId, EnginePermissionPolicy, EngineProfileId,
    EngineRouteId, EngineRunConfig, EngineRuntimeControls, EngineRuntimeControlsInput,
    EngineSelection, FilesystemAccess, FiniteMillis, ImportLegacyPreferences, LegacyImportOutcome,
    LegacyPreferencesImported, ModelFavoriteId, NavigationProject, NavigationRecord,
    NavigationRoute, NetworkAccess, OpenCode2Selection, PermissionId, ProjectId, Query,
    ReadUserPreferences, RecordNavigation, RequestId, ThreadId, UnixMillis, UserPreferences,
    WebSearchAccess,
};
use artisan_protocol::{
    ClientRequest, FrameId, ProtocolVersion, ResponsePayload, ServerResponse, WireEnvelope,
    WireEnvelopeBody, decode_envelope, encode_envelope,
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

fn default_config() -> EngineRunConfig {
    let one = FiniteMillis::new(1).expect("one millisecond");
    let bytes = |value| ByteLimit::new(value).expect("byte limit");
    let count = |value| CountLimit::new(value).expect("count limit");
    let runtime = EngineRuntimeControls::new(EngineRuntimeControlsInput {
        attempt_budget: FiniteMillis::new(100).expect("attempt budget"),
        readiness_budget: one,
        health_budget: one,
        prompt_budget: one,
        stream_budget: one,
        close_budget: one,
        max_json_body_bytes: bytes(8_192),
        max_sse_line_bytes: bytes(4_096),
        max_sse_event_bytes: bytes(8_192),
        max_readiness_line_bytes: bytes(4_096),
        max_header_count: count(8),
        max_http_buffer_bytes: bytes(8_192),
        max_stderr_bytes: bytes(4_096),
        observation_capacity: count(16),
    })
    .expect("runtime");
    EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            EngineProfileId::parse("profile").expect("profile"),
            EngineModelId::parse("model").expect("model"),
            EngineRouteId::parse("route").expect("route"),
            None,
            EnginePermissionPolicy::new(
                PermissionId::parse("permission").expect("permission"),
                EngineAgentId::parse("agent").expect("agent"),
                ApprovalMode::OnRequest,
                FilesystemAccess::Workspace,
                NetworkAccess::Enabled,
                WebSearchAccess::Disabled,
            ),
        )),
        runtime,
    )
}

fn preferences(with_config: bool) -> Result<UserPreferences, Box<dyn Error>> {
    let first = ProjectId::parse("project-a")?;
    let thread = ThreadId::parse("thread-a")?;
    Ok(UserPreferences {
        revision: 9,
        default_engine_config: with_config.then(default_config),
        navigation: NavigationRecord::new(
            vec![
                NavigationProject {
                    project_id: first.clone(),
                    last_thread_id: Some(thread.clone()),
                },
                NavigationProject {
                    project_id: ProjectId::parse("project-b")?,
                    last_thread_id: None,
                },
            ],
            with_config.then(|| NavigationRoute {
                project_id: first,
                thread_id: Some(thread),
            }),
        )?,
        account: AccountProfile {
            display_name: DisplayName::parse("theo")?,
            host_name: DisplayName::parse("ubuntu")?,
        },
    })
}

#[test]
fn preference_requests_round_trip() -> Result<(), Box<dyn Error>> {
    round_trip(
        WireEnvelopeBody::Request(ClientRequest::Query(Query::ReadUserPreferences(
            ReadUserPreferences,
        ))),
        "read-preferences",
    )?;
    for thread in [None, Some(ThreadId::parse("thread-a")?)] {
        round_trip(
            WireEnvelopeBody::Request(ClientRequest::Command(Command::RecordNavigation(
                RecordNavigation {
                    request_id: RequestId::parse("record-navigation")?,
                    project_id: ProjectId::parse("project-a")?,
                    thread_id: thread,
                },
            ))),
            "record-navigation",
        )?;
    }
    let selection = CatalogSelection {
        model_id: ModelFavoriteId::parse("codex-sol")?,
        profile_id: None,
        reasoning_effort: Some(CatalogOptionId::parse("low")?),
        speed: None,
        context_window: None,
        permission: None,
    };
    for default_selection in [None, Some(selection)] {
        round_trip(
            WireEnvelopeBody::Request(ClientRequest::Command(Command::ImportLegacyPreferences(
                ImportLegacyPreferences {
                    request_id: RequestId::parse("import-legacy")?,
                    default_selection,
                    project_order: vec![ProjectId::parse("b")?, ProjectId::parse("a")?],
                },
            ))),
            "import-legacy",
        )?;
    }
    Ok(())
}

#[test]
fn preference_answers_round_trip() -> Result<(), Box<dyn Error>> {
    for with_config in [false, true] {
        round_trip(
            WireEnvelopeBody::Response(ServerResponse {
                request_id: RequestId::parse("read-preferences")?,
                payload: ResponsePayload::UserPreferences(preferences(with_config)?),
            }),
            "preferences",
        )?;
    }
    round_trip(
        WireEnvelopeBody::Response(ServerResponse {
            request_id: RequestId::parse("import-legacy")?,
            payload: ResponsePayload::LegacyPreferencesImported(LegacyPreferencesImported {
                default_model: LegacyImportOutcome::Refused,
                project_order: LegacyImportOutcome::Imported,
                preferences: preferences(true)?,
            }),
        }),
        "legacy-imported",
    )
}

#[test]
fn pushed_host_state_events_round_trip() -> Result<(), Box<dyn Error>> {
    use artisan_domain::{
        EngineUsageAuth, EngineUsageAuthentication, EngineUsageReport, EngineUsageSnapshot, Event,
        QuotaSurface, ThreadRetitled, ThreadTitle,
    };
    use artisan_protocol::{EventCursor, ServerEvent};

    let report = EngineUsageReport::new(
        Some("theo@example.com".to_owned()),
        EngineUsageAuth::new(EngineUsageAuthentication::Authenticated, None)?,
        "Codex".to_owned(),
        "codex".to_owned(),
        None,
        Some(QuotaSurface::Unknown),
        Vec::new(),
    )?;
    let usage = EngineUsageSnapshot::new(vec![report], "2026-09-25T12:00:00Z".to_owned())?;
    let events = [
        Event::AccountUsage(usage),
        Event::UserPreferences(preferences(true)?),
        Event::ThreadRetitled(ThreadRetitled {
            thread_id: ThreadId::parse("thread-a")?,
            title: ThreadTitle::parse("Refined title")?,
        }),
    ];
    for (index, event) in events.into_iter().enumerate() {
        round_trip(
            WireEnvelopeBody::Event(ServerEvent {
                cursor: EventCursor::new(u64::try_from(index)? + 1)?,
                event,
            }),
            "host-state",
        )?;
    }
    Ok(())
}
