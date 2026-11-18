use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use artisan_api::ClaudeAiSession;
use artisan_paths::private::PrivateFileError;

const FILE: &str = "artisan-hosts.json";
pub(crate) const ORGANIZATION_COOKIE: &str = "lastActiveOrg";

#[derive(Debug, thiserror::Error)]
pub enum AuthorizationError {
    #[error("this device has no data folder for the claude.ai browser profile")]
    NoDataFolder(#[source] artisan_paths::RootError),
    #[error("reading which hosts this device signed in to claude.ai from {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("decoding which hosts this device signed in to claude.ai from {path}")]
    Decode {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("recording that this device signed {host} in to claude.ai")]
    Write {
        host: String,
        #[source]
        source: PrivateFileError,
    },
    #[error("the claude.ai session names no organization, so {host} cannot be bound to it")]
    NoOrganization { host: String },
}

#[derive(Debug, Default)]
pub(crate) struct Authorizations(BTreeMap<String, String>);

impl Authorizations {
    pub(crate) fn read(profile: &Path) -> Result<Self, AuthorizationError> {
        let path = profile.join(FILE);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(source) => return Err(AuthorizationError::Read { path, source }),
        };
        serde_json::from_slice(&bytes)
            .map(Self)
            .map_err(|source| AuthorizationError::Decode { path, source })
    }

    pub(crate) fn organization(&self, host: &str) -> Option<&str> {
        self.0.get(host).map(String::as_str)
    }

    fn write(&self, profile: &Path, host: &str) -> Result<(), AuthorizationError> {
        let bytes = serde_json::to_vec_pretty(&self.0)
            .expect("invariant: a map of strings always encodes as JSON");
        artisan_paths::private::write_file(&profile.join(FILE), &bytes).map_err(|source| {
            AuthorizationError::Write {
                host: host.to_owned(),
                source,
            }
        })
    }
}

pub(crate) fn authorize_in(
    profile: &Path,
    host: &str,
    session: &ClaudeAiSession,
) -> Result<(), AuthorizationError> {
    let organization = session
        .cookie(ORGANIZATION_COOKIE)
        .filter(|organization| !organization.is_empty())
        .ok_or_else(|| AuthorizationError::NoOrganization {
            host: host.to_owned(),
        })?;
    let mut authorizations = Authorizations::read(profile)?;
    authorizations
        .0
        .insert(host.to_owned(), organization.to_owned());
    authorizations.write(profile, host)
}

pub(crate) fn same_account(organization: &str, session: &ClaudeAiSession) -> bool {
    session.cookie(ORGANIZATION_COOKIE) == Some(organization)
}
