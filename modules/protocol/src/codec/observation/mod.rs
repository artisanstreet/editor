//! Finite engine-observation and transcript delivery codec.
//!
//! Owns observation identifiers, every observation enum wire conversion,
//! engine-observation encode/decode, and transcript content codec. Each row is
//! an already-validated domain value; decoding re-validates through domain
//! constructors so hostile peers cannot smuggle invalid rows across the wire.

#[allow(clippy::wildcard_imports)]
use super::*;

// ---------------------------------------------------------------------------
// S1b: finite engine-observation delivery.
// ---------------------------------------------------------------------------
//
// Every observation below is an already-validated, sanitized S1a domain
// value. Encoding is infallible except where a collection must fit Cap'n
// Proto's 32-bit list length; decoding re-validates every bound through the
// domain constructors so a hostile peer can never smuggle an over-long,
// empty-required, unknown-label, or state-inconsistent row across the wire.

mod decode;
mod encode;
mod scalars;

pub(crate) use self::{decode::*, encode::*, scalars::*};
