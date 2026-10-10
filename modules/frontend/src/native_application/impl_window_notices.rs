//! The window's notice stack: showing notices, retiring them when their
//! lifetime ends, and pausing them while hovered (see [`crate::window_notice`]).

use super::*;

impl NativeApplication {
    /// Puts a notice at the front of the window's stack.
    pub(super) fn show_notice(
        &mut self,
        severity: crate::window_notice::NoticeSeverity,
        message: String,
        cx: &mut Context<Self>,
    ) {
        self.window_notices
            .push(severity, message, std::time::Instant::now());
        self.schedule_window_notice_timer(cx);
        cx.notify();
    }

    /// Retires expired notices and wakes again when the next lifetime ends.
    fn schedule_window_notice_timer(&mut self, cx: &mut Context<Self>) {
        let Some(wait) = self.window_notices.tick(std::time::Instant::now()) else {
            self.window_notice_timer = None;
            return;
        };
        self.window_notice_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |app, cx| {
                app.schedule_window_notice_timer(cx);
                cx.notify();
            });
        }));
    }

    /// The notice stack below the titlebar's right end.
    pub(super) fn window_notice_stack(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let now = std::time::Instant::now();
        self.window_notices.tick(now);
        let entity = cx.entity().downgrade();
        let hover_entity = entity.clone();
        let (stack, moving) = self.window_notices.render(
            ArtisanTheme::for_mode(ThemeMode::Dark),
            now,
            move |id| {
                let entity = entity.clone();
                Box::new(move |_, _, cx| {
                    let _ = entity.update(cx, |app, cx| {
                        app.window_notices.dismiss(id, std::time::Instant::now());
                        cx.notify();
                    });
                })
            },
            move |hovered, _, cx| {
                let hovered = *hovered;
                let _ = hover_entity.update(cx, |app, cx| {
                    app.window_notices
                        .set_hovered(hovered, std::time::Instant::now());
                    app.schedule_window_notice_timer(cx);
                    cx.notify();
                });
            },
        )?;
        if moving {
            window.request_animation_frame();
        }
        Some(
            div()
                .absolute()
                .top(px(56.0))
                .right(px(16.0))
                .child(stack)
                .into_any_element(),
        )
    }
}
