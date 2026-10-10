//! A provider's safety system stepping into a request.
//!
//! Providers screen requests for policy areas such as cybersecurity or
//! biology. When a screen holds a response back, moves the request to a more
//! conservative model, or refuses it outright, the adapter reports it here so
//! the reader learns why the turn changed course instead of seeing a generic
//! failure.

use super::{
    OBSERVATION_LABEL_MAX_BYTES, OBSERVATION_PROVIDER_CODE_MAX_BYTES, OBSERVATION_TEXT_MAX_BYTES,
    ObservationError, ObservationId, ObservationSequence, check_optional_text,
};

/// What the provider's safety system did to the request.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SafetyAction {
    /// The response is held back while the provider runs a safety check.
    Holding,
    /// The check finished and the response continues.
    Released,
    /// The request was moved to a more conservative model.
    Rerouted,
    /// The provider refused to serve the request.
    Refused,
}

impl SafetyAction {
    /// Returns the stable spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Holding => "holding",
            Self::Released => "released",
            Self::Rerouted => "rerouted",
            Self::Refused => "refused",
        }
    }

    /// Parses a stored safety action.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "holding" => Ok(Self::Holding),
            "released" => Ok(Self::Released),
            "rerouted" => Ok(Self::Rerouted),
            "refused" => Ok(Self::Refused),
            _ => Err(ObservationError::UnknownValue { field: "action" }),
        }
    }
}

/// The policy area the provider named, when it named one.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SafetyCategory {
    /// Cybersecurity: exploits, malware, intrusion.
    Cyber,
    /// Biology: pathogens and biological weapons.
    Bio,
    /// Chemistry: chemical weapons.
    Chemical,
    /// Another usage-policy area the provider did not narrow down.
    Policy,
    /// The provider gave no category.
    Unspecified,
}

impl SafetyCategory {
    /// Returns the stable spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cyber => "cyber",
            Self::Bio => "bio",
            Self::Chemical => "chemical",
            Self::Policy => "policy",
            Self::Unspecified => "unspecified",
        }
    }

    /// Parses a stored safety category.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "cyber" => Ok(Self::Cyber),
            "bio" => Ok(Self::Bio),
            "chemical" => Ok(Self::Chemical),
            "policy" => Ok(Self::Policy),
            "unspecified" => Ok(Self::Unspecified),
            _ => Err(ObservationError::UnknownValue { field: "category" }),
        }
    }
}

/// One report of a provider safety system acting on the request.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SafetyObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    pub(super) action: SafetyAction,
    pub(super) category: SafetyCategory,
    pub(super) provider_code: Option<String>,
    pub(super) message: Option<String>,
    pub(super) model: Option<String>,
}

impl SafetyObservation {
    /// Creates a safety observation after validating its text bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when a present provider code, message, or
    /// model is empty or exceeds its byte bound.
    pub fn new(
        id: ObservationId,
        sequence: ObservationSequence,
        action: SafetyAction,
        category: SafetyCategory,
        provider_code: Option<String>,
        message: Option<String>,
        model: Option<String>,
    ) -> Result<Self, ObservationError> {
        check_optional_text(
            provider_code.as_deref(),
            "provider_code",
            OBSERVATION_PROVIDER_CODE_MAX_BYTES,
        )?;
        check_optional_text(message.as_deref(), "message", OBSERVATION_TEXT_MAX_BYTES)?;
        check_optional_text(model.as_deref(), "model", OBSERVATION_LABEL_MAX_BYTES)?;
        Ok(Self {
            id,
            sequence,
            action,
            category,
            provider_code,
            message,
            model,
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

    /// Returns what the safety system did.
    #[must_use]
    pub const fn action(&self) -> SafetyAction {
        self.action
    }

    /// Returns the policy area, when the provider named one.
    #[must_use]
    pub const fn category(&self) -> SafetyCategory {
        self.category
    }

    /// Returns the provider's own code for the intervention, when disclosed.
    #[must_use]
    pub fn provider_code(&self) -> Option<&str> {
        self.provider_code.as_deref()
    }

    /// Returns the provider's explanation for the reader, when supplied.
    #[must_use]
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// Returns the model the request now runs on after a reroute.
    #[must_use]
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: &str) -> ObservationId {
        ObservationId::parse(value).expect("valid observation id")
    }

    #[test]
    fn safety_spellings_round_trip() {
        for action in [
            SafetyAction::Holding,
            SafetyAction::Released,
            SafetyAction::Rerouted,
            SafetyAction::Refused,
        ] {
            assert_eq!(SafetyAction::parse(action.as_str()), Ok(action));
        }
        for category in [
            SafetyCategory::Cyber,
            SafetyCategory::Bio,
            SafetyCategory::Chemical,
            SafetyCategory::Policy,
            SafetyCategory::Unspecified,
        ] {
            assert_eq!(SafetyCategory::parse(category.as_str()), Ok(category));
        }
        assert!(SafetyAction::parse("blocked").is_err());
        assert!(SafetyCategory::parse("weapons").is_err());
    }

    #[test]
    fn safety_rejects_empty_present_text() {
        let sequence = ObservationSequence::new(1).expect("valid sequence");
        assert!(
            SafetyObservation::new(
                id("safety-1"),
                sequence,
                SafetyAction::Refused,
                SafetyCategory::Cyber,
                Some(String::new()),
                None,
                None,
            )
            .is_err()
        );
        let row = SafetyObservation::new(
            id("safety-2"),
            sequence,
            SafetyAction::Rerouted,
            SafetyCategory::Cyber,
            Some("highRiskCyberActivity".to_owned()),
            None,
            Some("gpt-5.2".to_owned()),
        )
        .expect("valid safety row");
        assert_eq!(row.model(), Some("gpt-5.2"));
        assert_eq!(row.action(), SafetyAction::Rerouted);
    }
}
