//! Product concepts that do not depend on transport, storage, or presentation.

use std::{fmt, str::FromStr};

use thiserror::Error;

/// Stable identity for an Artisan workspace.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WorkspaceId(String);

impl WorkspaceId {
    /// Creates an identifier after validating the external value.
    ///
    /// # Errors
    ///
    /// Returns [`WorkspaceIdError::Empty`] when the value has no non-whitespace
    /// characters.
    pub fn parse(value: impl Into<String>) -> Result<Self, WorkspaceIdError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(WorkspaceIdError::Empty);
        }

        Ok(Self(value))
    }

    /// Returns the validated identifier text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WorkspaceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for WorkspaceId {
    type Err = WorkspaceIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// Validation failure for [`WorkspaceId`].
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum WorkspaceIdError {
    /// The supplied value contained no non-whitespace characters.
    #[error("workspace identifier must not be empty")]
    Empty,
}
