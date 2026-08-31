//! Canonical, bounded persistence codec for an engine run configuration.
//!
//! This module intentionally stays private to the database crate. The wire
//! protocol has its own conversion, while this codec is the only authority
//! for the bytes stored in SQLite and in immutable assistant-run snapshots.

use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use artisan_domain::{
    ApprovalMode, ByteLimit, CountLimit, ENGINE_CONFIG_MAX_ENCODED_BYTES, EngineAgentId,
    EngineConfigError, EngineId, EngineModelId, EnginePermissionPolicy, EngineProfileId,
    EngineRouteId, EngineRunConfig, EngineRuntimeControls, EngineSelection, EngineVariantId,
    FilesystemAccess, FiniteMillis, NetworkAccess, OpenCode2Selection, PermissionId,
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

    let permission_id = PermissionId::parse(raw.permission.permission_id).map_err(|_| {
        EngineRunConfigCodecError::InvalidField {
            field: "permission_id",
        }
    })?;
    let agent_id = EngineAgentId::parse(raw.permission.agent_id)
        .map_err(|_| EngineRunConfigCodecError::InvalidField { field: "agent_id" })?;
    let permission = EnginePermissionPolicy::new(
        permission_id,
        agent_id,
        parse_approval(&raw.permission.approval)?,
        parse_filesystem(&raw.permission.filesystem)?,
        parse_network(&raw.permission.network)?,
        parse_web_search(&raw.permission.web_search)?,
    );

    let runtime = EngineRuntimeControls::new(
        parse_millis(raw.runtime.attempt_budget_ms, "attempt_budget_ms")?,
        parse_millis(raw.runtime.readiness_budget_ms, "readiness_budget_ms")?,
        parse_millis(raw.runtime.health_budget_ms, "health_budget_ms")?,
        parse_millis(raw.runtime.prompt_budget_ms, "prompt_budget_ms")?,
        parse_millis(raw.runtime.stream_budget_ms, "stream_budget_ms")?,
        parse_millis(raw.runtime.close_budget_ms, "close_budget_ms")?,
        parse_bytes(raw.runtime.max_json_body_bytes, "max_json_body_bytes")?,
        parse_bytes(raw.runtime.max_sse_line_bytes, "max_sse_line_bytes")?,
        parse_bytes(raw.runtime.max_sse_event_bytes, "max_sse_event_bytes")?,
        parse_bytes(
            raw.runtime.max_readiness_line_bytes,
            "max_readiness_line_bytes",
        )?,
        parse_count(raw.runtime.max_header_count, "max_header_count")?,
        parse_bytes(raw.runtime.max_http_buffer_bytes, "max_http_buffer_bytes")?,
        parse_bytes(raw.runtime.max_stderr_bytes, "max_stderr_bytes")?,
        parse_count(raw.runtime.observation_capacity, "observation_capacity")?,
    )
    .map_err(domain_error)?;

    Ok(EngineRunConfig::new(
        EngineSelection::OpenCode2(OpenCode2Selection::new(
            profile_id, model_id, route_id, variant_id, permission,
        )),
        runtime,
    ))
}

/// Encodes one configuration into canonical bounded JSON bytes.
pub(crate) fn encode(config: &EngineRunConfig) -> Result<Vec<u8>, EngineRunConfigCodecError> {
    let selection = config.selection().as_opencode2();
    let permission = selection.permission();
    let runtime = config.runtime();
    let stored = StoredConfig {
        version: 1,
        engine: EngineId::OpenCode2.as_str(),
        profile_id: selection.profile_id().as_str(),
        model_id: selection.model_id().as_str(),
        route_id: selection.route_id().as_str(),
        variant_id: selection.variant_id().map(|value| value.as_str()),
        permission: StoredPermission {
            permission_id: permission.permission_id().as_str(),
            agent_id: permission.agent_id().as_str(),
            approval: permission.approval().as_str(),
            filesystem: permission.filesystem().as_str(),
            network: permission.network().as_str(),
            web_search: permission.web_search().as_str(),
        },
        runtime: StoredRuntime {
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
        },
    };
    let encoded = serde_json::to_vec(&stored).map_err(|_| EngineRunConfigCodecError::Encode)?;
    if encoded.len() > ENGINE_CONFIG_MAX_ENCODED_BYTES {
        return Err(EngineRunConfigCodecError::TooLarge);
    }
    Ok(encoded)
}

/// Decodes one stored configuration and rejects any noncanonical bytes.
pub(crate) fn decode(bytes: &[u8]) -> Result<EngineRunConfig, EngineRunConfigCodecError> {
    if bytes.len() > ENGINE_CONFIG_MAX_ENCODED_BYTES {
        return Err(EngineRunConfigCodecError::TooLarge);
    }
    let raw: RawConfig =
        serde_json::from_slice(bytes).map_err(|_| EngineRunConfigCodecError::Malformed)?;
    let config = into_domain(raw)?;
    let canonical = encode(&config)?;
    if canonical.as_slice() != bytes {
        return Err(EngineRunConfigCodecError::NonCanonical);
    }
    Ok(config)
}
