//! Failure cards for messages the Forge could not deliver.
//!
//! Each card states the verbatim dispatcher reason and offers the Forge's
//! actions on its stored payload: `Retry` (only when the Forge reports the
//! failure retryable) dispatches it again, and `Start new chat` moves the
//! prompt into a new thread as an unsent draft. Both name the failure by
//! identity; no prompt text leaves this view.

use super::*;

/// Which failure-card action a focus handle belongs to.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum FailedAction {
    Retry,
    NewChat,
}

impl NativeComposerControls {
    /// Renders one failure card for the newest failed dispatch.
    #[must_use]
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI builder composes the failure card, its copy, and both actions in order"
    )]
    pub fn render_failed_dispatches(
        &mut self,
        theme: ArtisanTheme,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        let rows = self.snapshot.failed_dispatches.clone();
        if rows.is_empty() {
            return None;
        }
        let entity = cx.entity();
        let desktop_theme = DesktopTheme::neutral_dark();
        let mut cards = div()
            .id(ElementId::Name(
                "artisan-native-composer-failed-dispatches".into(),
            ))
            .debug_selector(|| "artisan-native-composer-failed-dispatches".to_owned())
            .w_full()
            .flex()
            .flex_col()
            .gap(px(8.0));
        for (index, row) in rows.into_iter().take(1).enumerate() {
            let cancelled = row.reason == "run cancelled";
            let row_index = isize::try_from(index).unwrap_or(isize::MAX);
            let tab_index = 30isize.saturating_add(row_index.saturating_mul(2));
            let command_id = row.command_id().to_owned();
            let generation = row.generation();
            let mut actions = div().flex().flex_row().items_center().gap(px(4.0));
            if row.retryable {
                let focus = self.failed_focus(&row.identity, FailedAction::Retry, tab_index, cx);
                let selector = format!(
                    "{NATIVE_COMPOSER_FAILED_RETRY_SELECTOR}-{}",
                    row.command_id()
                );
                let retry_entity = entity.clone();
                let retry_command = command_id.clone();
                if let Ok(retry) = Button::new(
                    ElementId::Name(selector.clone().into()),
                    focus,
                    theme,
                    MotionPolicy::Reduced,
                    ButtonVariant::Ghost,
                    ButtonSize::Small,
                    ButtonContent::text("Retry"),
                ) {
                    actions = actions.child(
                        retry
                            .focus_visibility(FocusVisibility::Visible)
                            .disabled(
                                self.snapshot.disabled || !self.snapshot.failed_new_chat_ready,
                            )
                            .debug_selector(selector)
                            .on_activate(move |_, _, app| {
                                retry_entity.update(app, |controls, controls_cx| {
                                    controls.emit_if_allowed(
                                        NativeComposerControlsEvent::RetryFailedDispatch {
                                            command_id: retry_command.clone(),
                                            generation,
                                        },
                                        controls_cx,
                                    );
                                });
                            }),
                    );
                }
            }
            let focus = self.failed_focus(
                &row.identity,
                FailedAction::NewChat,
                tab_index.saturating_add(1),
                cx,
            );
            let selector = format!(
                "{NATIVE_COMPOSER_FAILED_NEW_THREAD_SELECTOR}-{}",
                row.command_id()
            );
            let action_entity = entity.clone();
            if let Ok(new_chat) = Button::new(
                ElementId::Name(selector.clone().into()),
                focus,
                theme,
                MotionPolicy::Reduced,
                ButtonVariant::Ghost,
                ButtonSize::Small,
                ButtonContent::text("Start new chat"),
            ) {
                actions = actions.child(
                    new_chat
                        .focus_visibility(FocusVisibility::Visible)
                        .disabled(self.snapshot.disabled || !self.snapshot.failed_new_chat_ready)
                        .debug_selector(selector)
                        .on_activate(move |_, _, app| {
                            action_entity.update(app, |controls, controls_cx| {
                                controls.emit_if_allowed(
                                    NativeComposerControlsEvent::StartNewThreadWithFailedPrompt {
                                        command_id: command_id.clone(),
                                        generation,
                                    },
                                    controls_cx,
                                );
                            });
                        }),
                );
            }
            let mut body = div()
                .min_w(px(0.0))
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(theme.typography.control_text)
                        .text_color(desktop_theme.foreground)
                        .child(if cancelled {
                            "Message not sent"
                        } else {
                            "Send failed"
                        }),
                )
                .child(
                    div()
                        .text_size(theme.typography.control_text)
                        .text_color(desktop_theme.secondary)
                        .child(if cancelled {
                            "The run was stopped before this queued message was sent.".into()
                        } else if row.reason == "OpenCode2 provider turn interrupted" {
                            "The provider session was interrupted.".into()
                        } else {
                            row.reason.clone()
                        }),
                );
            if !row.text.is_empty() {
                body = body.child(
                    div()
                        .text_size(theme.typography.control_text)
                        .text_color(desktop_theme.secondary)
                        .child(row.text.clone()),
                );
            }
            if row.has_attachments {
                body = body.child(
                    div()
                        .text_size(theme.typography.label_text)
                        .text_color(desktop_theme.secondary)
                        .child("Images move with the prompt."),
                );
            }
            cards = cards.child(
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(px(12.0))
                    .rounded(px(14.0))
                    .border_1()
                    .border_color(if cancelled {
                        desktop_theme.secondary
                    } else {
                        theme.colors.destructive.with_alpha(0.4).to_paint()
                    })
                    .bg(desktop_theme.field)
                    .px(px(16.0))
                    .py(px(12.0))
                    .child(body)
                    .child(actions),
            );
        }
        Some(cards)
    }

    fn failed_focus(
        &mut self,
        identity: &QueuedSteeringIdentity,
        action: FailedAction,
        tab_index: isize,
        cx: &mut Context<Self>,
    ) -> FocusHandle {
        let key = (identity.clone(), action);
        let focus = self
            .failed_focus
            .get(&key)
            .cloned()
            .unwrap_or_else(|| cx.focus_handle())
            .tab_index(tab_index)
            .tab_stop(!self.snapshot.disabled);
        self.failed_focus.insert(key, focus.clone());
        focus
    }
}
