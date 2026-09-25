//! The host account the Forge runs as, presented to its Editors.
//!
//! The Forge serves one account: the operating-system user that runs it on
//! its host. Its name and the host's name are what the Editor shows as the
//! profile, so the Editor never reads its own machine's environment for
//! them (it may be connected to another host).

#![forbid(unsafe_code)]

use artisan_domain::{AccountProfile, DisplayName};

const FALLBACK_USER: &str = "Artisan";
const FALLBACK_HOST: &str = "This host";

/// The account profile of the running Forge.
#[must_use]
pub(crate) fn host_account_profile() -> AccountProfile {
    let user = ["USER", "LOGNAME", "USERNAME"]
        .into_iter()
        .find_map(|name| std::env::var(name).ok());
    let host = std::fs::read_to_string("/etc/hostname")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .or_else(|| std::env::var("COMPUTERNAME").ok());
    account_profile(user, host)
}

/// Builds the profile from the observed names, falling back when one is
/// missing or unusable.
#[must_use]
pub(crate) fn account_profile(user: Option<String>, host: Option<String>) -> AccountProfile {
    let name = |value: Option<String>, fallback: &str| {
        value
            .map(|value| value.trim().to_owned())
            .and_then(|value| DisplayName::parse(value).ok())
            .unwrap_or_else(|| {
                DisplayName::parse(fallback).expect("the fallback name is a valid display name")
            })
    };
    AccountProfile {
        display_name: name(user, FALLBACK_USER),
        host_name: name(host, FALLBACK_HOST),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_trims_names_and_falls_back_when_missing_or_blank() {
        let profile = account_profile(Some("theo".into()), Some("ubuntu\n".into()));
        assert_eq!(profile.display_name.as_str(), "theo");
        assert_eq!(profile.host_name.as_str(), "ubuntu");
        let fallback = account_profile(None, Some("  ".into()));
        assert_eq!(fallback.display_name.as_str(), FALLBACK_USER);
        assert_eq!(fallback.host_name.as_str(), FALLBACK_HOST);
    }
}
