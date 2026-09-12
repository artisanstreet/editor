//! Canonical, bounded persistence codec for an engine run configuration.
//!
//! This module intentionally stays private to the database crate. The wire
//! protocol has its own conversion, while this codec is the only authority
//! for the bytes stored in SQLite and in immutable assistant-run snapshots.

use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use artisan_domain::{
    ApprovalMode, ByteLimit, ClaudeEffort, ClaudePermissionMode, ClaudeSelection,
    CodexModelContextWindow, CodexReasoningEffort, CodexSelection, CodexServiceTier, CountLimit,
    CursorPermissionMode, CursorReasoningEffort, CursorSelection, CursorSpeed,
    ENGINE_CONFIG_MAX_ENCODED_BYTES, EngineAgentId, EngineConfigError, EngineId, EngineModelId,
    EnginePermissionPolicy, EngineProfileId, EngineRouteId, EngineRunConfig, EngineRuntimeControls,
    EngineRuntimeControlsInput, EngineSelection, EngineVariantId, FilesystemAccess, FiniteMillis,
    GrokPermissionMode, GrokReasoningEffort, GrokSelection, NetworkAccess, OpenCode2Selection,
    PermissionId,
    WebSearchAccess,
};

#[derive(Debug, Error)]
pub(crate) enum EngineRunConfigCodecError {
    #[error("engine configuration blob exceeds its bound")]
    TooLarge,
    #[error("engine configuration blob is malformed")]
    Malformed,
    #[error("engine configuration blob has a non-canonical shape")]
    NonCanonical,
    #[error("engine configuration field is invalid: {field}")]
    InvalidField { field: &'static str },
    #[error("engine configuration could not be encoded")]
    Encode,
}

#[derive(Serialize)]
struct StoredConfig<'a> {
    version: u16,
    engine: &'static str,
    profile_id: &'a str,
    model_id: &'a str,
    route_id: &'a str,
    variant_id: Option<&'a str>,
    permission: StoredPermission<'a>,
    runtime: StoredRuntime,
}

#[derive(Serialize)]
struct StoredPermission<'a> {
    permission_id: &'a str,
    agent_id: &'a str,
    approval: &'static str,
    filesystem: &'static str,
    network: &'static str,
    web_search: &'static str,
}

#[derive(Serialize)]
struct StoredRuntime {
    attempt_budget_ms: u64,
    readiness_budget_ms: u64,
    health_budget_ms: u64,
    prompt_budget_ms: u64,
    stream_budget_ms: u64,
    close_budget_ms: u64,
    max_json_body_bytes: u64,
    max_sse_line_bytes: u64,
    max_sse_event_bytes: u64,
    max_readiness_line_bytes: u64,
    max_header_count: u64,
    max_http_buffer_bytes: u64,
    max_stderr_bytes: u64,
    observation_capacity: u64,
}

#[derive(Debug)]
struct RawConfig {
    version: u16,
    engine: String,
    profile_id: String,
    model_id: String,
    route_id: String,
    variant_id: Option<String>,
    permission: RawPermission,
    runtime: RawRuntime,
}

#[derive(Debug)]
struct RawPermission {
    permission_id: String,
    agent_id: String,
    approval: String,
    filesystem: String,
    network: String,
    web_search: String,
}

#[derive(Debug)]
struct RawRuntime {
    attempt_budget_ms: u64,
    readiness_budget_ms: u64,
    health_budget_ms: u64,
    prompt_budget_ms: u64,
    stream_budget_ms: u64,
    close_budget_ms: u64,
    max_json_body_bytes: u64,
    max_sse_line_bytes: u64,
    max_sse_event_bytes: u64,
    max_readiness_line_bytes: u64,
    max_header_count: u64,
    max_http_buffer_bytes: u64,
    max_stderr_bytes: u64,
    observation_capacity: u64,
}

fn invalid<E: de::Error>(_: &'static str) -> E {
    E::custom("invalid engine configuration field")
}

fn require_key<'de, M: MapAccess<'de>>(map: &mut M, expected: &'static str) -> Result<(), M::Error>
where
    M::Error: de::Error,
{
    match map.next_key::<String>()? {
        Some(key) if key == expected => Ok(()),
        Some(_) | None => Err(invalid(expected)),
    }
}

fn next_value<'de, M, T>(map: &mut M, field: &'static str) -> Result<T, M::Error>
where
    M: MapAccess<'de>,
    T: Deserialize<'de>,
    M::Error: de::Error,
{
    map.next_value().map_err(|_| invalid(field))
}

impl<'de> Deserialize<'de> for RawConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RawConfigVisitor;

        impl<'de> Visitor<'de> for RawConfigVisitor {
            type Value = RawConfig;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("the canonical engine configuration object")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                require_key(&mut map, "version")?;
                let version = next_value(&mut map, "version")?;
                require_key(&mut map, "engine")?;
                let engine = next_value(&mut map, "engine")?;
                require_key(&mut map, "profile_id")?;
                let profile_id = next_value(&mut map, "profile_id")?;
                require_key(&mut map, "model_id")?;
                let model_id = next_value(&mut map, "model_id")?;
                require_key(&mut map, "route_id")?;
                let route_id = next_value(&mut map, "route_id")?;
                require_key(&mut map, "variant_id")?;
                let variant_id = next_value(&mut map, "variant_id")?;
                require_key(&mut map, "permission")?;
                let permission = next_value(&mut map, "permission")?;
                require_key(&mut map, "runtime")?;
                let runtime = next_value(&mut map, "runtime")?;
                if map.next_key::<String>()?.is_some() {
                    return Err(invalid("object"));
                }
                Ok(RawConfig {
                    version,
                    engine,
                    profile_id,
                    model_id,
                    route_id,
                    variant_id,
                    permission,
                    runtime,
                })
            }
        }

        deserializer.deserialize_map(RawConfigVisitor)
    }
}

impl<'de> Deserialize<'de> for RawPermission {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RawPermissionVisitor;

        impl<'de> Visitor<'de> for RawPermissionVisitor {
            type Value = RawPermission;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("the canonical engine permission object")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                require_key(&mut map, "permission_id")?;
                let permission_id = next_value(&mut map, "permission_id")?;
                require_key(&mut map, "agent_id")?;
                let agent_id = next_value(&mut map, "agent_id")?;
                require_key(&mut map, "approval")?;
                let approval = next_value(&mut map, "approval")?;
                require_key(&mut map, "filesystem")?;
                let filesystem = next_value(&mut map, "filesystem")?;
                require_key(&mut map, "network")?;
                let network = next_value(&mut map, "network")?;
                require_key(&mut map, "web_search")?;
                let web_search = next_value(&mut map, "web_search")?;
                if map.next_key::<String>()?.is_some() {
                    return Err(invalid("permission"));
                }
                Ok(RawPermission {
                    permission_id,
                    agent_id,
                    approval,
                    filesystem,
                    network,
                    web_search,
                })
            }
        }

        deserializer.deserialize_map(RawPermissionVisitor)
    }
}

impl<'de> Deserialize<'de> for RawRuntime {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RawRuntimeVisitor;

        impl<'de> Visitor<'de> for RawRuntimeVisitor {
            type Value = RawRuntime;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("the canonical engine runtime object")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                require_key(&mut map, "attempt_budget_ms")?;
                let attempt_budget_ms = next_value(&mut map, "attempt_budget_ms")?;
                require_key(&mut map, "readiness_budget_ms")?;
                let readiness_budget_ms = next_value(&mut map, "readiness_budget_ms")?;
                require_key(&mut map, "health_budget_ms")?;
                let health_budget_ms = next_value(&mut map, "health_budget_ms")?;
                require_key(&mut map, "prompt_budget_ms")?;
                let prompt_budget_ms = next_value(&mut map, "prompt_budget_ms")?;
                require_key(&mut map, "stream_budget_ms")?;
                let stream_budget_ms = next_value(&mut map, "stream_budget_ms")?;
                require_key(&mut map, "close_budget_ms")?;
                let close_budget_ms = next_value(&mut map, "close_budget_ms")?;
                require_key(&mut map, "max_json_body_bytes")?;
                let max_json_body_bytes = next_value(&mut map, "max_json_body_bytes")?;
                require_key(&mut map, "max_sse_line_bytes")?;
                let max_sse_line_bytes = next_value(&mut map, "max_sse_line_bytes")?;
                require_key(&mut map, "max_sse_event_bytes")?;
                let max_sse_event_bytes = next_value(&mut map, "max_sse_event_bytes")?;
                require_key(&mut map, "max_readiness_line_bytes")?;
                let max_readiness_line_bytes = next_value(&mut map, "max_readiness_line_bytes")?;
                require_key(&mut map, "max_header_count")?;
                let max_header_count = next_value(&mut map, "max_header_count")?;
                require_key(&mut map, "max_http_buffer_bytes")?;
                let max_http_buffer_bytes = next_value(&mut map, "max_http_buffer_bytes")?;
                require_key(&mut map, "max_stderr_bytes")?;
                let max_stderr_bytes = next_value(&mut map, "max_stderr_bytes")?;
                require_key(&mut map, "observation_capacity")?;
                let observation_capacity = next_value(&mut map, "observation_capacity")?;
                if map.next_key::<String>()?.is_some() {
                    return Err(invalid("runtime"));
                }
                Ok(RawRuntime {
                    attempt_budget_ms,
                    readiness_budget_ms,
                    health_budget_ms,
                    prompt_budget_ms,
                    stream_budget_ms,
                    close_budget_ms,
                    max_json_body_bytes,
                    max_sse_line_bytes,
                    max_sse_event_bytes,
                    max_readiness_line_bytes,
                    max_header_count,
                    max_http_buffer_bytes,
                    max_stderr_bytes,
                    observation_capacity,
                })
            }
        }

        deserializer.deserialize_map(RawRuntimeVisitor)
    }
}

fn domain_error(error: EngineConfigError) -> EngineRunConfigCodecError {
    EngineRunConfigCodecError::InvalidField {
        field: error.field(),
    }
}

fn parse_millis(
    value: u64,
    field: &'static str,
) -> Result<FiniteMillis, EngineRunConfigCodecError> {
    FiniteMillis::new(value).map_err(|_| EngineRunConfigCodecError::InvalidField { field })
}

fn parse_bytes(value: u64, field: &'static str) -> Result<ByteLimit, EngineRunConfigCodecError> {
    ByteLimit::new(value).map_err(|_| EngineRunConfigCodecError::InvalidField { field })
}

fn parse_count(value: u64, field: &'static str) -> Result<CountLimit, EngineRunConfigCodecError> {
    CountLimit::new(value).map_err(|_| EngineRunConfigCodecError::InvalidField { field })
}

fn parse_approval(value: &str) -> Result<ApprovalMode, EngineRunConfigCodecError> {
    match value {
        "never" => Ok(ApprovalMode::Never),
        "on_request" => Ok(ApprovalMode::OnRequest),
        "always" => Ok(ApprovalMode::Always),
        _ => Err(EngineRunConfigCodecError::InvalidField { field: "approval" }),
    }
}

fn parse_filesystem(value: &str) -> Result<FilesystemAccess, EngineRunConfigCodecError> {
    match value {
        "none" => Ok(FilesystemAccess::None),
        "workspace" => Ok(FilesystemAccess::Workspace),
        "host" => Ok(FilesystemAccess::Host),
        _ => Err(EngineRunConfigCodecError::InvalidField {
            field: "filesystem",
        }),
    }
}

fn parse_network(value: &str) -> Result<NetworkAccess, EngineRunConfigCodecError> {
    match value {
        "disabled" => Ok(NetworkAccess::Disabled),
        "enabled" => Ok(NetworkAccess::Enabled),
        _ => Err(EngineRunConfigCodecError::InvalidField { field: "network" }),
    }
}

fn parse_web_search(value: &str) -> Result<WebSearchAccess, EngineRunConfigCodecError> {
    match value {
        "disabled" => Ok(WebSearchAccess::Disabled),
        "enabled" => Ok(WebSearchAccess::Enabled),
        _ => Err(EngineRunConfigCodecError::InvalidField {
            field: "web_search",
        }),
    }
}

fn convert_permission(
    raw: &RawPermission,
) -> Result<EnginePermissionPolicy, EngineRunConfigCodecError> {
    let permission_id = PermissionId::parse(raw.permission_id.clone()).map_err(|_| {
        EngineRunConfigCodecError::InvalidField {
            field: "permission_id",
        }
    })?;
    let agent_id = EngineAgentId::parse(raw.agent_id.clone())
        .map_err(|_| EngineRunConfigCodecError::InvalidField { field: "agent_id" })?;
    Ok(EnginePermissionPolicy::new(
        permission_id,
        agent_id,
        parse_approval(&raw.approval)?,
        parse_filesystem(&raw.filesystem)?,
        parse_network(&raw.network)?,
        parse_web_search(&raw.web_search)?,
    ))
}

fn convert_runtime(raw: &RawRuntime) -> Result<EngineRuntimeControls, EngineRunConfigCodecError> {
    EngineRuntimeControls::new(EngineRuntimeControlsInput {
        attempt_budget: parse_millis(raw.attempt_budget_ms, "attempt_budget_ms")?,
        readiness_budget: parse_millis(raw.readiness_budget_ms, "readiness_budget_ms")?,
        health_budget: parse_millis(raw.health_budget_ms, "health_budget_ms")?,
        prompt_budget: parse_millis(raw.prompt_budget_ms, "prompt_budget_ms")?,
        stream_budget: parse_millis(raw.stream_budget_ms, "stream_budget_ms")?,
        close_budget: parse_millis(raw.close_budget_ms, "close_budget_ms")?,
        max_json_body_bytes: parse_bytes(raw.max_json_body_bytes, "max_json_body_bytes")?,
        max_sse_line_bytes: parse_bytes(raw.max_sse_line_bytes, "max_sse_line_bytes")?,
        max_sse_event_bytes: parse_bytes(raw.max_sse_event_bytes, "max_sse_event_bytes")?,
        max_readiness_line_bytes: parse_bytes(
            raw.max_readiness_line_bytes,
            "max_readiness_line_bytes",
        )?,
        max_header_count: parse_count(raw.max_header_count, "max_header_count")?,
        max_http_buffer_bytes: parse_bytes(raw.max_http_buffer_bytes, "max_http_buffer_bytes")?,
        max_stderr_bytes: parse_bytes(raw.max_stderr_bytes, "max_stderr_bytes")?,
        observation_capacity: parse_count(raw.observation_capacity, "observation_capacity")?,
    })
    .map_err(domain_error)
}

fn parse_optional_model(
    value: Option<String>,
    field: &'static str,
) -> Result<Option<EngineModelId>, EngineRunConfigCodecError> {
    value
        .map(EngineModelId::parse)
        .transpose()
        .map_err(|_| EngineRunConfigCodecError::InvalidField { field })
}

fn into_domain(raw: RawConfig) -> Result<EngineRunConfig, EngineRunConfigCodecError> {
    if raw.version != 1 {
        return Err(EngineRunConfigCodecError::InvalidField { field: "version" });
    }
    if raw.engine != EngineId::OpenCode2.as_str() {
        return Err(EngineRunConfigCodecError::InvalidField { field: "engine" });
    }

    let profile_id = EngineProfileId::parse(raw.profile_id).map_err(|_| {
        EngineRunConfigCodecError::InvalidField {
            field: "profile_id",
        }
    })?;
    let model_id = EngineModelId::parse(raw.model_id)
        .map_err(|_| EngineRunConfigCodecError::InvalidField { field: "model_id" })?;
    let route_id = EngineRouteId::parse(raw.route_id)
        .map_err(|_| EngineRunConfigCodecError::InvalidField { field: "route_id" })?;
    let variant_id = raw
        .variant_id
        .map(EngineVariantId::parse)
        .transpose()
        .map_err(|_| EngineRunConfigCodecError::InvalidField {
            field: "variant_id",
        })?;

    let permission = convert_permission(&raw.permission)?;

    let runtime = convert_runtime(&raw.runtime)?;

    Ok(EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            profile_id, model_id, route_id, variant_id, permission,
        )),
        runtime,
    ))
}

#[derive(Serialize)]
struct StoredCodexDetails<'a> {
    model_id: Option<&'a str>,
    reasoning_effort: Option<&'static str>,
    service_tier: Option<&'static str>,
    model_context_window: Option<u64>,
}

#[derive(Serialize)]
struct StoredClaudeDetails<'a> {
    model_id: Option<&'a str>,
    effort: Option<&'static str>,
    permission_mode: Option<&'static str>,
    disable_tools: bool,
    safe_mode: bool,
}

#[derive(Serialize)]
struct StoredGrokDetails<'a> {
    model_id: Option<&'a str>,
    reasoning_effort: Option<&'a str>,
    permission_mode: Option<&'static str>,
}

#[derive(Serialize)]
struct StoredCursorDetails<'a> {
    model_id: Option<&'a str>,
    reasoning_effort: Option<&'a str>,
    speed: Option<&'static str>,
    permission_mode: Option<&'static str>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum StoredDetails<'a> {
    Codex(StoredCodexDetails<'a>),
    Claude(StoredClaudeDetails<'a>),
    Grok(StoredGrokDetails<'a>),
    Cursor(StoredCursorDetails<'a>),
}

#[derive(Serialize)]
struct StoredConfigV2<'a> {
    version: u16,
    engine: &'static str,
    profile_id: &'a str,
    permission: Option<StoredPermission<'a>>,
    runtime: StoredRuntime,
    details: StoredDetails<'a>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfigV2 {
    version: u16,
    engine: String,
    profile_id: String,
    permission: Option<RawPermission>,
    runtime: RawRuntime,
    details: serde_json::Value,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCodexDetails {
    model_id: Option<String>,
    reasoning_effort: Option<String>,
    service_tier: Option<String>,
    model_context_window: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawClaudeDetails {
    model_id: Option<String>,
    effort: Option<String>,
    permission_mode: Option<String>,
    disable_tools: bool,
    safe_mode: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGrokDetails {
    model_id: Option<String>,
    reasoning_effort: Option<String>,
    permission_mode: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCursorDetails {
    model_id: Option<String>,
    reasoning_effort: Option<String>,
    speed: Option<String>,
    permission_mode: Option<String>,
}

#[derive(Deserialize)]
struct VersionPeek {
    version: u16,
}

#[expect(
    clippy::too_many_lines,
    reason = "the v2 decoder is one linear field-by-field conversion with per-field error mapping; \
              extraction would fragment the field-to-error table"
)]
fn into_domain_v2(raw: RawConfigV2) -> Result<EngineRunConfig, EngineRunConfigCodecError> {
    if raw.version != 2 {
        return Err(EngineRunConfigCodecError::InvalidField { field: "version" });
    }
    let engine = EngineId::parse(&raw.engine)
        .map_err(|_| EngineRunConfigCodecError::InvalidField { field: "engine" })?;
    if engine == EngineId::OpenCode2 {
        return Err(EngineRunConfigCodecError::InvalidField { field: "engine" });
    }
    let profile_id = EngineProfileId::parse(raw.profile_id).map_err(|_| {
        EngineRunConfigCodecError::InvalidField {
            field: "profile_id",
        }
    })?;
    let permission = raw
        .permission
        .as_ref()
        .map(convert_permission)
        .transpose()?;
    let runtime = convert_runtime(&raw.runtime)?;
    let selection = match engine {
        EngineId::OpenCode2 => {
            return Err(EngineRunConfigCodecError::InvalidField { field: "engine" });
        }
        EngineId::Codex => {
            let permission = permission.ok_or(EngineRunConfigCodecError::InvalidField {
                field: "permission",
            })?;
            let details: RawCodexDetails = serde_json::from_value(raw.details)
                .map_err(|_| EngineRunConfigCodecError::Malformed)?;
            EngineSelection::Codex(
                CodexSelection::new(
                    profile_id,
                    parse_optional_model(details.model_id, "model_id")?,
                    permission,
                    details
                        .reasoning_effort
                        .as_deref()
                        .map(CodexReasoningEffort::parse)
                        .transpose()
                        .map_err(domain_error)?,
                    details
                        .service_tier
                        .as_deref()
                        .map(CodexServiceTier::parse)
                        .transpose()
                        .map_err(domain_error)?,
                    details
                        .model_context_window
                        .map(CodexModelContextWindow::new)
                        .transpose()
                        .map_err(domain_error)?,
                )
                .map_err(domain_error)?,
            )
        }
        EngineId::Claude => {
            let permission = permission.ok_or(EngineRunConfigCodecError::InvalidField {
                field: "permission",
            })?;
            let details: RawClaudeDetails = serde_json::from_value(raw.details)
                .map_err(|_| EngineRunConfigCodecError::Malformed)?;
            EngineSelection::Claude(
                ClaudeSelection::new(
                    profile_id,
                    parse_optional_model(details.model_id, "model_id")?,
                    permission,
                    details
                        .effort
                        .as_deref()
                        .map(ClaudeEffort::parse)
                        .transpose()
                        .map_err(domain_error)?,
                    details
                        .permission_mode
                        .as_deref()
                        .map(ClaudePermissionMode::parse)
                        .transpose()
                        .map_err(domain_error)?,
                    details.disable_tools,
                    details.safe_mode,
                )
                .map_err(domain_error)?,
            )
        }
        EngineId::Grok => {
            let permission = permission.ok_or(EngineRunConfigCodecError::InvalidField {
                field: "permission",
            })?;
            let details: RawGrokDetails = serde_json::from_value(raw.details)
                .map_err(|_| EngineRunConfigCodecError::Malformed)?;
            EngineSelection::Grok(GrokSelection::new(
                profile_id,
                parse_optional_model(details.model_id, "model_id")?,
                permission,
                details
                    .reasoning_effort
                    .map(GrokReasoningEffort::parse)
                    .transpose()
                    .map_err(domain_error)?,
                details
                    .permission_mode
                    .as_deref()
                    .map(GrokPermissionMode::parse)
                    .transpose()
                    .map_err(domain_error)?,
            ))
        }
        EngineId::Cursor => {
            let permission = permission.ok_or(EngineRunConfigCodecError::InvalidField {
                field: "permission",
            })?;
            let details: RawCursorDetails = serde_json::from_value(raw.details)
                .map_err(|_| EngineRunConfigCodecError::Malformed)?;
            EngineSelection::Cursor(CursorSelection::new(
                profile_id,
                parse_optional_model(details.model_id, "model_id")?,
                permission,
                details
                    .reasoning_effort
                    .map(CursorReasoningEffort::parse)
                    .transpose()
                    .map_err(domain_error)?,
                details
                    .speed
                    .as_deref()
                    .map(CursorSpeed::parse)
                    .transpose()
                    .map_err(domain_error)?,
                details
                    .permission_mode
                    .as_deref()
                    .map(CursorPermissionMode::parse)
                    .transpose()
                    .map_err(domain_error)?,
            ))
        }
    };
    Ok(EngineRunConfig::new(selection, runtime))
}

fn stored_permission(permission: &EnginePermissionPolicy) -> StoredPermission<'_> {
    StoredPermission {
        permission_id: permission.permission_id().as_str(),
        agent_id: permission.agent_id().as_str(),
        approval: permission.approval().as_str(),
        filesystem: permission.filesystem().as_str(),
        network: permission.network().as_str(),
        web_search: permission.web_search().as_str(),
    }
}

fn stored_runtime(runtime: EngineRuntimeControls) -> StoredRuntime {
    StoredRuntime {
        attempt_budget_ms: runtime.attempt_budget().get(),
        readiness_budget_ms: runtime.readiness_budget().get(),
        health_budget_ms: runtime.health_budget().get(),
        prompt_budget_ms: runtime.prompt_budget().get(),
        stream_budget_ms: runtime.stream_budget().get(),
        close_budget_ms: runtime.close_budget().get(),
        max_json_body_bytes: runtime.max_json_body_bytes().get(),
        max_sse_line_bytes: runtime.max_sse_line_bytes().get(),
        max_sse_event_bytes: runtime.max_sse_event_bytes().get(),
        max_readiness_line_bytes: runtime.max_readiness_line_bytes().get(),
        max_header_count: runtime.max_header_count().get(),
        max_http_buffer_bytes: runtime.max_http_buffer_bytes().get(),
        max_stderr_bytes: runtime.max_stderr_bytes().get(),
        observation_capacity: runtime.observation_capacity().get(),
    }
}

/// Encodes the legacy version 1 shape byte-identically to previous
/// revisions. Only `OpenCode` 2 selections use this path.
fn encode_v1(
    selection: &OpenCode2Selection,
    runtime: EngineRuntimeControls,
) -> Result<Vec<u8>, EngineRunConfigCodecError> {
    let stored = StoredConfig {
        version: 1,
        engine: EngineId::OpenCode2.as_str(),
        profile_id: selection.profile_id().as_str(),
        model_id: selection.model_id().as_str(),
        route_id: selection.route_id().as_str(),
        variant_id: selection.variant_id().map(EngineVariantId::as_str),
        permission: stored_permission(selection.permission()),
        runtime: stored_runtime(runtime),
    };
    serde_json::to_vec(&stored).map_err(|_| EngineRunConfigCodecError::Encode)
}

fn encode_v2(
    engine: EngineId,
    profile_id: &str,
    permission: Option<&EnginePermissionPolicy>,
    runtime: EngineRuntimeControls,
    details: StoredDetails<'_>,
) -> Result<Vec<u8>, EngineRunConfigCodecError> {
    let stored = StoredConfigV2 {
        version: 2,
        engine: engine.as_str(),
        profile_id,
        permission: permission.map(stored_permission),
        runtime: stored_runtime(runtime),
        details,
    };
    serde_json::to_vec(&stored).map_err(|_| EngineRunConfigCodecError::Encode)
}

/// Encodes one configuration into canonical bounded JSON bytes.
///
/// `OpenCode` 2 selections encode as version 1, byte-identical to previous
/// revisions. Every other engine encodes as the tagged version 2 shape.
pub(crate) fn encode(config: &EngineRunConfig) -> Result<Vec<u8>, EngineRunConfigCodecError> {
    let encoded = match config.selection() {
        EngineSelection::OpenCode2(selection) => encode_v1(selection, config.runtime())?,
        EngineSelection::Codex(selection) => encode_v2(
            EngineId::Codex,
            selection.profile_id().as_str(),
            Some(selection.permission()),
            config.runtime(),
            StoredDetails::Codex(StoredCodexDetails {
                model_id: selection.model_id().map(EngineModelId::as_str),
                reasoning_effort: selection
                    .reasoning_effort()
                    .map(CodexReasoningEffort::as_str),
                service_tier: selection.service_tier().map(CodexServiceTier::as_str),
                model_context_window: selection
                    .model_context_window()
                    .map(CodexModelContextWindow::get),
            }),
        )?,
        EngineSelection::Claude(selection) => encode_v2(
            EngineId::Claude,
            selection.profile_id().as_str(),
            Some(selection.permission()),
            config.runtime(),
            StoredDetails::Claude(StoredClaudeDetails {
                model_id: selection.model_id().map(EngineModelId::as_str),
                effort: selection.effort().map(ClaudeEffort::as_str),
                permission_mode: selection
                    .permission_mode()
                    .map(ClaudePermissionMode::as_str),
                disable_tools: selection.disable_tools(),
                safe_mode: selection.safe_mode(),
            }),
        )?,
        EngineSelection::Grok(selection) => encode_v2(
            EngineId::Grok,
            selection.profile_id().as_str(),
            Some(selection.permission()),
            config.runtime(),
            StoredDetails::Grok(StoredGrokDetails {
                model_id: selection.model_id().map(EngineModelId::as_str),
                reasoning_effort: selection
                    .reasoning_effort()
                    .map(GrokReasoningEffort::as_str),
                permission_mode: selection.permission_mode().map(GrokPermissionMode::as_str),
            }),
        )?,
        EngineSelection::Cursor(selection) => encode_v2(
            EngineId::Cursor,
            selection.profile_id().as_str(),
            Some(selection.permission()),
            config.runtime(),
            StoredDetails::Cursor(StoredCursorDetails {
                model_id: selection.model_id().map(EngineModelId::as_str),
                reasoning_effort: selection
                    .reasoning_effort()
                    .map(CursorReasoningEffort::as_str),
                speed: selection.speed().map(CursorSpeed::as_str),
                permission_mode: selection
                    .permission_mode()
                    .map(CursorPermissionMode::as_str),
            }),
        )?,
    };
    if encoded.len() > ENGINE_CONFIG_MAX_ENCODED_BYTES {
        return Err(EngineRunConfigCodecError::TooLarge);
    }
    Ok(encoded)
}

/// Decodes one stored configuration and rejects any noncanonical bytes.
///
/// Version 1 blobs keep the exact legacy `OpenCode` 2 shape. Version 2
/// blobs carry one tagged per-engine selection. Unknown engines and unknown
/// versions are rejected as typed field errors, never defaulted.
pub(crate) fn decode(bytes: &[u8]) -> Result<EngineRunConfig, EngineRunConfigCodecError> {
    if bytes.len() > ENGINE_CONFIG_MAX_ENCODED_BYTES {
        return Err(EngineRunConfigCodecError::TooLarge);
    }
    let peek: VersionPeek =
        serde_json::from_slice(bytes).map_err(|_| EngineRunConfigCodecError::Malformed)?;
    let config = match peek.version {
        1 => {
            let raw: RawConfig =
                serde_json::from_slice(bytes).map_err(|_| EngineRunConfigCodecError::Malformed)?;
            into_domain(raw)?
        }
        2 => {
            let raw: RawConfigV2 =
                serde_json::from_slice(bytes).map_err(|_| EngineRunConfigCodecError::Malformed)?;
            into_domain_v2(raw)?
        }
        _ => {
            return Err(EngineRunConfigCodecError::InvalidField { field: "version" });
        }
    };
    let canonical = encode(&config)?;
    if canonical.as_slice() != bytes {
        return Err(EngineRunConfigCodecError::NonCanonical);
    }
    Ok(config)
}
