//! Owned pinned-identity material for exact-leaf certificate pinning.
//!
//! [`PinnedIdentity`] records the SHA-256 fingerprint of the full Forge
//! end-entity DER certificate handed over by the local authenticated
//! pairing flow. Certificate fingerprints are public identity material,
//! not credentials: they compare with ordinary equality and render as
//! lowercase hex. Parse failures are typed and never echo the rejected
//! payload into errors, logs, or panics.

use std::fmt;

use rustls_pki_types::CertificateDer;
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Length of a SHA-256 digest in bytes.
const DIGEST_LEN: usize = 32;

/// Length of the hexadecimal rendering of a SHA-256 digest.
const HEX_LEN: usize = DIGEST_LEN * 2;

/// Failures raised while parsing a pinned-identity fingerprint.
///
/// Variants deliberately carry no payload: fingerprints arrive from
/// untrusted surfaces, and rejected input is never echoed anywhere.
#[derive(Debug, Clone, Copy, Eq, Error, PartialEq)]
pub enum PinnedIdentityError {
    /// The payload was not exactly 64 characters long.
    #[error("pinned identity must be exactly 64 hexadecimal characters")]
    WrongLength,

    /// The payload contained a character outside the hexadecimal alphabet.
    #[error("pinned identity contains a non-hexadecimal character")]
    NonHex,
}

/// The SHA-256 fingerprint of exactly one end-entity certificate.
///
/// Construction is infallible from DER or raw digest bytes; the only
/// fallible path is [`PinnedIdentity::parse`] over hand-supplied hex.
/// Equality is ordinary byte equality because a certificate fingerprint
/// is public identity material rather than a secret.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct PinnedIdentity {
    fingerprint: [u8; DIGEST_LEN],
}

impl PinnedIdentity {
    /// Hash algorithm behind every pin produced by this type.
    pub const ALGORITHM: &'static str = "sha256";

    /// Pins the full DER encoding of the end-entity `certificate` by
    /// SHA-256.
    ///
    /// The hash covers the entire certificate exactly as handed off;
    /// neither the issuing chain nor the subject name participates.
    #[must_use]
    pub fn from_certificate(certificate: &CertificateDer<'_>) -> Self {
        Self::from_digest(hash_certificate(certificate))
    }

    /// Adopts an already-computed 32-byte SHA-256 digest.
    ///
    /// A digest computed over anything other than the end-entity DER
    /// simply pins a different identity; no check here can distinguish
    /// intent from bytes.
    #[must_use]
    pub fn from_digest(digest: [u8; DIGEST_LEN]) -> Self {
        Self {
            fingerprint: digest,
        }
    }

    /// Parses a strictly 64-character hexadecimal fingerprint.
    ///
    /// Both letter cases are accepted; rendering stays lowercase, and
    /// no surrounding whitespace is trimmed.
    ///
    /// # Errors
    ///
    /// Returns [`PinnedIdentityError::WrongLength`] when the payload is
    /// not exactly 64 characters and [`PinnedIdentityError::NonHex`]
    /// when any character falls outside the hexadecimal alphabet. The
    /// rejected payload never appears in the returned error.
    pub fn parse(fingerprint: &str) -> Result<Self, PinnedIdentityError> {
        Ok(Self::from_digest(parse_hex(fingerprint)?))
    }

    /// Borrows the raw 32-byte SHA-256 digest.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; DIGEST_LEN] {
        &self.fingerprint
    }

    /// Renders the fingerprint as 64 lowercase hexadecimal characters.
    #[must_use]
    pub fn to_hex(&self) -> String {
        encode_hex(&self.fingerprint)
    }
}

impl fmt::Display for PinnedIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.to_hex())
    }
}

impl fmt::Debug for PinnedIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PinnedIdentity")
            .field("algorithm", &Self::ALGORITHM)
            .field("fingerprint", &self.to_hex())
            .finish()
    }
}

/// Computes SHA-256 over the complete DER bytes of `certificate`.
fn hash_certificate(certificate: &CertificateDer<'_>) -> [u8; DIGEST_LEN] {
    let digest = Sha256::digest(certificate.as_ref());
    let mut fingerprint = [0u8; DIGEST_LEN];
    fingerprint.copy_from_slice(&digest);
    fingerprint
}

/// Decodes exactly 64 hexadecimal characters into a 32-byte digest.
fn parse_hex(fingerprint: &str) -> Result<[u8; DIGEST_LEN], PinnedIdentityError> {
    let bytes = fingerprint.as_bytes();
    if bytes.len() != HEX_LEN {
        return Err(PinnedIdentityError::WrongLength);
    }
    let mut digest = [0u8; DIGEST_LEN];
    for (pair, slot) in bytes.chunks_exact(2).zip(&mut digest) {
        *slot = (hex_value(pair[0])? << 4) | hex_value(pair[1])?;
    }
    Ok(digest)
}

/// Maps one ASCII hexadecimal digit onto its 4-bit value.
const fn hex_value(nibble: u8) -> Result<u8, PinnedIdentityError> {
    match nibble {
        b'0'..=b'9' => Ok(nibble - b'0'),
        b'a'..=b'f' => Ok(nibble - b'a' + 0x0a),
        b'A'..=b'F' => Ok(nibble - b'A' + 0x0a),
        _ => Err(PinnedIdentityError::NonHex),
    }
}

/// Encodes `bytes` as lowercase hexadecimal without allocation churn.
fn encode_hex(bytes: &[u8]) -> String {
    const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        hex.push(HEX_DIGITS[usize::from(byte >> 4)] as char);
        hex.push(HEX_DIGITS[usize::from(byte & 0x0f)] as char);
    }
    hex
}
