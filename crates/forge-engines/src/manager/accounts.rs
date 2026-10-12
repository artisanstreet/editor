//! Sign-in, usage and customisation on `Engines` (see `account.rs`,
//! `usage.rs`, `customise.rs`). Every change is published as an
//! `EngineStatus`.

use super::Engines;
use crate::{Customisation, Launch};
use anyhow::{Context, Result};
use artisan_api::{EngineId, SignInState, UsageLimits};
use std::time::Duration;

/// How long a usage read is served before asking the engine again.
pub(super) const USAGE_TTL: Duration = Duration::from_mins(5);

impl Engines {
    /// Probes whether `engine` is signed in (no model call) and publishes
    /// the result. Does nothing while the engine isn't installed.
    /// Re-reads everything Forge knows about an installed engine: sign-in,
    /// models and usage. Done at startup and after installs.
    pub async fn refresh(&self, engine: EngineId) {
        if self.binary(engine).is_none() {
            return;
        }
        self.check_sign_in(engine).await;
        if let Err(error) = self.refresh_models(engine).await {
            tracing::info!(%engine, "couldn't list models: {error:#}");
        }
        self.refresh_usage(engine).await;
    }

    pub async fn check_sign_in(&self, engine: EngineId) -> SignInState {
        let sign_in = &self.slot(engine).sign_in;
        if let Some(binary) = self.binary(engine) {
            sign_in.check(&binary).await;
            self.publish(engine);
        }
        sign_in.state()
    }

    /// Starts the engine's own sign-in. The status moves to `LoggingIn`
    /// and gains the link and device code as the engine prints them.
    ///
    /// # Errors
    ///
    /// When the engine isn't installed.
    pub fn login(&self, engine: EngineId) -> Result<()> {
        let binary = self
            .binary(engine)
            .with_context(|| format!("{} is not installed", engine.display_name()))?;
        self.follow_sign_in(engine);
        self.slot(engine).sign_in.login(&binary);
        Ok(())
    }

    /// Passes what the user pasted to a running sign-in (Claude's code).
    ///
    /// # Errors
    ///
    /// When no sign-in is running for `engine`.
    pub async fn submit_login_code(&self, engine: EngineId, code: &str) -> Result<()> {
        self.slot(engine).sign_in.submit_code(code).await
    }

    pub fn cancel_login(&self, engine: EngineId) {
        self.follow_sign_in(engine);
        self.slot(engine).sign_in.cancel_login();
    }

    /// # Errors
    ///
    /// When the engine isn't installed.
    pub fn logout(&self, engine: EngineId) -> Result<()> {
        let binary = self
            .binary(engine)
            .with_context(|| format!("{} is not installed", engine.display_name()))?;
        self.follow_sign_in(engine);
        self.slot(engine).sign_in.logout(&binary);
        self.inner.usage.invalidate(engine);
        Ok(())
    }

    /// Reads usage unless a read younger than five minutes is cached, and
    /// publishes it. `None` means "usage unavailable".
    pub async fn refresh_usage(&self, engine: EngineId) -> Option<UsageLimits> {
        let binary = self.binary(engine)?;
        let usage = self
            .inner
            .usage
            .read(engine, &binary, &self.home(engine))
            .await;
        self.publish(engine);
        usage
    }

    /// Writes the prompt and skill links into the engine home and returns
    /// the launch arguments for MCP servers. Call right before each spawn.
    ///
    /// # Errors
    ///
    /// When the engine home can't be written.
    pub fn materialize(&self, engine: EngineId, customisation: &Customisation) -> Result<Launch> {
        crate::materialize(engine, &self.home(engine), customisation)
    }

    /// Publishes every sign-in change until the sign-in settles.
    fn follow_sign_in(&self, engine: EngineId) {
        let mut changes = self.slot(engine).sign_in.subscribe();
        let engines = self.clone();
        tokio::spawn(async move {
            while changes.changed().await.is_ok() {
                engines.publish(engine);
                let settled = matches!(
                    *changes.borrow_and_update(),
                    SignInState::SignedIn { .. }
                        | SignInState::SignedOut
                        | SignInState::Failed { .. }
                );
                if settled {
                    engines.inner.usage.invalidate(engine);
                    break;
                }
            }
        });
    }
}
