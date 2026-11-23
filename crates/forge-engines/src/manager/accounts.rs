use super::Engines;
use crate::{Customisation, Launch};
use artisan_api::{EngineId, McpServer, SignInState, UsageLimits};
use std::time::Duration;
use tokio::sync::broadcast;

#[derive(Debug, thiserror::Error)]
#[error("{} is not installed", engine.display_name())]
pub struct NotInstalled {
    pub engine: EngineId,
}

pub(super) const USAGE_TTL: Duration = Duration::from_mins(5);

impl Engines {
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
            tracing::info!(%engine, error = diagnostics::chain(&error), "couldn't list models");
        }
        self.refresh_usage(engine, true).await;
    }

    pub async fn keep_usage_fresh(&self) {
        let mut changes = self.subscribe();
        loop {
            let reading: Vec<EngineId> = EngineId::ALL
                .into_iter()
                .filter(|engine| crate::has_usage_reader(*engine) && self.binary(*engine).is_some())
                .collect();
            let next = reading
                .iter()
                .map(|engine| self.inner.usage.due_at(*engine))
                .min();
            tokio::select! {
                () = until(next) => {}
                change = changes.recv() => match change {
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => {
                        unreachable!("invariant: engine statuses outlive the usage refresher that holds the engines")
                    }
                },
            }
            let now = tokio::time::Instant::now();
            for engine in reading {
                if self.inner.usage.due_at(engine) <= now {
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

    pub fn login(&self, engine: EngineId) -> Result<(), NotInstalled> {
        let lease = self.lease(engine).ok_or(NotInstalled { engine })?;
        self.follow_sign_in(engine);
        self.invalidate_models(engine);
        self.slot(engine).sign_in.login_leased(lease);
        Ok(())
    }

    pub fn login_codex_api_key(&self, key: artisan_api::Secret) -> Result<(), NotInstalled> {
        let engine = EngineId::Codex;
        let lease = self.lease(engine).ok_or(NotInstalled { engine })?;
        self.follow_sign_in(engine);
        self.invalidate_models(engine);
        self.slot(engine).sign_in.api_key_leased(lease, key);
        Ok(())
    }

    pub async fn submit_login_code(
        &self,
        engine: EngineId,
        code: &str,
    ) -> Result<(), crate::account::SubmitCodeError> {
        self.slot(engine).sign_in.submit_code(code).await
    }

    pub fn cancel_login(&self, engine: EngineId) {
        self.follow_sign_in(engine);
        self.slot(engine).sign_in.cancel_login();
    }

    pub fn logout(&self, engine: EngineId) -> Result<(), NotInstalled> {
        let lease = self.lease(engine).ok_or(NotInstalled { engine })?;
        self.follow_sign_in(engine);
        self.invalidate_models(engine);
        self.slot(engine).sign_in.logout_leased(lease);
        self.inner.usage.invalidate(engine);
        Ok(())
    }

    pub async fn refresh_usage(&self, engine: EngineId, force: bool) -> Option<UsageLimits> {
        let Some(lease) = self.lease(engine) else {
            self.inner.usage.unreadable(engine);
            return None;
        };
        let usage = self
            .inner
            .usage
            .read(engine, lease.path(), &self.home(engine), force)
            .await;
        self.publish(engine);
        usage
    }

    pub fn materialize(
        &self,
        engine: EngineId,
        customisation: &Customisation,
        offered: impl FnOnce(&[McpServer]) -> Vec<McpServer>,
    ) -> Result<Launch, crate::MaterializeError> {
        crate::materialize(engine, &self.home(engine), customisation, offered)
    }

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
                        if matches!(state, SignInState::SignedIn { .. })
                            || (engine == EngineId::OpenCode
                                && matches!(state, SignInState::SignedOut))
                        {
                            if let Err(error) = engines.refresh_models(engine).await {
                                tracing::info!(%engine, error = diagnostics::chain(&error), "couldn't refresh models after sign-in");
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

async fn until(due: Option<tokio::time::Instant>) {
    match due {
        Some(due) => tokio::time::sleep_until(due).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
#[cfg(unix)]
#[path = "../../tests/manager/accounts.rs"]
mod tests;
