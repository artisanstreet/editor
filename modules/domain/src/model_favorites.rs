//! Domain values for the globally scoped model-favorites preference.
//!
//! Favorites contain stable catalog model ids only. Provider routes, display
//! labels, runtime capability fields, and catalog admission are deliberately
//! outside this module. The backend owns admission against its current
//! catalog before calling the database repository.

use std::collections::HashSet;
use std::fmt;
use std::str::FromStr;

use thiserror::Error;

use crate::bounds::IDENTIFIER_MAX_BYTES;

/// Maximum number of stable catalog ids in the global favorites snapshot.
pub const MODEL_FAVORITES_MAX_MODELS: usize = 1_024;

/// Maximum canonical JSON size retained in one idempotency receipt.
///
/// The bound is intentionally much larger than the worst-case encoding of a
/// 1,024-entry snapshot at the 128-byte identifier limit, while still making
/// receipt growth finite at the storage boundary.
pub const MODEL_FAVORITES_MAX_SNAPSHOT_BYTES: usize = 262_144;

/// A stable model id from the catalog.
///
/// This type is distinct from provider, route, variant, and display-name
/// values so a favorites record cannot accidentally persist a presentation or
/// runtime field in place of the catalog identity.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ModelFavoriteId(String);

impl ModelFavoriteId {
    /// Parses a stable catalog model id.
    ///
    /// The validation rule matches the native wire identifier contract:
    /// non-empty, no Unicode whitespace or control characters, and at most
    /// [`IDENTIFIER_MAX_BYTES`] UTF-8 bytes.
    ///
    /// # Errors
    ///
    /// Returns [`ModelFavoriteIdError`] when the value violates one of those
    /// constraints.
    pub fn parse(value: impl Into<String>) -> Result<Self, ModelFavoriteIdError> {
        let value = value.into();
        validate_model_favorite_id(&value)?;
        Ok(Self(value))
    }

    /// Returns the validated catalog model id.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModelFavoriteId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ModelFavoriteId {
    type Err = ModelFavoriteIdError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

/// Validation failure for a stable catalog model id.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ModelFavoriteIdError {
    /// The supplied id contained no characters.
    #[error("model favorite id must not be empty")]
    Empty,
    /// The supplied id contained whitespace or a control character.
    #[error(
        "model favorite id must not contain whitespace or control characters; found {character:?}"
    )]
    ForbiddenCharacter {
        /// The offending Unicode scalar value.
        character: char,
    },
    /// The supplied id exceeded the native identifier byte bound.
    #[error("model favorite id is {length} UTF-8 bytes; the maximum is {maximum}")]
    TooLong {
        /// The offending UTF-8 byte length.
        length: usize,
        /// The maximum accepted UTF-8 byte length.
        maximum: usize,
    },
}

fn validate_model_favorite_id(value: &str) -> Result<(), ModelFavoriteIdError> {
    if value.is_empty() {
        return Err(ModelFavoriteIdError::Empty);
    }

    if let Some(character) = value
        .chars()
        .find(|character| character.is_whitespace() || character.is_control())
    {
        return Err(ModelFavoriteIdError::ForbiddenCharacter { character });
    }

    let length = value.len();
    if length > IDENTIFIER_MAX_BYTES {
        return Err(ModelFavoriteIdError::TooLong {
            length,
            maximum: IDENTIFIER_MAX_BYTES,
        });
    }

    Ok(())
}

/// Monotonically increasing revision of the global favorites snapshot.
///
/// Revision zero is the empty initial state. Revisions are bounded to the
/// signed SQLite integer range so every domain value is representable by the
/// persistence schema.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ModelFavoritesRevision(u64);

impl ModelFavoritesRevision {
    const MAX_VALUE: u64 = i64::MAX as u64;

    /// Creates a revision from its persisted unsigned representation.
    ///
    /// # Errors
    ///
    /// Returns [`ModelFavoritesRevisionError::OutOfRange`] when the value
    /// cannot be represented by SQLite's signed integer range.
    pub const fn new(value: u64) -> Result<Self, ModelFavoritesRevisionError> {
        if value > Self::MAX_VALUE {
            return Err(ModelFavoritesRevisionError::OutOfRange {
                value,
                maximum: Self::MAX_VALUE,
            });
        }
        Ok(Self(value))
    }

    /// Returns the revision as an unsigned domain value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Returns the revision in the signed representation used by SQLite.
    #[must_use]
    pub const fn as_i64(self) -> i64 {
        self.0.cast_signed()
    }

    /// Advances the revision by one.
    ///
    /// # Errors
    ///
    /// Returns [`ModelFavoritesRevisionError::Overflow`] at the last
    /// representable revision.
    pub const fn checked_next(self) -> Result<Self, ModelFavoritesRevisionError> {
        if self.0 >= Self::MAX_VALUE {
            return Err(ModelFavoritesRevisionError::Overflow { value: self.0 });
        }
        Ok(Self(self.0 + 1))
    }
}

impl fmt::Display for ModelFavoritesRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Failure while constructing or advancing a favorites revision.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ModelFavoritesRevisionError {
    /// A raw persisted value is outside SQLite's signed integer range.
    #[error("model favorites revision {value} exceeds the maximum {maximum}")]
    OutOfRange {
        /// The rejected raw value.
        value: u64,
        /// The largest representable revision.
        maximum: u64,
    },
    /// The revision cannot advance without leaving its storage range.
    #[error("model favorites revision {value} cannot advance")]
    Overflow {
        /// The final representable revision.
        value: u64,
    },
}

/// An ordered, bounded snapshot of globally favorited model ids.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelFavoritesSnapshot {
    revision: ModelFavoritesRevision,
    model_ids: Vec<ModelFavoriteId>,
}

impl ModelFavoritesSnapshot {
    /// Builds a snapshot after checking its cardinality and uniqueness.
    ///
    /// The caller supplies the order. The database repository supplies the
    /// canonical `favorited_at_ms, model_id` order when it reads durable rows.
    ///
    /// # Errors
    ///
    /// Returns [`ModelFavoritesSnapshotError`] when the snapshot is too large
    /// or contains the same stable model id more than once.
    pub fn new(
        revision: ModelFavoritesRevision,
        model_ids: Vec<ModelFavoriteId>,
    ) -> Result<Self, ModelFavoritesSnapshotError> {
        if model_ids.len() > MODEL_FAVORITES_MAX_MODELS {
            return Err(ModelFavoritesSnapshotError::TooManyModels {
                count: model_ids.len(),
                maximum: MODEL_FAVORITES_MAX_MODELS,
            });
        }

        let mut seen = HashSet::with_capacity(model_ids.len());
        for model_id in &model_ids {
            if !seen.insert(model_id) {
                return Err(ModelFavoritesSnapshotError::DuplicateModelId {
                    model_id: model_id.clone(),
                });
            }
        }

        Ok(Self {
            revision,
            model_ids,
        })
    }

    /// Returns the empty revision-zero snapshot.
    #[must_use]
    pub fn empty() -> Self {
        Self {
            revision: ModelFavoritesRevision::default(),
            model_ids: Vec::new(),
        }
    }

    /// Returns the snapshot revision.
    #[must_use]
    pub const fn revision(&self) -> ModelFavoritesRevision {
        self.revision
    }

    /// Returns model ids in their persisted favorite order.
    #[must_use]
    pub fn model_ids(&self) -> &[ModelFavoriteId] {
        &self.model_ids
    }

    /// Reports whether the snapshot contains the supplied stable model id.
    #[must_use]
    pub fn contains(&self, model_id: &ModelFavoriteId) -> bool {
        self.model_ids.iter().any(|candidate| candidate == model_id)
    }
}

/// Failure while constructing a model favorites snapshot.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum ModelFavoritesSnapshotError {
    /// The snapshot exceeded the global cardinality ceiling.
    #[error("model favorites snapshot contains {count} models; the maximum is {maximum}")]
    TooManyModels {
        /// The rejected number of model ids.
        count: usize,
        /// The maximum accepted number of model ids.
        maximum: usize,
    },
    /// The snapshot repeated one stable model id.
    #[error("model favorites snapshot repeats model id `{model_id}`")]
    DuplicateModelId {
        /// The repeated catalog model id.
        model_id: ModelFavoriteId,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_favorite_ids_use_the_native_identifier_bound() {
        assert_eq!(
            ModelFavoriteId::parse("").unwrap_err(),
            ModelFavoriteIdError::Empty
        );
        assert_eq!(
            ModelFavoriteId::parse("model id").unwrap_err(),
            ModelFavoriteIdError::ForbiddenCharacter { character: ' ' }
        );
        let too_long = "x".repeat(IDENTIFIER_MAX_BYTES + 1);
        assert_eq!(
            ModelFavoriteId::parse(too_long).unwrap_err(),
            ModelFavoriteIdError::TooLong {
                length: IDENTIFIER_MAX_BYTES + 1,
                maximum: IDENTIFIER_MAX_BYTES,
            }
        );
    }

    #[test]
    fn revisions_start_at_zero_and_stop_at_sqlite_maximum() {
        let initial = ModelFavoritesRevision::default();
        assert_eq!(initial.get(), 0);
        assert_eq!(initial.checked_next().unwrap().get(), 1);
        let final_revision = ModelFavoritesRevision::new(i64::MAX as u64).unwrap();
        assert_eq!(
            final_revision.checked_next().unwrap_err(),
            ModelFavoritesRevisionError::Overflow {
                value: i64::MAX as u64
            }
        );
    }

    #[test]
    fn snapshots_reject_duplicates_and_cardinality_overflow() {
        let duplicate = ModelFavoriteId::parse("model-a").unwrap();
        assert!(matches!(
            ModelFavoritesSnapshot::new(
                ModelFavoritesRevision::default(),
                vec![duplicate.clone(), duplicate]
            ),
            Err(ModelFavoritesSnapshotError::DuplicateModelId { .. })
        ));

        let ids = (0..=MODEL_FAVORITES_MAX_MODELS)
            .map(|index| ModelFavoriteId::parse(format!("model-{index}")).unwrap())
            .collect();
        assert!(matches!(
            ModelFavoritesSnapshot::new(ModelFavoritesRevision::default(), ids),
            Err(ModelFavoritesSnapshotError::TooManyModels { .. })
        ));
    }
}
