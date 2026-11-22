use crate::MachineKeypair;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};

#[derive(Default, Serialize, Deserialize)]
struct MachineFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    secret_b64u: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    public_b64u: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    enrollment_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    machine_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account_id: Option<String>,
}

impl MachineFile {
    fn present_fields(&self) -> String {
        let fields: Vec<&str> = [
            (self.secret_b64u.is_some(), "secret_b64u"),
            (self.public_b64u.is_some(), "public_b64u"),
            (self.enrollment_id.is_some(), "enrollment_id"),
            (self.machine_id.is_some(), "machine_id"),
            (self.account_id.is_some(), "account_id"),
        ]
        .into_iter()
        .filter_map(|(present, name)| present.then_some(name))
        .collect();
        fields.join(", ")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MachineAccount {
    Unkeyed,
    Keyed {
        key: MachineKeypair,
    },
    Pending {
        key: MachineKeypair,
        enrollment_id: String,
    },
    Enrolled {
        key: MachineKeypair,
        machine_id: String,
        account_id: String,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum MachineFileError {
    #[error("the account file is not the JSON `ae account` writes")]
    Parse(#[source] serde_json::Error),
    #[error("the account file holds {fields}, which is not a state `ae account` writes")]
    Inconsistent { fields: String },
    #[error("the account file holds an unusable machine key")]
    Key(#[source] KeyProblem),
}

#[derive(Debug, thiserror::Error)]
pub enum KeyProblem {
    #[error("the secret is not base64url")]
    SecretEncoding(#[source] base64::DecodeError),
    #[error("the secret is {bytes} bytes, not 32")]
    SecretLength { bytes: usize },
    #[error("the public key is not base64url")]
    PublicEncoding(#[source] base64::DecodeError),
    #[error("the public key is {bytes} bytes, not 32")]
    PublicLength { bytes: usize },
    #[error("the secret is not a usable key")]
    Secret(#[source] crate::MachineKeyError),
    #[error("the public key does not match the secret")]
    Mismatch,
}

fn decode_key(secret: &str, public: &str) -> Result<MachineKeypair, KeyProblem> {
    let secret = URL_SAFE_NO_PAD
        .decode(secret)
        .map_err(KeyProblem::SecretEncoding)?;
    let secret: [u8; 32] =
        secret
            .as_slice()
            .try_into()
            .map_err(|_short| KeyProblem::SecretLength {
                bytes: secret.len(),
            })?;
    let public = URL_SAFE_NO_PAD
        .decode(public)
        .map_err(KeyProblem::PublicEncoding)?;
    let public: [u8; 32] =
        public
            .as_slice()
            .try_into()
            .map_err(|_short| KeyProblem::PublicLength {
                bytes: public.len(),
            })?;
    let key = MachineKeypair::from_secret(secret).map_err(KeyProblem::Secret)?;
    if key.public() != &public {
        return Err(KeyProblem::Mismatch);
    }
    Ok(key)
}

impl MachineAccount {
    pub fn decode(bytes: &[u8]) -> Result<Self, MachineFileError> {
        let file: MachineFile = serde_json::from_slice(bytes).map_err(MachineFileError::Parse)?;
        let inconsistent = MachineFileError::Inconsistent {
            fields: file.present_fields(),
        };
        let MachineFile {
            secret_b64u,
            public_b64u,
            enrollment_id,
            machine_id,
            account_id,
        } = file;
        let key = match (secret_b64u, public_b64u) {
            (None, None) => None,
            (Some(secret), Some(public)) => {
                Some(decode_key(&secret, &public).map_err(MachineFileError::Key)?)
            }
            (Some(_), None) | (None, Some(_)) => return Err(inconsistent),
        };
        match (key, enrollment_id, machine_id, account_id) {
            (None, None, None, None) => Ok(Self::Unkeyed),
            (Some(key), None, None, None) => Ok(Self::Keyed { key }),
            (Some(key), Some(enrollment_id), None, None) => {
                Ok(Self::Pending { key, enrollment_id })
            }
            (Some(key), None, Some(machine_id), Some(account_id)) => Ok(Self::Enrolled {
                key,
                machine_id,
                account_id,
            }),
            (_, _, _, _) => Err(inconsistent),
        }
    }

    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let keyed = |key: &MachineKeypair| MachineFile {
            secret_b64u: Some(URL_SAFE_NO_PAD.encode(key.secret())),
            public_b64u: Some(key.public_b64u()),
            ..MachineFile::default()
        };
        let file = match self {
            Self::Unkeyed => MachineFile::default(),
            Self::Keyed { key } => keyed(key),
            Self::Pending { key, enrollment_id } => MachineFile {
                enrollment_id: Some(enrollment_id.clone()),
                ..keyed(key)
            },
            Self::Enrolled {
                key,
                machine_id,
                account_id,
            } => MachineFile {
                machine_id: Some(machine_id.clone()),
                account_id: Some(account_id.clone()),
                ..keyed(key)
            },
        };
        serde_json::to_vec_pretty(&file).expect("invariant: an account file of strings serializes")
    }

    #[must_use]
    pub const fn key(&self) -> Option<&MachineKeypair> {
        match self {
            Self::Unkeyed => None,
            Self::Keyed { key } | Self::Pending { key, .. } | Self::Enrolled { key, .. } => {
                Some(key)
            }
        }
    }
}
