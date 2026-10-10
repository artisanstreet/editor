//! Owned protocol coverage for agent extensions and the global prompt: the
//! reads, enable and adopt commands, prompt saves and resolutions, the
//! answers, and the pushed events.

use std::error::Error;

use artisan_domain::{
    AdoptExtension, Command, ConnectorStatus, EngineInventory, Event, Extension, ExtensionCatalog,
    ExtensionEngineState, ExtensionEngineStatus, ExtensionKind, ExtensionLinkSupport,
    ExtensionOrigin, GlobalPrompt, GlobalPromptCandidate, GlobalPromptEngineState,
    GlobalPromptEngineStatus, GlobalPromptResolution, GlobalPromptRevision, GlobalPromptSetup,
    InventoryConnector, InventorySkill, InventorySkillSource, Query, ReadExtensions,
    ReadGlobalPrompt, RequestId, ResolveGlobalPrompt, SaveGlobalPrompt, SetExtensionEnabled,
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

fn catalog() -> Result<ExtensionCatalog, Box<dyn Error>> {
    let engine = |engine_id: &str, status, reason: Option<&str>| ExtensionEngineState {
        engine_id: engine_id.to_owned(),
        status,
        reason: reason.map(str::to_owned),
    };
    Ok(ExtensionCatalog::new(
        vec![
            Extension {
                id: "artisan.artisan-editor".to_owned(),
                kind: ExtensionKind::Skill,
                origin: ExtensionOrigin::FirstParty,
                name: "artisan-editor".to_owned(),
                description: "Write replies that render richly.".to_owned(),
                digest: "a".repeat(64),
                file_count: 2,
                byte_count: 40_960,
                enabled: true,
                engines: vec![
                    engine("claude", ExtensionEngineStatus::Synced, None),
                    engine(
                        "codex",
                        ExtensionEngineStatus::Conflict,
                        Some("Codex has its own skill named artisan-editor."),
                    ),
                    engine(
                        "cursor",
                        ExtensionEngineStatus::Unavailable,
                        Some("Cursor runs are not available in Artisan yet."),
                    ),
                ],
            },
            Extension {
                id: "discovered.pdf.bbbbbbbbbbbb".to_owned(),
                kind: ExtensionKind::Skill,
                origin: ExtensionOrigin::Discovered,
                name: "pdf".to_owned(),
                description: String::new(),
                digest: "b".repeat(64),
                file_count: 1,
                byte_count: 12,
                enabled: false,
                engines: vec![
                    engine("claude", ExtensionEngineStatus::Native, None),
                    engine("codex", ExtensionEngineStatus::NotPresent, None),
                ],
            },
        ],
        ExtensionLinkSupport::DeveloperModeRequired,
    )?
    .with_inventories(vec![
        EngineInventory {
            engine_id: "claude".to_owned(),
            project_id: Some("project-1".to_owned()),
            skills: vec![
                InventorySkill {
                    name: "cloudflare:wrangler".to_owned(),
                    description: "Run Wrangler.".to_owned(),
                    source: InventorySkillSource::Plugin,
                    plugin: Some("cloudflare".to_owned()),
                },
                InventorySkill {
                    name: "simplify".to_owned(),
                    description: String::new(),
                    source: InventorySkillSource::System,
                    plugin: None,
                },
            ],
            connectors: vec![
                InventoryConnector {
                    name: "claude.ai Notion".to_owned(),
                    status: ConnectorStatus::NeedsAuth,
                    plugin: None,
                },
                InventoryConnector {
                    name: "cloudflare-api".to_owned(),
                    status: ConnectorStatus::Connected,
                    plugin: Some("cloudflare".to_owned()),
                },
            ],
        },
        // An engine that ran outside any project and has listed no skills.
        EngineInventory {
            engine_id: "codex".to_owned(),
            project_id: None,
            skills: Vec::new(),
            connectors: vec![InventoryConnector {
                name: "sentry".to_owned(),
                status: ConnectorStatus::Pending,
                plugin: None,
            }],
        },
    ])?)
}

fn prompt(setup: GlobalPromptSetup) -> Result<GlobalPrompt, Box<dyn Error>> {
    Ok(GlobalPrompt::new(
        GlobalPromptRevision::new(4),
        "Use British spelling.\n".to_owned(),
        setup,
        vec![
            GlobalPromptEngineState {
                engine_id: "claude".to_owned(),
                status: GlobalPromptEngineStatus::Synced,
                file: Some(".claude/CLAUDE.md".to_owned()),
                external_text: None,
                reason: None,
            },
            GlobalPromptEngineState {
                engine_id: "codex".to_owned(),
                status: GlobalPromptEngineStatus::Drifted,
                file: Some(".codex/AGENTS.md".to_owned()),
                // An emptied file is drift too, distinct from no drift.
                external_text: Some(String::new()),
                reason: None,
            },
            GlobalPromptEngineState {
                engine_id: "cursor".to_owned(),
                status: GlobalPromptEngineStatus::Unavailable,
                file: None,
                external_text: None,
                reason: Some("Cursor runs are not available in Artisan yet.".to_owned()),
            },
        ],
    )?)
}

fn choice() -> GlobalPromptSetup {
    GlobalPromptSetup::ChoiceRequired(vec![
        GlobalPromptCandidate {
            candidate_id: "c".repeat(64),
            engine_ids: vec!["claude".to_owned(), "grok".to_owned()],
            text: "Be brief.".to_owned(),
        },
        GlobalPromptCandidate {
            candidate_id: "d".repeat(64),
            engine_ids: vec!["codex".to_owned()],
            text: "Be thorough.".to_owned(),
        },
    ])
}

#[test]
fn extension_and_prompt_requests_round_trip() -> Result<(), Box<dyn Error>> {
    let request = |frame: &str| RequestId::parse(frame);
    round_trip(
        WireEnvelopeBody::Request(ClientRequest::Query(Query::ReadExtensions(ReadExtensions))),
        "read-extensions",
    )?;
    round_trip(
        WireEnvelopeBody::Request(ClientRequest::Query(Query::ReadGlobalPrompt(
            ReadGlobalPrompt,
        ))),
        "read-prompt",
    )?;
    round_trip(
        WireEnvelopeBody::Request(ClientRequest::Command(Command::SetExtensionEnabled(
            SetExtensionEnabled {
                request_id: request("enable")?,
                extension_id: "artisan.artisan-editor".to_owned(),
                enabled: false,
            },
        ))),
        "enable",
    )?;
    round_trip(
        WireEnvelopeBody::Request(ClientRequest::Command(Command::AdoptExtension(
            AdoptExtension {
                request_id: request("adopt")?,
                extension_id: "discovered.pdf.bbbbbbbbbbbb".to_owned(),
            },
        ))),
        "adopt",
    )?;
    round_trip(
        WireEnvelopeBody::Request(ClientRequest::Command(Command::SaveGlobalPrompt(
            SaveGlobalPrompt {
                request_id: request("save")?,
                base_revision: GlobalPromptRevision::new(4),
                text: "Line one.\n\tLine two.\n".to_owned(),
            },
        ))),
        "save",
    )?;
    for (frame, resolution) in [
        (
            "choose",
            GlobalPromptResolution::ChooseCandidate {
                candidate_id: "c".repeat(64),
            },
        ),
        (
            "import",
            GlobalPromptResolution::ImportEngine {
                engine_id: "codex".to_owned(),
            },
        ),
        (
            "overwrite",
            GlobalPromptResolution::OverwriteEngine {
                engine_id: "codex".to_owned(),
            },
        ),
    ] {
        round_trip(
            WireEnvelopeBody::Request(ClientRequest::Command(Command::ResolveGlobalPrompt(
                ResolveGlobalPrompt {
                    request_id: request(frame)?,
                    resolution,
                },
            ))),
            frame,
        )?;
    }
    Ok(())
}

#[test]
fn extension_and_prompt_answers_round_trip() -> Result<(), Box<dyn Error>> {
    round_trip(
        WireEnvelopeBody::Response(ServerResponse {
            request_id: RequestId::parse("catalog")?,
            payload: ResponsePayload::Extensions(catalog()?),
        }),
        "catalog",
    )?;
    for (frame, setup) in [("ready", GlobalPromptSetup::Ready), ("choice", choice())] {
        round_trip(
            WireEnvelopeBody::Response(ServerResponse {
                request_id: RequestId::parse(frame)?,
                payload: ResponsePayload::GlobalPrompt(prompt(setup)?),
            }),
            frame,
        )?;
    }
    Ok(())
}

#[test]
fn extension_and_prompt_events_round_trip() -> Result<(), Box<dyn Error>> {
    for (sequence, event) in [
        (1, Event::Extensions(catalog()?)),
        (2, Event::GlobalPrompt(prompt(choice())?)),
    ] {
        round_trip(
            WireEnvelopeBody::Event(ServerEvent {
                cursor: EventCursor::new(sequence)?,
                event,
            }),
            &format!("event-{sequence}"),
        )?;
    }
    Ok(())
}
