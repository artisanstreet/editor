use crate::core::client::CallError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    headline: String,
    detail: Option<String>,
}

impl Problem {
    #[must_use]
    pub fn new(headline: impl Into<String>) -> Self {
        Self {
            headline: headline.into(),
            detail: None,
        }
    }

    #[must_use]
    pub fn caused_by(headline: impl Into<String>, cause: &dyn std::error::Error) -> Self {
        Self::detailed(headline, ::diagnostics::chain(cause))
    }

    #[must_use]
    pub fn detailed(headline: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            headline: headline.into(),
            detail: Some(detail.into()),
        }
    }

    #[must_use]
    pub fn calling_forge(headline: &str, error: &CallError) -> Self {
        Self::caused_by(format!("{headline} {}", error.fault().sentence()), error)
    }

    #[must_use]
    pub fn headline(&self) -> &str {
        &self.headline
    }

    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.headline)
    }
}

impl crate::core::ResidentBytes for Problem {
    fn resident_bytes(&self) -> usize {
        self.headline.capacity() + self.detail.as_ref().map_or(0, String::capacity)
    }
}
