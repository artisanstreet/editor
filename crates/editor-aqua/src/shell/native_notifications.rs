use crate::core::Screen;
use crate::shell::host::Shell;
use crate::shell::notifications::parse_tag;
use crate::shell::workspace::NotificationEffects;
use aqua::AppCtx;
#[cfg(windows)]
use aqua::prelude::NotificationCenter;
use aqua::prelude::{
    AppIdentity, AquaWindowId, NotificationError, NotificationResponse, TaskbarBadge, WindowRequest,
};
use std::{
    collections::VecDeque,
    num::NonZeroUsize,
    sync::{Arc, Mutex},
};

#[cfg(not(windows))]
struct NotificationCenter;

#[cfg(not(windows))]
impl NotificationCenter {
    fn new(
        _identity: AppIdentity,
        _respond: Arc<dyn Fn(NotificationResponse) + Send + Sync>,
    ) -> Result<Self, NotificationError> {
        Err(NotificationError::Unsupported)
    }

    #[expect(
        clippy::unused_self,
        reason = "the non-Windows stub keeps the Windows notification center's method signatures"
    )]
    fn withdraw(&self, _tag: &str) -> Result<(), NotificationError> {
        Err(NotificationError::Unsupported)
    }

    #[expect(
        clippy::unused_self,
        reason = "the non-Windows stub keeps the Windows notification center's method signatures"
    )]
    fn show(&self, _notification: aqua::prelude::Notification) -> Result<(), NotificationError> {
        Err(NotificationError::Unsupported)
    }
}

static COLD_LAUNCH: std::sync::OnceLock<NotificationResponse> = std::sync::OnceLock::new();

pub fn record_cold_launch(response: NotificationResponse) {
    assert!(
        COLD_LAUNCH.set(response).is_ok(),
        "invariant: the cold launch response is recorded once per process"
    );
}

#[derive(Debug, thiserror::Error)]
#[error("show the attention count {count} on the taskbar")]
pub struct BadgeError {
    count: u32,
    #[source]
    source: aqua_protocol::window::WindowError,
}

#[derive(Debug, thiserror::Error)]
#[error("bring the Editor forward for a notification")]
pub struct ActivateError(#[source] aqua_protocol::window::WindowError);

pub struct NativeNotifications {
    center: NotificationCenter,
    wake: aqua::mailbox::Mailbox<()>,
    responses: Arc<Mutex<VecDeque<NotificationResponse>>>,
}

impl std::fmt::Debug for NativeNotifications {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativeNotifications")
            .finish_non_exhaustive()
    }
}

impl NativeNotifications {
    pub(crate) fn new() -> Result<Self, NotificationError> {
        let wake = aqua::mailbox::Mailbox::new(NonZeroUsize::MIN);
        let sender = wake.sender();
        let responses = Arc::new(Mutex::new(VecDeque::from_iter(COLD_LAUNCH.get().cloned())));
        let pending = Arc::clone(&responses);
        let center = NotificationCenter::new(
            AppIdentity::new("com.artisanstreet.editor", "Artisan Editor")?,
            Arc::new(move |response| {
                pending
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push_back(response);
                match sender.send(()) {
                    Ok(()) | Err(aqua::mailbox::SendError::Full(())) => {}
                    Err(aqua::mailbox::SendError::Closed(())) => {
                        tracing::debug!(
                            "a notification answer arrived after the Editor closed its notifications"
                        );
                    }
                }
            }),
        )?;
        Ok(Self {
            center,
            wake,
            responses,
        })
    }

    pub(crate) fn has_pending(&self) -> bool {
        !self
            .responses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
    }

    pub(crate) fn bind(&self, ctx: &AppCtx<'_>, window: AquaWindowId) {
        ctx.bind_mailbox(window, &self.wake);
    }

    pub(crate) fn apply(
        &self,
        effects: NotificationEffects,
        shell: &Shell,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) -> Result<(), BadgeError> {
        for tag in effects.withdraw {
            if let Err(error) = self.center.withdraw(&tag) {
                tracing::warn!(%error, "could not withdraw notification");
            }
        }
        for notification in effects.show {
            if let Err(error) = self.center.show(notification) {
                tracing::warn!(%error, "could not show notification");
            }
        }
        if let Some(attention) = effects.badge {
            let theme = shell.preferences.theme();
            let color = if attention.waiting > 0 {
                theme.colors.needs_answer
            } else {
                theme.colors.red.solid
            };
            let count = u32::try_from(attention.waiting.saturating_add(attention.stopped))
                .unwrap_or(u32::MAX);
            ctx.request_window(WindowRequest::SetBadge(
                window,
                (count > 0).then(|| TaskbarBadge {
                    count,
                    color: color.to_paint(),
                }),
            ))
            .map_err(|source| BadgeError { count, source })?;
        }
        Ok(())
    }

    pub(crate) fn drain(
        &mut self,
        shell: &mut Shell,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) -> Result<bool, ActivateError> {
        self.wake.drain();
        let responses = std::mem::take(
            &mut *self
                .responses
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        let changed = !responses.is_empty();
        for response in responses {
            if let Some(screen) = activation(shell, &response.tag) {
                shell.navigate(screen);
            }
            ctx.request_window(WindowRequest::Activate(window))
                .map_err(ActivateError)?;
        }
        Ok(changed)
    }
}

#[must_use]
pub fn activation(shell: &Shell, tag: &str) -> Option<Screen> {
    match parse_tag(tag) {
        Some((host, thread))
            if shell.entry.workspace().is_some() && shell.core.host(&host).is_some() =>
        {
            Some(Screen::Thread { host, thread })
        }
        Some((host, thread)) => {
            tracing::info!(
                %host,
                thread = %thread.0,
                "ignored a notification from a signed-out session or a removed Forge"
            );
            None
        }
        None => {
            tracing::warn!("ignored a notification with an unreadable tag");
            None
        }
    }
}
