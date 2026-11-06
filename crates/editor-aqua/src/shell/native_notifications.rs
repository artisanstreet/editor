use crate::core::Screen;
use crate::shell::host::Shell;
use crate::shell::notifications::parse_tag;
use crate::shell::workspace::NotificationEffects;
use aqua::AppCtx;
use aqua::prelude::{
    AppIdentity, AquaWindowId, NotificationCenter, NotificationError, NotificationResponse,
    TaskbarBadge, WindowRequest,
};
use std::{
    collections::VecDeque,
    num::NonZeroUsize,
    sync::{Arc, Mutex},
};

static COLD_LAUNCH: std::sync::OnceLock<NotificationResponse> = std::sync::OnceLock::new();

pub fn record_cold_launch(response: NotificationResponse) {
    let _ = COLD_LAUNCH.set(response);
}

pub struct NativeNotifications {
    center: NotificationCenter,
    wake: aqua::mailbox::Mailbox<()>,
    responses: Arc<Mutex<VecDeque<NotificationResponse>>>,
}

impl NativeNotifications {
    pub fn new() -> Result<Self, NotificationError> {
        let wake = aqua::mailbox::Mailbox::new(NonZeroUsize::MIN);
        let sender = wake.sender();
        let responses = Arc::new(Mutex::new(VecDeque::from_iter(COLD_LAUNCH.get().cloned())));
        let pending = Arc::clone(&responses);
        let center = NotificationCenter::new(
            AppIdentity::new("com.artisanstreet.editor", "Artisan Editor")?,
            Arc::new(move |response| {
                pending
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .push_back(response);
                let _ = sender.send(());
            }),
        )?;
        Ok(Self {
            center,
            wake,
            responses,
        })
    }

    pub fn has_pending(&self) -> bool {
        !self
            .responses
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_empty()
    }

    pub fn bind(&self, ctx: &AppCtx<'_>, window: AquaWindowId) {
        ctx.bind_mailbox(window, &self.wake);
    }

    pub fn apply(
        &mut self,
        effects: NotificationEffects,
        shell: &Shell,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) -> anyhow::Result<()> {
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
            let count = attention
                .waiting
                .saturating_add(attention.stopped)
                .min(u32::MAX as usize) as u32;
            ctx.request_window(WindowRequest::SetBadge(
                window,
                (count > 0).then(|| TaskbarBadge {
                    count,
                    color: color.to_paint(),
                }),
            ))?;
        }
        Ok(())
    }

    pub fn drain(
        &mut self,
        shell: &mut Shell,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) -> anyhow::Result<bool> {
        self.wake.drain();
        let responses = std::mem::take(
            &mut *self
                .responses
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        let changed = !responses.is_empty();
        for response in responses {
            if let Some((host, thread)) = parse_tag(&response.tag) {
                shell.navigate(Screen::Thread { host, thread });
            }
            ctx.request_window(WindowRequest::Activate(window))?;
        }
        Ok(changed)
    }
}
