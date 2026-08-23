use std::collections::{BTreeMap, BTreeSet};

pub const BLOCK_THRESHOLD: u16 = 80;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CountryCode([u8; 2]);

impl CountryCode {
    /// Parses an ISO 3166-1 alpha-2 country code.
    ///
    /// # Errors
    ///
    /// Returns [`BrokerConfigurationError::InvalidCountry`] when the value is
    /// not exactly two ASCII letters.
    pub fn parse(value: &str) -> Result<Self, BrokerConfigurationError> {
        let normalized = value.trim().to_ascii_uppercase();
        let bytes = normalized.as_bytes();
        if bytes.len() != 2 || !bytes.iter().all(u8::is_ascii_alphabetic) {
            return Err(BrokerConfigurationError::InvalidCountry(value.to_owned()));
        }
        Ok(Self([bytes[0], bytes[1]]))
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum SignalSource {
    Account,
    Billing,
    Network,
    Device,
    SystemLocale,
    EnvironmentLocale,
}

impl SignalSource {
    const fn weight(self) -> u16 {
        match self {
            Self::Account => 100,
            Self::Billing => 90,
            Self::Network => 80,
            Self::Device => 60,
            Self::SystemLocale => 30,
            Self::EnvironmentLocale => 20,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CountrySignal {
    pub country: CountryCode,
    pub source: SignalSource,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Policy {
    pub blocked_countries: BTreeSet<CountryCode>,
    pub fail_closed: bool,
    pub sanctions_match: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Evaluation {
    pub block: bool,
    pub blocked_country: Option<CountryCode>,
    pub evidence_score: u16,
}

#[derive(Debug, Eq, PartialEq)]
pub enum BrokerConfigurationError {
    InvalidBoolean(String),
    InvalidCountry(String),
}

impl std::fmt::Display for BrokerConfigurationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBoolean(value) => write!(formatter, "invalid boolean: {value}"),
            Self::InvalidCountry(value) => write!(formatter, "invalid ISO country code: {value}"),
        }
    }
}

impl std::error::Error for BrokerConfigurationError {}

#[must_use]
pub fn evaluate(policy: &Policy, signals: &[CountrySignal]) -> Evaluation {
    if policy.sanctions_match {
        return Evaluation {
            block: true,
            blocked_country: None,
            evidence_score: u16::MAX,
        };
    }

    // Count a source at most once for a country. Repeating the same correlated
    // observation must not manufacture confidence.
    let mut observed = BTreeSet::new();
    let mut scores = BTreeMap::<CountryCode, u16>::new();
    let mut reliable_signal_present = false;
    for signal in signals {
        if observed.insert((signal.country, signal.source)) {
            reliable_signal_present |= signal.source.weight() >= BLOCK_THRESHOLD;
            let score = scores.entry(signal.country).or_default();
            *score = score.saturating_add(signal.source.weight());
        }
    }

    let blocked = scores
        .into_iter()
        .filter(|(country, _)| policy.blocked_countries.contains(country))
        .max_by_key(|(country, score)| (*score, *country));
    match blocked {
        Some((country, score)) if score >= BLOCK_THRESHOLD => Evaluation {
            block: true,
            blocked_country: Some(country),
            evidence_score: score,
        },
        Some((country, score)) => Evaluation {
            block: policy.fail_closed,
            blocked_country: Some(country),
            evidence_score: score,
        },
        None => Evaluation {
            block: policy.fail_closed && !reliable_signal_present,
            blocked_country: None,
            evidence_score: 0,
        },
    }
}

/// Loads the explicit country policy from the Broker process environment.
///
/// # Errors
///
/// Returns an error when a country code or boolean policy value is invalid.
pub fn policy_from_environment(
    environment: &impl Environment,
) -> Result<Policy, BrokerConfigurationError> {
    let blocked_countries = environment
        .value("ARTISAN_BROKER_BLOCKED_COUNTRIES")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(CountryCode::parse)
        .collect::<Result<BTreeSet<_>, _>>()?;
    Ok(Policy {
        blocked_countries,
        fail_closed: boolean_value(environment, "ARTISAN_BROKER_FAIL_CLOSED")?,
        sanctions_match: boolean_value(environment, "ARTISAN_BROKER_SANCTIONS_MATCH")?,
    })
}

/// Collects explicit high-confidence inputs and weak host locale observations.
///
/// # Errors
///
/// Returns an error when an explicit country signal is invalid.
pub fn signals_from_environment(
    environment: &impl Environment,
) -> Result<Vec<CountrySignal>, BrokerConfigurationError> {
    let mut signals = Vec::new();
    for (name, source) in [
        ("ARTISAN_BROKER_ACCOUNT_COUNTRY", SignalSource::Account),
        ("ARTISAN_BROKER_BILLING_COUNTRY", SignalSource::Billing),
        ("ARTISAN_BROKER_NETWORK_COUNTRY", SignalSource::Network),
        ("ARTISAN_BROKER_DEVICE_COUNTRY", SignalSource::Device),
    ] {
        if let Some(value) = environment
            .value(name)
            .filter(|value| !value.trim().is_empty())
        {
            signals.push(CountrySignal {
                country: CountryCode::parse(&value)?,
                source,
            });
        }
    }

    if let Some(country) = sys_locale::get_locale().and_then(|locale| country_from_locale(&locale))
    {
        signals.push(CountrySignal {
            country,
            source: SignalSource::SystemLocale,
        });
    }
    for name in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Some(country) = environment
            .value(name)
            .as_deref()
            .and_then(country_from_locale)
        {
            signals.push(CountrySignal {
                country,
                source: SignalSource::EnvironmentLocale,
            });
        }
    }
    Ok(signals)
}

pub trait Environment {
    fn value(&self, name: &str) -> Option<String>;
}

pub struct ProcessEnvironment;

impl Environment for ProcessEnvironment {
    fn value(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

fn boolean_value(
    environment: &impl Environment,
    name: &str,
) -> Result<bool, BrokerConfigurationError> {
    match environment.value(name).as_deref().map(str::trim) {
        None | Some("" | "0" | "false") => Ok(false),
        Some("1" | "true") => Ok(true),
        Some(value) => Err(BrokerConfigurationError::InvalidBoolean(value.to_owned())),
    }
}

fn country_from_locale(locale: &str) -> Option<CountryCode> {
    let base = locale.split(['.', '@']).next()?;
    base.split(['-', '_'])
        .rev()
        .find(|part| part.len() == 2 && part.bytes().all(|byte| byte.is_ascii_alphabetic()))
        .and_then(|country| CountryCode::parse(country).ok())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    struct TestEnvironment(BTreeMap<String, String>);

    impl Environment for TestEnvironment {
        fn value(&self, name: &str) -> Option<String> {
            self.0.get(name).cloned()
        }
    }

    fn country(value: &str) -> CountryCode {
        CountryCode::parse(value).expect("country")
    }

    #[test]
    fn one_strong_blocked_signal_blocks() {
        let policy = Policy {
            blocked_countries: BTreeSet::from([country("RU")]),
            ..Policy::default()
        };
        let result = evaluate(
            &policy,
            &[CountrySignal {
                country: country("RU"),
                source: SignalSource::Network,
            }],
        );
        assert!(result.block);
        assert_eq!(result.evidence_score, 80);
    }

    #[test]
    fn locale_observations_alone_do_not_block() {
        let policy = Policy {
            blocked_countries: BTreeSet::from([country("RU")]),
            ..Policy::default()
        };
        let result = evaluate(
            &policy,
            &[
                CountrySignal {
                    country: country("RU"),
                    source: SignalSource::SystemLocale,
                },
                CountrySignal {
                    country: country("RU"),
                    source: SignalSource::EnvironmentLocale,
                },
            ],
        );
        assert!(!result.block);
        assert_eq!(result.evidence_score, 50);
    }

    #[test]
    fn duplicate_sources_do_not_inflate_confidence() {
        let policy = Policy {
            blocked_countries: BTreeSet::from([country("BY")]),
            ..Policy::default()
        };
        let repeated = CountrySignal {
            country: country("BY"),
            source: SignalSource::Device,
        };
        let result = evaluate(&policy, &[repeated, repeated]);
        assert!(!result.block);
        assert_eq!(result.evidence_score, 60);
    }

    #[test]
    fn a_sanctions_entity_match_blocks_without_country_inference() {
        let result = evaluate(
            &Policy {
                sanctions_match: true,
                ..Policy::default()
            },
            &[],
        );
        assert!(result.block);
        assert_eq!(result.blocked_country, None);
    }

    #[test]
    fn fail_closed_blocks_when_no_signal_exists() {
        let result = evaluate(
            &Policy {
                fail_closed: true,
                ..Policy::default()
            },
            &[],
        );
        assert!(result.block);
    }

    #[test]
    fn fail_closed_requires_a_reliable_signal() {
        let policy = Policy {
            fail_closed: true,
            ..Policy::default()
        };
        let weak = evaluate(
            &policy,
            &[CountrySignal {
                country: country("NO"),
                source: SignalSource::SystemLocale,
            }],
        );
        let strong = evaluate(
            &policy,
            &[CountrySignal {
                country: country("NO"),
                source: SignalSource::Network,
            }],
        );
        assert!(weak.block);
        assert!(!strong.block);
    }

    #[test]
    fn parses_policy_and_explicit_signals() {
        let environment = TestEnvironment(BTreeMap::from([
            ("ARTISAN_BROKER_BLOCKED_COUNTRIES".into(), "ru, by".into()),
            ("ARTISAN_BROKER_FAIL_CLOSED".into(), "true".into()),
            ("ARTISAN_BROKER_NETWORK_COUNTRY".into(), "ru".into()),
        ]));
        let policy = policy_from_environment(&environment).expect("policy");
        let signals = signals_from_environment(&environment).expect("signals");
        assert!(policy.blocked_countries.contains(&country("RU")));
        assert!(policy.blocked_countries.contains(&country("BY")));
        assert!(policy.fail_closed);
        assert!(signals.iter().any(|signal| {
            signal.country == country("RU") && signal.source == SignalSource::Network
        }));
    }

    #[test]
    fn extracts_regions_from_common_locale_shapes() {
        assert_eq!(country_from_locale("en_US.UTF-8"), Some(country("US")));
        assert_eq!(country_from_locale("zh-Hans-CN"), Some(country("CN")));
        assert_eq!(country_from_locale("C"), None);
    }
}
