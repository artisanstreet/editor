use serde::{Deserialize, Serialize};
use std::fmt;

macro_rules! id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            /// A fresh, time-ordered id.
            #[must_use]
            pub fn new() -> Self {
                Self(uuid::Uuid::now_v7().to_string())
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }
    };
}

id!(
    /// A conversation.
    ThreadId
);
id!(
    /// Anything in a thread. Clients create the ids of the messages they
    /// send, so a retried send is the same item.
    ItemId
);
id!(
    /// One engine turn (or several, when steers continue it).
    RunId
);
id!(
    /// A working directory the user opened.
    ProjectId
);

impl ProjectId {
    /// The built-in "No project": each of its threads runs in a fresh
    /// folder of its own under Forge's data folder.
    #[must_use]
    pub fn scratch() -> Self {
        Self("scratch".to_owned())
    }

    #[must_use]
    pub fn is_scratch(&self) -> bool {
        self.0 == "scratch"
    }
}

id!(
    /// A paired Editor device.
    DeviceId
);

/// Forge's global change counter. Every stored change gets the next value.
pub type Seq = u64;

/// SHA-256 of a blob's bytes, lowercase hex. Blobs are content addressed.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BlobHash(pub String);

impl BlobHash {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether this looks like a SHA-256 hex digest (safe to use in a path).
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.0.len() == 64
            && self
                .0
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    }
}

impl fmt::Display for BlobHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
