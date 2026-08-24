//! Forge host configuration.
//!
//! Mirrors `modules/forge/src/config.ts`: the host accepts only loopback
//! listeners, the WebSocket path is fixed to `/api/ws`, and every field is
//! validated before any native resource is opened. The native config surface
//! is TOML; field names and defaults match the TypeScript schema exactly.

use std::net::IpAddr;

use serde::Deserialize;

/// Which loopback interface the listener may bind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ListenHost {
    /// IPv4 loopback only.
    LoopbackV4,
    /// IPv6 loopback only.
    LoopbackV6,
}

impl ListenHost {
    /// The string form used in configuration and the state card.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LoopbackV4 => "127.0.0.1",
            Self::LoopbackV6 => "::1",
        }
    }

    /// The IP address to bind.
    #[must_use]
    pub fn address(self) -> IpAddr {
        match self {
            Self::LoopbackV4 => IpAddr::from([127, 0, 0, 1]),
            Self::LoopbackV6 => IpAddr::from([0, 0, 0, 0, 0, 0, 0, 1]),
        }
    }
}

/// Raw TOML surface; every field optional so defaults live in one place.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawForgeConfig {
    allowed_hostnames: Option<Vec<String>>,
    allowed_origins: Option<Vec<String>>,
    auth_token: Option<String>,
    database_path: Option<String>,
    instance_id: Option<String>,
    listen_host: Option<String>,
    listen_port: Option<u16>,
    migrations_path: Option<String>,
}

/// Validated host configuration; construction is the validation gate.
#[derive(Debug, Clone)]
pub struct ForgeConfig {
    allowed_hostnames: Vec<String>,
    allowed_origins: Vec<String>,
    auth_token: Option<String>,
    database_path: String,
    instance_id: String,
    listen_host: ListenHost,
    listen_port: u16,
    migrations_path: String,
    drain_timeout_seconds: u64,
    heartbeat_interval_ms: u64,
    heartbeat_timeout_ms: u64,
}

const INSTANCE_ID_MIN: usize = 1;
const INSTANCE_ID_MAX: usize = 128;
const AUTH_TOKEN_MIN: usize = 32;
const WEBSOCKET_PATH: &str = "/api/ws";

impl ForgeConfig {
    /// Decodes and validates raw TOML text before any resource is opened.
    ///
    /// # Errors
    /// Returns a descriptive message for malformed TOML, unknown fields,
    /// missing required paths, or any value outside its legacy bounds.
    pub fn decode_toml(text: &str) -> Result<Self, ConfigError> {
        let raw: RawForgeConfig = toml::from_str(text)
            .map_err(|error| ConfigError(format!("invalid forge config: {error}")))?;
        Self::try_from_raw(raw)
    }

    fn try_from_raw(raw: RawForgeConfig) -> Result<Self, ConfigError> {
        let database_path = raw
            .database_path
            .ok_or_else(|| ConfigError("database_path is required".to_string()))?;
        let migrations_path = raw
            .migrations_path
            .ok_or_else(|| ConfigError("migrations_path is required".to_string()))?;
        let instance_id = raw.instance_id.unwrap_or_default();
        if instance_id.len() < INSTANCE_ID_MIN || instance_id.len() > INSTANCE_ID_MAX {
            return Err(ConfigError(format!(
                "instance_id must be {INSTANCE_ID_MIN}..={INSTANCE_ID_MAX} characters"
            )));
        }
        let listen_host = match raw.listen_host.as_deref() {
            None | Some("127.0.0.1") => ListenHost::LoopbackV4,
            Some("::1") => ListenHost::LoopbackV6,
            Some(other) => {
                return Err(ConfigError(format!(
                    "listen_host must be 127.0.0.1 or ::1, got {other:?}"
                )));
            }
        };
        if let Some(token) = raw.auth_token.as_deref() {
            if token.len() < AUTH_TOKEN_MIN {
                return Err(ConfigError(format!(
                    "auth_token must be at least {AUTH_TOKEN_MIN} characters when present"
                )));
            }
        }
        Ok(Self {
            allowed_hostnames: raw.allowed_hostnames.unwrap_or_default(),
            allowed_origins: raw.allowed_origins.unwrap_or_default(),
            auth_token: raw.auth_token,
            database_path,
            instance_id,
            listen_host,
            listen_port: raw.listen_port.unwrap_or(0),
            migrations_path,
            drain_timeout_seconds: 30,
            heartbeat_interval_ms: 15_000,
            heartbeat_timeout_ms: 30_000,
        })
    }

    /// Exact reverse-proxy hostnames that may reach the listener.
    #[must_use]
    pub fn allowed_hostnames(&self) -> &[String] {
        &self.allowed_hostnames
    }

    /// Origins permitted on WebSocket upgrades.
    #[must_use]
    pub fn allowed_origins(&self) -> &[String] {
        &self.allowed_origins
    }

    /// Optional shared secret; present only when pairing has been configured.
    #[must_use]
    pub fn auth_token(&self) -> Option<&str> {
        self.auth_token.as_deref()
    }

    /// `SQLite` database path; the lease file derives from it.
    #[must_use]
    pub fn database_path(&self) -> &str {
        &self.database_path
    }

    /// Stable identity of this Forge instance.
    #[must_use]
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// Loopback address to bind.
    #[must_use]
    pub fn listen_host(&self) -> ListenHost {
        self.listen_host
    }

    /// TCP port; zero lets the OS choose (the development default).
    #[must_use]
    pub fn listen_port(&self) -> u16 {
        self.listen_port
    }

    /// Directory of Drizzle migration SQL replayed verbatim at startup.
    #[must_use]
    pub fn migrations_path(&self) -> &str {
        &self.migrations_path
    }

    /// Fixed control-lane WebSocket path.
    #[must_use]
    pub fn websocket_path(&self) -> &'static str {
        WEBSOCKET_PATH
    }

    /// Graceful drain budget before forced teardown.
    #[must_use]
    pub fn drain_timeout_seconds(&self) -> u64 {
        self.drain_timeout_seconds
    }

    /// Welcome-frame heartbeat interval in milliseconds.
    #[must_use]
    pub fn heartbeat_interval_ms(&self) -> u64 {
        self.heartbeat_interval_ms
    }

    /// Welcome-frame heartbeat timeout in milliseconds.
    #[must_use]
    pub fn heartbeat_timeout_ms(&self) -> u64 {
        self.heartbeat_timeout_ms
    }
}

/// Configuration failures carry a stable, user-presentable message.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ConfigError(String);

#[cfg(test)]
mod tests {
    use super::{ForgeConfig, ListenHost};

    const BASE: &str =
        "database_path = 'home/artisan.db'\nmigrations_path = 'drizzle'\ninstance_id = 'dev-1'\n";

    #[test]
    fn defaults_match_the_legacy_schema() {
        let config = ForgeConfig::decode_toml(BASE).expect("base config decodes");
        assert_eq!(config.listen_host(), ListenHost::LoopbackV4);
        assert_eq!(config.listen_port(), 0);
        assert!(config.allowed_hostnames().is_empty());
        assert!(config.allowed_origins().is_empty());
        assert!(config.auth_token().is_none());
        assert_eq!(config.websocket_path(), "/api/ws");
    }

    #[test]
    fn ipv6_loopback_is_accepted_and_remote_hosts_are_not() {
        let good = ForgeConfig::decode_toml(&format!("{BASE}listen_host = '::1'\n"));
        assert_eq!(
            good.expect("ipv6 decodes").listen_host(),
            ListenHost::LoopbackV6
        );
        let bad = ForgeConfig::decode_toml(&format!("{BASE}listen_host = '0.0.0.0'\n"));
        assert!(bad.is_err(), "non-loopback hosts must be rejected");
    }

    #[test]
    fn instance_id_bounds_are_enforced() {
        let long = "x".repeat(129);
        assert!(ForgeConfig::decode_toml(&format!("{BASE}instance_id = '{long}'\n")).is_err());
        assert!(ForgeConfig::decode_toml(&format!("{BASE}instance_id = ''\n")).is_err());
    }

    #[test]
    fn short_auth_tokens_are_rejected() {
        assert!(ForgeConfig::decode_toml(&format!("{BASE}auth_token = 'short'\n")).is_err());
        let long = format!("{BASE}auth_token = '{}'\n", "t".repeat(32));
        assert!(ForgeConfig::decode_toml(&long).is_ok());
    }

    #[test]
    fn unknown_fields_fail_closed() {
        let text = format!("{BASE}surprise_field = true\n");
        assert!(
            ForgeConfig::decode_toml(&text).is_err(),
            "toml crate rejects unknown fields by default"
        );
    }

    #[test]
    fn missing_required_paths_fail() {
        assert!(ForgeConfig::decode_toml("instance_id = 'x'\n").is_err());
    }
}
