//! What the signed-in claude.ai account can do, and what it has used.
//!
//! Both are first-party endpoints of the same host the transport talks to,
//! so they need nothing beyond the session.

use artisan_api::{UsageCadence, UsageLimits, UsageWindow};
use serde_json::Value;

use super::transport::Transport;

/// What the account is and what it may do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capabilities {
    /// The organization the session is in.
    pub org_uuid: String,
    /// The account's email, when claude.ai says it.
    pub account: Option<String>,
    /// The plan the account is on.
    pub rate_limit_tier: Option<String>,
    /// Whether claude.ai's chat endpoint answers this session.
    pub chat: bool,
    /// Whether the account may use a client tool.
    pub client_tools: bool,
}

/// Reads what the account is and may do.
///
/// # Errors
///
/// Returns the error when the session is not signed in.
pub async fn capabilities(transport: &Transport) -> anyhow::Result<Capabilities> {
    let org_uuid = transport.session().org_uuid.clone();
    // The organization's own record answers when the session is good; the
    // bootstrap answers what the account is.
    let organization = transport
        .get(&format!("/api/organizations/{org_uuid}"))
        .await
        .unwrap_or(Value::Null);
    let bootstrap = transport.get("/api/bootstrap").await.unwrap_or(Value::Null);

    let account = account_of(&bootstrap);
    let rate_limit_tier = organization["rate_limit_tier"]
        .as_str()
        .or_else(|| {
            bootstrap["account"]["memberships"]
                .as_array()
                .and_then(|memberships| {
                    memberships
                        .iter()
                        .find(|membership| {
                            membership["organization"]["uuid"].as_str() == Some(&org_uuid)
                        })
                        .or_else(|| memberships.first())
                })
                .and_then(|membership| membership["organization"]["rate_limit_tier"].as_str())
        })
        .map(str::to_owned);
    Ok(Capabilities {
        chat: !organization.is_null(),
        client_tools: true,
        org_uuid,
        account,
        rate_limit_tier,
    })
}

/// The account's email, if claude.ai said it.
fn account_of(bootstrap: &Value) -> Option<String> {
    let account = &bootstrap["account"];
    account["email_address"]
        .as_str()
        .or_else(|| account["full_name"].as_str())
        .map(str::to_owned)
}

/// Reads the account's usage windows.
///
/// # Errors
///
/// Returns the error when the session is not signed in.
pub async fn usage(transport: &Transport) -> anyhow::Result<UsageLimits> {
    let org_uuid = transport.session().org_uuid.clone();
    let usage = transport
        .get(&format!("/api/organizations/{org_uuid}/usage"))
        .await?;
    Ok(UsageLimits {
        windows: windows_of(&usage),
        spend: None,
        read_at: artisan_api::now(),
    })
}

/// The windows claude.ai reports, in Forge's shape.
fn windows_of(usage: &Value) -> Vec<UsageWindow> {
    let window = |name: &str, label: &str, cadence| {
        let entry = &usage[name];
        let percent = entry["utilization"].as_f64()?;
        let resets_at = entry["resets_at"]
            .as_str()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.timestamp_millis());
        Some(UsageWindow {
            label: label.to_owned(),
            cadence,
            scope: None,
            used_percent: percent.clamp(0.0, 100.0),
            resets_at,
        })
    };
    [
        window("five_hour", "Session", UsageCadence::Session),
        window("seven_day", "Week", UsageCadence::Week),
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_windows_are_read_in_forge_s_shape() {
        let usage = json!({
            "five_hour": {
                "utilization": 3.0,
                "resets_at": "2026-10-21T17:20:00.759594+00:00"
            },
            "seven_day": { "utilization": 12.5, "resets_at": null }
        });
        let windows = windows_of(&usage);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].label, "Session");
        assert_eq!(windows[0].cadence, UsageCadence::Session);
        assert!((windows[0].used_percent - 3.0).abs() < f64::EPSILON);
        assert!(windows[0].resets_at.is_some());
        assert_eq!(windows[1].label, "Week");
        assert_eq!(windows[1].cadence, UsageCadence::Week);
        assert!((windows[1].used_percent - 12.5).abs() < f64::EPSILON);
        assert!(windows[1].resets_at.is_none());
    }
}
