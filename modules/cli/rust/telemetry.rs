use std::{fs, time::SystemTime};

use serde::{Deserialize, Serialize};

use crate::{
    CliError, Result,
    error::io,
    instance::{self, read_json, reject_unsafe_destination, write_private_json},
    paths::Layout,
};

const TELEMETRY_VERSION: u8 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Preference {
    Unset,
    Enabled,
    Disabled,
}

impl Preference {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unset => "unset",
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TelemetryPreferences {
    pub version: u8,
    pub installation_id: String,
    pub usage_analytics: Preference,
    pub crash_reports: Preference,
    pub updated_at: String,
}

pub fn path(layout: &Layout) -> std::path::PathBuf {
    layout.root.join("telemetry.json")
}

pub fn load_or_create(layout: &Layout) -> Result<TelemetryPreferences> {
    let _ = instance::paths(layout)?;
    let path = path(layout);
    reject_unsafe_destination(&path)?;
    if path.is_file() {
        let preferences: TelemetryPreferences = read_json(&path)?;
        if preferences.version != TELEMETRY_VERSION {
            return Err(CliError::Installation(format!(
                "unsupported telemetry preferences version {}",
                preferences.version
            )));
        }
        return Ok(preferences);
    }
    fs::create_dir_all(&layout.root).map_err(io("create Artisan home directory"))?;
    let preferences = TelemetryPreferences {
        version: TELEMETRY_VERSION,
        installation_id: random_installation_id()?,
        usage_analytics: Preference::Unset,
        crash_reports: Preference::Unset,
        updated_at: updated_at()?,
    };
    write_private_json(&path, &preferences)?;
    Ok(preferences)
}

pub fn set_usage_analytics(
    layout: &Layout,
    preference: Preference,
) -> Result<TelemetryPreferences> {
    let mut preferences = load_or_create(layout)?;
    preferences.usage_analytics = preference;
    preferences.updated_at = updated_at()?;
    write_private_json(&path(layout), &preferences)?;
    Ok(preferences)
}

pub fn set_crash_reports(layout: &Layout, preference: Preference) -> Result<TelemetryPreferences> {
    let mut preferences = load_or_create(layout)?;
    preferences.crash_reports = preference;
    preferences.updated_at = updated_at()?;
    write_private_json(&path(layout), &preferences)?;
    Ok(preferences)
}

pub fn reset_identity(layout: &Layout) -> Result<TelemetryPreferences> {
    let mut preferences = load_or_create(layout)?;
    preferences.installation_id = random_installation_id()?;
    preferences.updated_at = updated_at()?;
    write_private_json(&path(layout), &preferences)?;
    Ok(preferences)
}

fn random_installation_id() -> Result<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|error| CliError::Installation(format!("secure random source failed: {error}")))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    ))
}

fn updated_at() -> Result<String> {
    let seconds = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|error| {
            CliError::Installation(format!("system clock is before Unix epoch: {error}"))
        })?
        .as_secs();
    unix_seconds_to_timestamp(seconds)
}

fn unix_seconds_to_timestamp(seconds: u64) -> Result<String> {
    let days = i64::try_from(seconds / 86_400).map_err(|_| {
        CliError::Installation("system clock is outside the supported timestamp range".into())
    })?;
    let seconds_of_day = seconds % 86_400;
    let hour = seconds_of_day / 3_600;
    let minute = (seconds_of_day % 3_600) / 60;
    let second = seconds_of_day % 60;

    // Howard Hinnant's civil-from-days algorithm converts Unix epoch days to
    // the proleptic Gregorian calendar without an additional time dependency.
    let epoch_days = days + 719_468;
    let era = if epoch_days >= 0 {
        epoch_days
    } else {
        epoch_days - 146_096
    } / 146_097;
    let day_of_era = epoch_days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    Ok(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"
    ))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{
        Preference, load_or_create, reset_identity, set_crash_reports, set_usage_analytics,
    };
    use crate::paths::Layout;

    fn temporary_layout() -> (tempfile::TempDir, Layout) {
        let directory = tempfile::tempdir().expect("temporary Artisan home");
        let root = directory.path().join("home");
        let layout = Layout {
            manifest: root.join("installation.json"),
            root,
        };
        (directory, layout)
    }

    #[test]
    fn first_creation_writes_versioned_unset_preferences_with_a_random_identity() {
        let (_directory, layout) = temporary_layout();

        let preferences = load_or_create(&layout).expect("create telemetry preferences");

        assert_eq!(preferences.version, 1);
        assert_eq!(preferences.usage_analytics, Preference::Unset);
        assert_eq!(preferences.crash_reports, Preference::Unset);
        assert_eq!(preferences.installation_id.len(), 36);
        assert_eq!(preferences.installation_id.as_bytes()[8], b'-');
        assert_eq!(preferences.installation_id.as_bytes()[13], b'-');
        assert_eq!(preferences.installation_id.as_bytes()[18], b'-');
        assert_eq!(preferences.installation_id.as_bytes()[23], b'-');
        assert!(!preferences.updated_at.is_empty());
        let persisted = fs::read_to_string(layout.root.join("telemetry.json"))
            .expect("persisted telemetry preferences");
        assert!(persisted.contains("\"version\": 1"));
        assert!(persisted.contains("\"usage_analytics\": \"unset\""));
        assert!(persisted.contains("\"crash_reports\": \"unset\""));
    }

    #[test]
    fn legacy_profile_home_migrates_before_creating_root_telemetry_preferences() {
        let (_directory, layout) = temporary_layout();
        let legacy = layout.root.join("profiles").join("default");
        fs::create_dir_all(&legacy).expect("legacy profile");
        fs::write(legacy.join("config.json"), b"{\"version\":1}").expect("legacy config");
        fs::write(legacy.join("secrets.json"), b"{\"version\":1}").expect("legacy secrets");

        load_or_create(&layout).expect("migrate and create telemetry preferences");

        assert!(layout.root.join("config.json").is_file());
        assert!(layout.root.join("secrets.json").is_file());
        assert!(layout.root.join("telemetry.json").is_file());
        assert!(!layout.root.join("profiles").exists());
    }

    #[test]
    fn usage_analytics_updates_independently_with_an_atomic_replacement() {
        let (_directory, layout) = temporary_layout();
        let original = load_or_create(&layout).expect("initial preferences");

        let updated = set_usage_analytics(&layout, Preference::Enabled).expect("enable analytics");

        assert_eq!(updated.usage_analytics, Preference::Enabled);
        assert_eq!(updated.crash_reports, Preference::Unset);
        assert_eq!(updated.installation_id, original.installation_id);
        assert_eq!(fs::read_dir(&layout.root).expect("home entries").count(), 1);
        assert_eq!(
            load_or_create(&layout)
                .expect("persisted preferences")
                .usage_analytics,
            Preference::Enabled
        );
    }

    #[test]
    fn crash_reports_updates_independently() {
        let (_directory, layout) = temporary_layout();
        set_usage_analytics(&layout, Preference::Disabled).expect("disable analytics");

        let updated = set_crash_reports(&layout, Preference::Enabled).expect("enable crashes");

        assert_eq!(updated.usage_analytics, Preference::Disabled);
        assert_eq!(updated.crash_reports, Preference::Enabled);
    }

    #[test]
    fn identity_reset_generates_a_new_id_without_changing_consent() {
        let (_directory, layout) = temporary_layout();
        set_usage_analytics(&layout, Preference::Enabled).expect("enable analytics");
        let before =
            set_crash_reports(&layout, Preference::Disabled).expect("disable crash reports");

        let after = reset_identity(&layout).expect("reset identity");

        assert_ne!(after.installation_id, before.installation_id);
        assert_eq!(after.usage_analytics, Preference::Enabled);
        assert_eq!(after.crash_reports, Preference::Disabled);
        assert_eq!(after, load_or_create(&layout).expect("persisted reset"));
    }

    #[test]
    fn updated_at_is_an_iso_8601_utc_timestamp() {
        let (_directory, layout) = temporary_layout();

        let timestamp = load_or_create(&layout)
            .expect("telemetry preferences")
            .updated_at;

        assert_eq!(timestamp.len(), 20, "{timestamp}");
        assert_eq!(&timestamp[4..5], "-");
        assert_eq!(&timestamp[7..8], "-");
        assert_eq!(&timestamp[10..11], "T");
        assert_eq!(&timestamp[13..14], ":");
        assert_eq!(&timestamp[16..17], ":");
        assert_eq!(&timestamp[19..20], "Z");
        for range in [0..4, 5..7, 8..10, 11..13, 14..16, 17..19] {
            assert!(timestamp[range].bytes().all(|byte| byte.is_ascii_digit()));
        }
    }

    #[test]
    fn unsupported_telemetry_schema_version_is_rejected() {
        let (_directory, layout) = temporary_layout();
        fs::create_dir_all(&layout.root).expect("home");
        fs::write(
            layout.root.join("telemetry.json"),
            br#"{"version":2,"installation_id":"00000000-0000-4000-8000-000000000000","usage_analytics":"unset","crash_reports":"unset","updated_at":"2026-08-22T00:00:00Z"}"#,
        )
        .expect("future preferences");

        let error = load_or_create(&layout).expect_err("reject future schema");

        assert!(
            error
                .to_string()
                .contains("unsupported telemetry preferences version 2")
        );
    }

    #[test]
    fn unsafe_telemetry_destination_is_rejected() {
        let (_directory, layout) = temporary_layout();
        fs::create_dir_all(layout.root.join("telemetry.json")).expect("unsafe destination");

        let error = load_or_create(&layout).expect_err("reject non-file destination");

        assert!(
            matches!(error, crate::CliError::UnsafePath(path) if path == layout.root.join("telemetry.json"))
        );
    }
}
