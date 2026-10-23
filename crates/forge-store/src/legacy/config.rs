//! One explicit boundary for retired provider configuration and statuses.
use artisan_api::{EngineConfig, EngineId, Permissions, RunStatus, Status};
use serde_json::{Map, Value};

pub(super) fn json_object(bytes: &[u8]) -> Option<Map<String, Value>> {
    match serde_json::from_slice(bytes) {
        Ok(Value::Object(object)) => Some(object),
        _ => None,
    }
}

/// The old versioned engine config (`{engine, permission, details, …}`).
pub(super) fn engine_config(bytes: &[u8]) -> Option<EngineConfig> {
    let old = json_object(bytes)?;
    let engine: EngineId = old.get("engine")?.as_str()?.parse().ok()?;
    let text = |object: Option<&Value>, key: &str| {
        object
            .and_then(|object| object.get(key))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let permission = old.get("permission");
    let details = old.get("details");
    let mut config = EngineConfig::new(engine);
    config.model = text(details, "model_id");
    config.effort = text(details, "effort").or_else(|| text(details, "reasoning_effort"));
    let never_asks = text(permission, "approval").as_deref() == Some("never")
        || text(details, "permission_mode").as_deref() == Some("bypassPermissions");
    config.permissions = if never_asks {
        Permissions::Full
    } else {
        Permissions::Auto
    };
    match engine {
        EngineId::Codex => {
            config.speed = text(details, "speed").or_else(|| text(details, "service_tier"))
        }
        EngineId::OpenCode => {
            let retired_effort = config.effort.take();
            config.variant = text(details, "variant").or(retired_effort);
        }
        _ => {}
    }
    let context_key = match engine {
        EngineId::Codex => Some("model_context_window"),
        EngineId::Claude => Some("auto_compact_window"),
        _ => None,
    };
    if let Some(tokens) = context_key
        .and_then(|key| details?.get(key)?.as_u64())
        .filter(|tokens| (1..=100_000_000).contains(tokens))
    {
        config
            .options
            .insert("context_window".to_owned(), Value::from(tokens));
    }
    Some(config)
}

pub(super) fn item_status(lifecycle: &str) -> Status {
    match lifecycle {
        // Streaming rows of finished runs never got their final lifecycle;
        // their text is what the user saw.
        "completed" | "streaming" => Status::Done,
        "failed" => Status::Failed,
        _ => Status::Cancelled,
    }
}

pub(super) fn run_status(lifecycle: &str) -> RunStatus {
    match lifecycle {
        "completed" => RunStatus::Done,
        "failed" => RunStatus::Failed,
        "cancelled" => RunStatus::Cancelled,
        // Anything still live belonged to the old process.
        _ => RunStatus::Interrupted,
    }
}

#[cfg(test)]
mod config_tests {
    use super::*;
    #[test]
    fn retired_context_choices_keep_their_provider_override() {
        let codex =
            engine_config(br#"{"engine":"codex","details":{"model_context_window":1000000}}"#)
                .unwrap();
        let claude =
            engine_config(br#"{"engine":"claude","details":{"auto_compact_window":400000}}"#)
                .unwrap();
        assert_eq!(codex.context_window_override(), Some(1_000_000));
        assert_eq!(claude.context_window_override(), Some(400_000));
        let invalid =
            engine_config(br#"{"engine":"codex","details":{"model_context_window":0}}"#).unwrap();
        assert!(invalid.options.is_empty());
        assert!(engine_config(br#"{"engine":"claude"}"#).is_some());
    }

    #[test]
    fn retired_provider_options_are_imported_into_canonical_fields() {
        let codex =
            engine_config(br#"{"engine":"codex","details":{"service_tier":"priority"}}"#).unwrap();
        assert_eq!(codex.speed.as_deref(), Some("priority"));
        assert!(codex.options.is_empty());
        let opencode =
            engine_config(br#"{"engine":"opencode","details":{"reasoning_effort":"high"}}"#)
                .unwrap();
        assert_eq!(opencode.variant.as_deref(), Some("high"));
        assert!(opencode.effort.is_none());
    }
}
