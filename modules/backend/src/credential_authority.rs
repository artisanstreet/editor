//! Single-use local credential authority for the Forge application handshake.
//!
//! Authentication consumes the expected capability only after a same-family,
//! constant-time value comparison succeeds. A successful authentication lends
//! the authority mutably to an unforgeable grant, so no second authentication
//! can race reconnect rotation. Rotation intentionally creates the only two
//! copies of a fresh reconnect secret: one for the Welcome and one retained as
//! the next expected credential. The caller-provided material is scrubbed
//! before control returns.

use std::{fmt, mem};

use artisan_protocol::{
    HelloCredential, LOCAL_CAPABILITY_BYTES, LocalCapability, RECONNECT_CAPABILITY_BYTES,
    ReconnectCapability,
};
use thiserror::Error;
use zeroize::Zeroize;

/// Non-secret credential family used in typed authentication failures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialKind {
    /// One-time capability supplied by the packaged editor at first contact.
    Initial,
    /// Single-use capability rotated by the preceding successful Welcome.
    Reconnect,
}

impl fmt::Display for CredentialKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Initial => "initial",
            Self::Reconnect => "reconnect",
        })
    }
}

/// Typed credential rejection with no secret material.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum CredentialAuthenticationError {
    /// A previous credential was accepted and reconnect rotation is unfinished.
    #[error("credential authority is awaiting reconnect rotation")]
    AwaitingRotation,
    /// The client presented the wrong credential family for this handshake.
    #[error("expected {expected} credential, received {presented} credential")]
    FamilyMismatch {
        /// Credential family currently owned by the authority.
        expected: CredentialKind,
        /// Credential family presented by the client.
        presented: CredentialKind,
    },
    /// The fixed-length credential bytes did not authenticate.
    #[error("{kind} credential was rejected")]
    Rejected {
        /// Rejected credential family; bytes are deliberately absent.
        kind: CredentialKind,
    },
}

/// Typed misuse of a staged reconnect rotation.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ReconnectRotationError {
    /// The Welcome credential has already been moved into an envelope.
    #[error("reconnect capability was already taken for Welcome")]
    AlreadyTaken,
    /// Rotation cannot commit before the Welcome credential leaves the stage.
    #[error("reconnect capability must be taken for Welcome before rotation commits")]
    NotTakenForWelcome,
}

/// Operating-system entropy was unavailable while minting a credential.
#[derive(Debug, Error)]
#[error("operating-system entropy failed while minting a reconnect capability: {source}")]
pub struct CredentialEntropyError {
    /// Typed source returned by the platform entropy provider.
    #[source]
    source: getrandom::Error,
}

enum ExpectedCredential {
    Initial(LocalCapability),
    Reconnect(ReconnectCapability),
}

enum CredentialState {
    Expected(ExpectedCredential),
    AwaitingRotation,
}

/// Process-local owner of the single credential valid for the next handshake.
///
/// This type deliberately implements neither `Clone` nor `Debug`: duplicating
/// the authority would duplicate replay state, while formatting it could make
/// later secret-bearing changes unsafe by default.
pub struct CredentialAuthority {
    state: CredentialState,
}

impl CredentialAuthority {
    /// Creates the first-contact authority from the launcher's one-time secret.
    #[must_use]
    pub const fn new(initial: LocalCapability) -> Self {
        Self {
            state: CredentialState::Expected(ExpectedCredential::Initial(initial)),
        }
    }

    /// Authenticates and consumes one presented credential.
    ///
    /// Wrong-family and wrong-value attempts leave the expected credential
    /// intact. A successful match drops and zeroizes both copies, transitions
    /// the authority to its rotation-only state, and returns an unforgeable
    /// mutable grant.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialAuthenticationError::AwaitingRotation`] after an
    /// earlier success, [`CredentialAuthenticationError::FamilyMismatch`] for
    /// the wrong credential family, or
    /// [`CredentialAuthenticationError::Rejected`] for a value mismatch.
    pub fn authenticate(
        &mut self,
        presented: HelloCredential,
    ) -> Result<AuthenticatedCredential<'_>, CredentialAuthenticationError> {
        let (expected_kind, presented_kind, matches) = match (&self.state, &presented) {
            (CredentialState::AwaitingRotation, _) => {
                return Err(CredentialAuthenticationError::AwaitingRotation);
            }
            (
                CredentialState::Expected(ExpectedCredential::Initial(expected)),
                HelloCredential::Initial(candidate),
            ) => (
                CredentialKind::Initial,
                CredentialKind::Initial,
                expected.constant_time_eq(candidate),
            ),
            (
                CredentialState::Expected(ExpectedCredential::Reconnect(expected)),
                HelloCredential::Reconnect(candidate),
            ) => (
                CredentialKind::Reconnect,
                CredentialKind::Reconnect,
                expected.constant_time_eq(candidate),
            ),
            (CredentialState::Expected(ExpectedCredential::Initial(_)), _) => {
                (CredentialKind::Initial, CredentialKind::Reconnect, false)
            }
            (CredentialState::Expected(ExpectedCredential::Reconnect(_)), _) => {
                (CredentialKind::Reconnect, CredentialKind::Initial, false)
            }
        };

        if expected_kind != presented_kind {
            return Err(CredentialAuthenticationError::FamilyMismatch {
                expected: expected_kind,
                presented: presented_kind,
            });
        }
        if !matches {
            return Err(CredentialAuthenticationError::Rejected {
                kind: expected_kind,
            });
        }

        let consumed = mem::replace(&mut self.state, CredentialState::AwaitingRotation);
        drop(consumed);
        drop(presented);
        Ok(AuthenticatedCredential { authority: self })
    }
}

/// Exclusive proof that the authority accepted exactly one credential.
///
/// The mutable borrow prevents authentication or parallel rotation until this
/// grant is consumed or dropped. It deliberately implements neither `Clone`
/// nor `Debug`.
pub struct AuthenticatedCredential<'authority> {
    authority: &'authority mut CredentialAuthority,
}

impl<'authority> AuthenticatedCredential<'authority> {
    /// Mints and stages the next reconnect pair from operating-system entropy.
    ///
    /// The temporary source buffer is scrubbed on both success and failure.
    /// An entropy failure consumes this authentication grant and leaves the
    /// authority in its fail-closed rotation-only state.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialEntropyError`] when the operating system cannot
    /// fill the fixed-size capability buffer.
    pub fn prepare_system_reconnect(
        self,
    ) -> Result<PendingReconnect<'authority>, CredentialEntropyError> {
        let mut material = [0_u8; RECONNECT_CAPABILITY_BYTES];
        if let Err(source) = getrandom::fill(&mut material) {
            material.zeroize();
            return Err(CredentialEntropyError { source });
        }
        Ok(self.prepare_reconnect(&mut material))
    }

    /// Stages the next server/client reconnect-capability pair.
    ///
    /// `material` must come from a cryptographically secure generator. It is
    /// copied only into the two required owned capabilities and then zeroized
    /// before this function returns.
    #[must_use]
    pub fn prepare_reconnect(
        self,
        material: &mut [u8; RECONNECT_CAPABILITY_BYTES],
    ) -> PendingReconnect<'authority> {
        let expected = ReconnectCapability::from_bytes(*material);
        let for_welcome = ReconnectCapability::from_bytes(*material);
        material.zeroize();
        PendingReconnect {
            authority: self.authority,
            expected: Some(expected),
            for_welcome: Some(for_welcome),
        }
    }
}

/// Staged reconnect rotation spanning Welcome construction and successful send.
///
/// Call [`Self::take_for_welcome`] exactly once, send that capability in the
/// Welcome, then call [`Self::commit`] only after the transport send succeeds.
/// Dropping this value without commit zeroizes the staged server copy and
/// leaves the authority unable to accept another credential, which is the safe
/// failure mode when the client may not have received its replacement secret.
pub struct PendingReconnect<'authority> {
    authority: &'authority mut CredentialAuthority,
    expected: Option<ReconnectCapability>,
    for_welcome: Option<ReconnectCapability>,
}

impl PendingReconnect<'_> {
    /// Moves the client copy into a Welcome exactly once.
    ///
    /// # Errors
    ///
    /// Returns [`ReconnectRotationError::AlreadyTaken`] after the first call.
    pub fn take_for_welcome(&mut self) -> Result<ReconnectCapability, ReconnectRotationError> {
        self.for_welcome
            .take()
            .ok_or(ReconnectRotationError::AlreadyTaken)
    }

    /// Installs the server copy as the only credential valid for reconnection.
    ///
    /// Call this only after the Welcome send completes successfully.
    ///
    /// # Errors
    ///
    /// Returns [`ReconnectRotationError::NotTakenForWelcome`] when the client
    /// copy has not left this stage.
    pub fn commit(mut self) -> Result<(), ReconnectRotationError> {
        if self.for_welcome.is_some() {
            return Err(ReconnectRotationError::NotTakenForWelcome);
        }
        let Some(expected) = self.expected.take() else {
            return Err(ReconnectRotationError::NotTakenForWelcome);
        };
        self.authority.state = CredentialState::Expected(ExpectedCredential::Reconnect(expected));
        Ok(())
    }
}

const _: () = assert!(LOCAL_CAPABILITY_BYTES == RECONNECT_CAPABILITY_BYTES);
