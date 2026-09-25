//! The manual (`OpenCode` 2-shaped) engine configuration document.
//!
//! The Editor's manual settings form keeps every field as the text the user
//! typed or pasted (one `key=value` line per field); the Forge builds and
//! validates the typed [`EngineRunConfig`] from it
//! ([`ResolveEngineConfiguration`]) or refuses with a reason. No value is
//! ever synthesized: every field starts empty.

use crate::catalog_selection::SubmissionRefusal;
use crate::engine_config::{
    EngineConfigError, EngineConfigReason, EngineRunConfig, EngineSelection,
};
use crate::identifiers::ThreadId;

/// Stable field order for the explicit configuration document.
pub const MANUAL_CONFIGURATION_KEYS: [&str; 24] = [
    "profile_id",
    "model_id",
    "route_id",
    "variant_id",
    "permission_id",
    "agent_id",
    "approval",
    "filesystem",
    "network",
    "web_search",
    "attempt_budget",
    "readiness_budget",
    "health_budget",
    "prompt_budget",
    "stream_budget",
    "close_budget",
    "max_json_body_bytes",
    "max_sse_line_bytes",
    "max_sse_event_bytes",
    "max_readiness_line_bytes",
    "max_header_count",
    "max_http_buffer_bytes",
    "max_stderr_bytes",
    "observation_capacity",
];

/// Finite byte bound for one configuration document.
pub const MAX_MANUAL_CONFIGURATION_BYTES: usize = 16 * 1024;

/// Finite line bound for one configuration document.
pub const MAX_MANUAL_CONFIGURATION_LINES: usize = MANUAL_CONFIGURATION_KEYS.len();

/// Raw text of every field of one manual configuration.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct ManualEngineConfiguration {
    pub profile_id: String,
    pub model_id: String,
    pub route_id: String,
    /// Empty means no variant.
    pub variant_id: String,
    pub permission_id: String,
    pub agent_id: String,
    /// Valid spellings: `never`, `on_request`, `always`.
    pub approval: String,
    /// Valid spellings: `none`, `workspace`, `host`.
    pub filesystem: String,
    /// Valid spellings: `disabled`, `enabled`.
    pub network: String,
    /// Valid spellings: `disabled`, `enabled`.
    pub web_search: String,
    pub attempt_budget: String,
    pub readiness_budget: String,
    pub health_budget: String,
    pub prompt_budget: String,
    pub stream_budget: String,
    pub close_budget: String,
    pub max_json_body_bytes: String,
    pub max_sse_line_bytes: String,
    pub max_sse_event_bytes: String,
    pub max_readiness_line_bytes: String,
    pub max_header_count: String,
    pub max_http_buffer_bytes: String,
    pub max_stderr_bytes: String,
    pub observation_capacity: String,
}

fn document_error(field: &'static str, reason: EngineConfigReason) -> EngineConfigError {
    EngineConfigError::new(field, reason)
}

impl ManualEngineConfiguration {
    /// The stable empty-value document: one exact `key=` line per field.
    #[must_use]
    pub fn template() -> String {
        Self::default().to_document()
    }

    /// Renders every field as one `key=value` line in the stable order.
    #[must_use]
    pub fn to_document(&self) -> String {
        let mut document = String::new();
        for (key, value) in MANUAL_CONFIGURATION_KEYS.iter().zip(self.values()) {
            document.push_str(key);
            document.push('=');
            document.push_str(value);
            document.push('\n');
        }
        document
    }

    fn values(&self) -> [&str; 24] {
        [
            &self.profile_id,
            &self.model_id,
            &self.route_id,
            &self.variant_id,
            &self.permission_id,
            &self.agent_id,
            &self.approval,
            &self.filesystem,
            &self.network,
            &self.web_search,
            &self.attempt_budget,
            &self.readiness_budget,
            &self.health_budget,
            &self.prompt_budget,
            &self.stream_budget,
            &self.close_budget,
            &self.max_json_body_bytes,
            &self.max_sse_line_bytes,
            &self.max_sse_event_bytes,
            &self.max_readiness_line_bytes,
            &self.max_header_count,
            &self.max_http_buffer_bytes,
            &self.max_stderr_bytes,
            &self.observation_capacity,
        ]
    }

    fn value_mut(&mut self, index: usize) -> &mut String {
        match index {
            0 => &mut self.profile_id,
            1 => &mut self.model_id,
            2 => &mut self.route_id,
            3 => &mut self.variant_id,
            4 => &mut self.permission_id,
            5 => &mut self.agent_id,
            6 => &mut self.approval,
            7 => &mut self.filesystem,
            8 => &mut self.network,
            9 => &mut self.web_search,
            10 => &mut self.attempt_budget,
            11 => &mut self.readiness_budget,
            12 => &mut self.health_budget,
            13 => &mut self.prompt_budget,
            14 => &mut self.stream_budget,
            15 => &mut self.close_budget,
            16 => &mut self.max_json_body_bytes,
            17 => &mut self.max_sse_line_bytes,
            18 => &mut self.max_sse_event_bytes,
            19 => &mut self.max_readiness_line_bytes,
            20 => &mut self.max_header_count,
            21 => &mut self.max_http_buffer_bytes,
            22 => &mut self.max_stderr_bytes,
            _ => &mut self.observation_capacity,
        }
    }

    /// Parses one complete document without retaining the source text in
    /// either success or failure.
    ///
    /// # Errors
    ///
    /// Returns a bounded [`EngineConfigError`] for an oversized document, a
    /// malformed/unknown/duplicate line, or a missing field. The error
    /// carries only a stable field label and a finite reason category.
    pub fn parse(document: &str) -> Result<Self, EngineConfigError> {
        if document.len() > MAX_MANUAL_CONFIGURATION_BYTES {
            return Err(document_error("document", EngineConfigReason::OutOfRange));
        }
        let mut parsed = Self::default();
        let mut seen = [false; MANUAL_CONFIGURATION_KEYS.len()];
        for (line_count, line) in document.lines().enumerate() {
            if line_count >= MAX_MANUAL_CONFIGURATION_LINES {
                return Err(document_error("document", EngineConfigReason::OutOfRange));
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(document_error(
                    "configuration",
                    EngineConfigReason::InvalidIdentifier,
                ));
            };
            let Some(index) = MANUAL_CONFIGURATION_KEYS
                .iter()
                .position(|known| *known == key)
            else {
                return Err(document_error(
                    "configuration",
                    EngineConfigReason::Unsupported,
                ));
            };
            if seen[index] {
                return Err(document_error(
                    MANUAL_CONFIGURATION_KEYS[index],
                    EngineConfigReason::Inconsistent,
                ));
            }
            if value.contains('=') {
                return Err(document_error(
                    MANUAL_CONFIGURATION_KEYS[index],
                    EngineConfigReason::InvalidIdentifier,
                ));
            }
            seen[index] = true;
            value.clone_into(parsed.value_mut(index));
        }
        if let Some(index) = seen.iter().position(|present| !present) {
            return Err(document_error(
                MANUAL_CONFIGURATION_KEYS[index],
                EngineConfigReason::InvalidIdentifier,
            ));
        }
        Ok(parsed)
    }

    /// The fields of a saved configuration. The document stays `OpenCode`
    /// 2-shaped; any other engine contributes its profile, model, and
    /// permission policy.
    #[must_use]
    pub fn from_config(config: &EngineRunConfig) -> Self {
        let selection = config.selection();
        let runtime = config.runtime();
        let (model_id, route_id, variant_id) = match selection {
            EngineSelection::OpenCode2(selection) => (
                selection.model_id().as_str().to_owned(),
                selection.route_id().as_str().to_owned(),
                selection
                    .variant_id()
                    .map_or_else(String::new, |id| id.as_str().to_owned()),
            ),
            other => (
                other
                    .model_id()
                    .map_or_else(String::new, |id| id.as_str().to_owned()),
                String::new(),
                String::new(),
            ),
        };
        let permission = selection.permission();
        Self {
            profile_id: selection.profile_id().as_str().to_owned(),
            model_id,
            route_id,
            variant_id,
            permission_id: permission.permission_id().as_str().to_owned(),
            agent_id: permission.agent_id().as_str().to_owned(),
            approval: permission.approval().as_str().to_owned(),
            filesystem: permission.filesystem().as_str().to_owned(),
            network: permission.network().as_str().to_owned(),
            web_search: permission.web_search().as_str().to_owned(),
            attempt_budget: runtime.attempt_budget().get().to_string(),
            readiness_budget: runtime.readiness_budget().get().to_string(),
            health_budget: runtime.health_budget().get().to_string(),
            prompt_budget: runtime.prompt_budget().get().to_string(),
            stream_budget: runtime.stream_budget().get().to_string(),
            close_budget: runtime.close_budget().get().to_string(),
            max_json_body_bytes: runtime.max_json_body_bytes().get().to_string(),
            max_sse_line_bytes: runtime.max_sse_line_bytes().get().to_string(),
            max_sse_event_bytes: runtime.max_sse_event_bytes().get().to_string(),
            max_readiness_line_bytes: runtime.max_readiness_line_bytes().get().to_string(),
            max_header_count: runtime.max_header_count().get().to_string(),
            max_http_buffer_bytes: runtime.max_http_buffer_bytes().get().to_string(),
            max_stderr_bytes: runtime.max_stderr_bytes().get().to_string(),
            observation_capacity: runtime.observation_capacity().get().to_string(),
        }
    }
}

/// Asks the Forge to build the configuration a manual document describes
/// for a thread, without saving it.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ResolveEngineConfiguration {
    /// Thread the configuration is for.
    pub thread_id: ThreadId,
    /// The document's fields.
    pub configuration: ManualEngineConfiguration,
}

/// The Forge's answer to [`ResolveEngineConfiguration`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EngineConfigurationResolution {
    /// Thread the configuration is for.
    pub thread_id: ThreadId,
    /// The document as asked, so a late answer can be matched.
    pub configuration: ManualEngineConfiguration,
    /// The built configuration, or why the Forge refused it.
    pub outcome: Result<EngineRunConfig, SubmissionRefusal>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documents_round_trip_and_reject_malformed_lines() {
        let template = ManualEngineConfiguration::template();
        assert_eq!(template.lines().count(), MANUAL_CONFIGURATION_KEYS.len());
        assert_eq!(
            ManualEngineConfiguration::parse(&template),
            Ok(ManualEngineConfiguration::default())
        );
        let filled = ManualEngineConfiguration {
            profile_id: "profile".into(),
            model_id: "model".into(),
            approval: "on_request".into(),
            ..ManualEngineConfiguration::default()
        };
        assert_eq!(
            ManualEngineConfiguration::parse(&filled.to_document()),
            Ok(filled)
        );
        assert!(ManualEngineConfiguration::parse("profile_id=\nprofile_id=\n").is_err());
        assert!(ManualEngineConfiguration::parse("profile_id=has=extra\n").is_err());
        assert!(ManualEngineConfiguration::parse("unknown=x\n").is_err());
        assert!(
            ManualEngineConfiguration::parse(&"x".repeat(MAX_MANUAL_CONFIGURATION_BYTES + 1))
                .is_err()
        );
    }
}
