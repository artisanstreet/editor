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
/// How often the background refresher looks for due readings.
const USAGE_TICK: Duration = Duration::from_secs(30);

impl Engines {
    /// Probes whether `engine` is signed in (no model call) and publishes
    /// the result. Does nothing while the engine isn't installed.
    /// Re-reads everything Forge knows about an installed engine: sign-in,
    /// models and usage. Done at startup and after installs.
    pub async fn refresh(&self, engine: EngineId) {
        let engines = self.clone();
        if let Err(error) =
            tokio::task::spawn_blocking(move || engines.reconcile_install(engine)).await
        {
            tracing::warn!(%engine, %error, "reconciling an engine's install");
        }
        if self.binary(engine).is_none() {
            return;
        }
        self.check_sign_in(engine).await;
        if let Err(error) = self.refresh_models(engine).await {
            tracing::info!(%engine, "couldn't list models: {error:#}");
        }
        self.refresh_usage(engine, true).await;
    }

    /// Keeps every engine's usage current in the background, as master's
    /// Forge did: each is read again once its reading is due (five minutes
    /// old, or a minute after a failed read). Runs until Forge stops.
    pub async fn keep_usage_fresh(&self) {
        let mut tick = tokio::time::interval(USAGE_TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            for engine in EngineId::ALL {
                if crate::has_usage_reader(engine) && self.binary(engine).is_some() {
                    self.refresh_usage(engine, false).await;
                }
            }
        }
    }

    pub async fn check_sign_in(&self, engine: EngineId) -> SignInState {
        let sign_in = &self.slot(engine).sign_in;
        if let Some(lease) = self.lease(engine) {
            sign_in.check(lease.path()).await;
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
        let lease = self
            .lease(engine)
            .with_context(|| format!("{} is not installed", engine.display_name()))?;
        self.follow_sign_in(engine);
        self.invalidate_models(engine);
        self.slot(engine).sign_in.login_leased(lease);
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
        let lease = self
            .lease(engine)
            .with_context(|| format!("{} is not installed", engine.display_name()))?;
        self.follow_sign_in(engine);
        self.invalidate_models(engine);
        self.slot(engine).sign_in.logout_leased(lease);
        self.inner.usage.invalidate(engine);
        Ok(())
    }

    /// Reads usage when due (or with `force`), and publishes it. `None`
    /// means "usage unavailable": no good read yet.
    pub async fn refresh_usage(&self, engine: EngineId, force: bool) -> Option<UsageLimits> {
        let lease = self.lease(engine)?;
        let usage = self
            .inner
            .usage
            .read(engine, lease.path(), &self.home(engine), force)
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

    /// One owned follower per engine, regardless of repeated button presses.
    fn follow_sign_in(&self, engine: EngineId) {
        let mut owner = super::lock(&self.slot(engine).sign_in_follower);
        if owner.as_ref().is_some_and(|task| !task.is_finished()) {
            return;
        }
        let mut changes = self.slot(engine).sign_in.subscribe();
        let weak = std::sync::Arc::downgrade(&self.inner);
        *owner = Some(tokio_util::task::AbortOnDropHandle::new(tokio::spawn(
            async move {
                while changes.changed().await.is_ok() {
                    let state = changes.borrow_and_update().clone();
                    let Some(inner) = weak.upgrade() else { break };
                    let engines = Self { inner };
                    engines.publish(engine);
                    if matches!(
                        state,
                        SignInState::SignedIn { .. }
                            | SignInState::SignedOut
                            | SignInState::Failed { .. }
                    ) {
                        engines.inner.usage.invalidate(engine);
                        // OpenCode's free models need no account.
                        if matches!(state, SignInState::SignedIn { .. })
                            || (engine == EngineId::OpenCode
                                && matches!(state, SignInState::SignedOut))
                        {
                            if let Err(error) = engines.refresh_models(engine).await {
                                tracing::info!(%engine, "couldn't refresh models after sign-in: {error:#}");
                            }
                        } else {
                            engines.invalidate_models(engine);
                            engines.publish(engine);
                        }
                    }
                }
            },
        )));
    }
}
