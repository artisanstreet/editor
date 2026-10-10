//! Images an engine showed during a run.
//!
//! An image observation carries a file path and display metadata only. The
//! pixels never travel through the observation stream: the engine host writes
//! inline image payloads to disk first and records where they landed.

use super::{
    OBSERVATION_DESCRIPTION_MAX_BYTES, OBSERVATION_PATH_MAX_BYTES, ObservationError, ObservationId,
    ObservationSequence, check_nonempty_text, check_optional_text,
};

/// How an image entered the run.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ImageOrigin {
    /// The engine generated the image.
    Generated,
    /// The engine looked at an existing image file.
    Viewed,
    /// A tool returned the image as part of its result.
    ToolResult,
}

impl ImageOrigin {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Generated => "generated",
            Self::Viewed => "viewed",
            Self::ToolResult => "tool_result",
        }
    }

    /// Parses a persisted image origin.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "generated" => Ok(Self::Generated),
            "viewed" => Ok(Self::Viewed),
            "tool_result" => Ok(Self::ToolResult),
            _ => Err(ObservationError::UnknownValue { field: "origin" }),
        }
    }
}

/// One image an engine generated, viewed, or received from a tool.
///
/// `path` is absolute on the machine that hosts the engine. The caption is a
/// short provider-disclosed description, such as a revised generation prompt
/// or the tool that produced the image.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ImageObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    pub(super) item_id: ObservationId,
    pub(super) origin: ImageOrigin,
    pub(super) path: String,
    pub(super) caption: Option<String>,
}

impl ImageObservation {
    /// Creates an image observation after validating identities and bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when the path is empty or exceeds
    /// [`OBSERVATION_PATH_MAX_BYTES`] bytes, or a present caption is empty or
    /// exceeds [`OBSERVATION_DESCRIPTION_MAX_BYTES`] bytes.
    pub fn new(
        id: ObservationId,
        sequence: ObservationSequence,
        item_id: ObservationId,
        origin: ImageOrigin,
        path: String,
        caption: Option<String>,
    ) -> Result<Self, ObservationError> {
        check_nonempty_text(&path, "path", OBSERVATION_PATH_MAX_BYTES)?;
        check_optional_text(
            caption.as_deref(),
            "caption",
            OBSERVATION_DESCRIPTION_MAX_BYTES,
        )?;
        Ok(Self {
            id,
            sequence,
            item_id,
            origin,
            path,
            caption,
        })
    }

    /// Returns the observation identity.
    #[must_use]
    pub const fn id(&self) -> &ObservationId {
        &self.id
    }

    /// Returns the durable sequence.
    #[must_use]
    pub const fn sequence(&self) -> ObservationSequence {
        self.sequence
    }

    /// Returns the native item or tool call that produced the image.
    #[must_use]
    pub const fn item_id(&self) -> &ObservationId {
        &self.item_id
    }

    /// Returns how the image entered the run.
    #[must_use]
    pub const fn origin(&self) -> ImageOrigin {
        self.origin
    }

    /// Returns the absolute image path on the engine host.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns the display caption, when disclosed.
    #[must_use]
    pub fn caption(&self) -> Option<&str> {
        self.caption.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: &str) -> ObservationId {
        ObservationId::parse(value).expect("valid observation id")
    }

    #[test]
    fn image_origin_round_trips_its_spelling() {
        for origin in [
            ImageOrigin::Generated,
            ImageOrigin::Viewed,
            ImageOrigin::ToolResult,
        ] {
            assert_eq!(ImageOrigin::parse(origin.as_str()), Ok(origin));
        }
        assert!(ImageOrigin::parse("pasted").is_err());
    }

    #[test]
    fn image_rejects_empty_path_and_caption() {
        let sequence = ObservationSequence::new(1).expect("valid sequence");
        assert!(
            ImageObservation::new(
                id("obs"),
                sequence,
                id("item"),
                ImageOrigin::Viewed,
                String::new(),
                None,
            )
            .is_err()
        );
        assert!(
            ImageObservation::new(
                id("obs"),
                sequence,
                id("item"),
                ImageOrigin::Viewed,
                "/tmp/a.png".to_owned(),
                Some(String::new()),
            )
            .is_err()
        );
    }
}
