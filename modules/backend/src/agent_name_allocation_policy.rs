//! Dependency-free policy for allocating thread-visible agent names.
//!
//! The TypeScript orchestration boundary supplies validated visible labels and
//! a random index. This native leaf keeps the name-bank and generation rules
//! pure: callers provide the already-bounded zero-based selector, and no
//! random source or orchestration state is consulted here.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::{collections::BTreeSet, fmt};

/// Maximum visible-name length in UTF-16 code units when a numeric generation
/// needs a suffix.
pub const VISIBLE_NAME_MAXIMUM: usize = 64;

/// The display-name spelling permanently reserved for the coordinator.
pub const RESERVED_COORDINATOR_NAME: &str = "coordinator";

/// The base used when the supplied name bank has no effective entries.
pub const FALLBACK_AGENT_NAME: &str = "Agent";

/// A selector could not identify one candidate in the available generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentNameAllocationError {
    /// The selector is outside the zero-based candidate range.
    SelectorOutOfRange {
        /// The caller-provided zero-based selector.
        selector: usize,
        /// The number of currently available candidates.
        candidate_count: usize,
    },
    /// Every representable generation was occupied.
    GenerationOverflow,
}

impl fmt::Display for AgentNameAllocationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SelectorOutOfRange {
                selector,
                candidate_count,
            } => write!(
                formatter,
                "selector {selector} is out of range for {candidate_count} candidates"
            ),
            Self::GenerationOverflow => formatter.write_str("agent name generation overflowed"),
        }
    }
}

impl std::error::Error for AgentNameAllocationError {}

/// Stateless entry point for the agent-name allocation policy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AgentNameAllocationPolicy;

impl AgentNameAllocationPolicy {
    /// Creates the stateless policy.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Selects one available name from the first available generation.
    ///
    /// Base names are considered in their first-occurrence order. Names and
    /// existing display labels compare case-insensitively, but the spelling of
    /// the first occurrence is retained in the returned name. Generation one
    /// is exhausted completely before a numeric suffix is considered. The
    /// selector is a zero-based index into the candidates that remain in that
    /// first non-empty generation; it is never clamped or remapped.
    ///
    /// # Errors
    ///
    /// Returns [`AgentNameAllocationError::SelectorOutOfRange`] when `selector`
    /// is not a candidate index in the first generation containing an
    /// available name. Returns
    /// [`AgentNameAllocationError::GenerationOverflow`] only if all
    /// representable numeric generations are occupied.
    pub fn choose_available_agent_name(
        name_bank: &[&str],
        existing_display_names: &[&str],
        selector: usize,
    ) -> Result<String, AgentNameAllocationError> {
        let mut used_names = BTreeSet::new();
        used_names.insert(normalize_name(RESERVED_COORDINATOR_NAME));
        for name in existing_display_names {
            used_names.insert(normalize_name(name));
        }

        let mut seen_bases = BTreeSet::new();
        let mut bases = Vec::new();
        for name in name_bank {
            let base = *name;
            if seen_bases.insert(normalize_name(base)) {
                bases.push(base.to_owned());
            }
        }
        if bases.is_empty() {
            bases.push(FALLBACK_AGENT_NAME.to_owned());
        }

        let mut generation = 1_usize;
        loop {
            let candidates = bases
                .iter()
                .map(|base| candidate_for(base, generation))
                .filter(|candidate| !used_names.contains(&normalize_name(candidate)))
                .collect::<Vec<_>>();

            if !candidates.is_empty() {
                let candidate_count = candidates.len();
                return candidates.into_iter().nth(selector).ok_or(
                    AgentNameAllocationError::SelectorOutOfRange {
                        selector,
                        candidate_count,
                    },
                );
            }

            generation = generation
                .checked_add(1)
                .ok_or(AgentNameAllocationError::GenerationOverflow)?;
        }
    }

    /// Selects one available name using the policy's pure bounded-index rule.
    ///
    /// This is an associated spelling of [`Self::choose_available_agent_name`]
    /// for callers that keep a policy value at their boundary.
    ///
    /// # Errors
    ///
    /// Returns the same selector or generation error as
    /// [`Self::choose_available_agent_name`].
    pub fn choose(
        name_bank: &[&str],
        existing_display_names: &[&str],
        selector: usize,
    ) -> Result<String, AgentNameAllocationError> {
        Self::choose_available_agent_name(name_bank, existing_display_names, selector)
    }
}

/// Selects one available name from the first available generation.
///
/// This free-function facade is the direct adapter surface for callers that do
/// not need to retain a [`AgentNameAllocationPolicy`] value. See
/// [`AgentNameAllocationPolicy::choose_available_agent_name`] for the full
/// selection contract.
///
/// # Errors
///
/// Returns the same selector or generation error as
/// [`AgentNameAllocationPolicy::choose_available_agent_name`].
pub fn choose_available_agent_name(
    name_bank: &[&str],
    existing_display_names: &[&str],
    selector: usize,
) -> Result<String, AgentNameAllocationError> {
    AgentNameAllocationPolicy::choose_available_agent_name(
        name_bank,
        existing_display_names,
        selector,
    )
}

fn normalize_name(name: &str) -> String {
    name.to_lowercase()
}

fn candidate_for(base: &str, generation: usize) -> String {
    if generation == 1 {
        return base.to_owned();
    }

    let suffix = format!(" {generation}");
    let bounded_suffix = suffix_tail(&suffix, VISIBLE_NAME_MAXIMUM);
    let suffix_length = utf16_code_unit_count(&bounded_suffix);
    let base_length = VISIBLE_NAME_MAXIMUM.saturating_sub(suffix_length);
    let bounded_base = utf16_prefix(base, base_length);
    format!("{bounded_base}{bounded_suffix}")
}

fn suffix_tail(value: &str, maximum: usize) -> String {
    let mut retained_reversed = String::new();
    let mut used = 0_usize;

    for character in value.chars().rev() {
        let character_length = character.len_utf16();
        if character_length > maximum.saturating_sub(used) {
            break;
        }
        retained_reversed.push(character);
        used += character_length;
    }

    retained_reversed.chars().rev().collect()
}

fn utf16_prefix(value: &str, maximum: usize) -> String {
    let mut bounded = String::new();
    let mut used = 0_usize;

    for character in value.chars() {
        let character_length = character.len_utf16();
        if character_length > maximum.saturating_sub(used) {
            break;
        }
        bounded.push(character);
        used += character_length;
    }

    bounded
}

fn utf16_code_unit_count(value: &str) -> usize {
    value.encode_utf16().count()
}
