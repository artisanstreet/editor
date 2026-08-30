#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

pub mod commands;
pub mod credentials;
pub(crate) mod engine_catalog;
pub(crate) mod engine_install;
pub(crate) mod engine_profiles;
pub mod error;
pub mod http;
pub mod instance;
pub mod manifest;
pub mod paths;
pub mod payload;
pub mod process;
pub mod telemetry;

pub use commands::{Cli, run};
pub use error::{CliError, Result};
