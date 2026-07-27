#![allow(clippy::missing_errors_doc, clippy::must_use_candidate)]

pub mod commands;
pub mod error;
pub mod http;
pub mod manifest;
pub mod paths;
pub mod process;
pub mod profile;

pub use commands::{Cli, run};
pub use error::{CliError, Result};
