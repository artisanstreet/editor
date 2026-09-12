//! Provider-neutral engine-socket adapters.
//!
//! This module is the backend half of the `EngineSocket` migration: the
//! domain [`EngineSocket`](artisan_domain::EngineSocket) seam, implemented
//! per provider by an adapter that holds the same verified launch capability
//! the owner executors already receive. The first packet ships the Codex
//! skeleton only: it reports the exact descriptor, exposes the verified
//! launch version through [`probe`](artisan_domain::EngineSocket::probe), and
//! returns
//! [`EngineOpenError::Unimplemented`](artisan_domain::EngineOpenError::Unimplemented)
//! from every open attempt.
//!
//! Nothing constructs an adapter yet and no production path routes through
//! this module, so the module-level `dead_code` allow stands until the wiring
//! packet lands. Deliberately no I/O, no spawn, and no task creation here:
//! the existing owner executors keep all custody.

#![forbid(unsafe_code)]
#![allow(dead_code)]

pub(crate) mod codex;
pub(crate) mod claude;
pub(crate) mod cursor;
pub(crate) mod grok;
pub(crate) mod hermes;
pub(crate) mod opencode;
