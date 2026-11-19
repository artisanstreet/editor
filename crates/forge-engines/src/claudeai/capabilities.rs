use artisan_api::{UsageCadence, UsageLimits, UsageWindow};
use serde_json::Value;

use super::transport::{RequestError, Transport};

#[derive(Debug, thiserror::Error)]
#[error("reading the claude.ai account's bootstrap")]
pub struct CapabilitiesError(#[source] pub RequestError);

#[derive(Debug, thiserror::Error)]
#[error("reading the usage of claude.ai organization {org}")]
pub struct UsageError {
    pub org: String,
    #[source]
    pub source: RequestError,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub org_uuid: String,
    pub account: Option<String>,
    pub rate_limit_tier: Option<String>,
    pub chat: bool,
    pub client_tools: bool,
}

pub async fn capabilities(transport: &Transport) -> Result<Capabilities, CapabilitiesError> {
    let org_uuid = transport.session().org_uuid.clone();
    let organization = match transport
        .get(&format!("/api/organizations/{org_uuid}"))
        .await
    {
        Ok(organization) => Some(organization),
        Err(error) => {
            tracing::info!(org = %org_uuid, "claude.ai's chat does not answer this session: {}", diagnostics::chain(&error));
            None
        }
    };
    let bootstrap = transport
        .get("/api/bootstrap")
        .await
        .map_err(CapabilitiesError)?;

    let account = account_of(&bootstrap);
    let rate_limit_tier = organization
        .as_ref()
        .and_then(|organization| organization["rate_limit_tier"].as_str())
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
        org_uuid,
        account,
        rate_limit_tier,
        chat: organization.is_some(),
        client_tools: true,
    })
}

fn account_of(bootstrap: &Value) -> Option<String> {
    let account = &bootstrap["account"];
    account["email_address"]
        .as_str()
        .or_else(|| account["full_name"].as_str())
        .map(str::to_owned)
}

pub async fn usage(transport: &Transport) -> Result<UsageLimits, UsageError> {
    let org_uuid = transport.session().org_uuid.clone();
    let usage = transport
        .get(&format!("/api/organizations/{org_uuid}/usage"))
        .await
        .map_err(|source| UsageError {
            org: org_uuid.clone(),
            source,
        })?;
    Ok(UsageLimits {
        windows: windows_of(&usage),
        spend: None,
        read_at: artisan_api::now(),
    })
}

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
#[path = "../../tests/claudeai/capabilities.rs"]
mod tests;
