use super::{ComposerAction, ComposerOwner, Dispatch};
use crate::composer::model_switch::{Compaction, SwitchChoice, SwitchPrompt};
use aqua_ui::{
    JustifyContent,
    element::{Box as UiBox, Element},
    style::Style,
};
use editor_ui::{
    dialog::{Dialog, DialogHost},
    kit::{Button, ButtonTone, Space, Spaced, TextRole},
    theme::ArtisanTheme,
};
use std::rc::Rc;

impl ComposerOwner {
    pub fn sync_switch_dialog(&mut self, app: &mut aqua::UiApp, theme: &ArtisanTheme) {
        let prompt = self.controller.borrow().switch_prompt.clone();
        if let Some((shown, host)) = &self.switch_dialog {
            host.sync(app);
            if prompt.as_ref() == Some(shown) && host.is_open(app) {
                return;
            }
        }
        self.dispose_switch_dialog(app);
        let Some(prompt) = prompt else {
            return;
        };
        let Some(owner) = app.find_debug_selector("workspace-root") else {
            return;
        };
        let cancel = Rc::clone(&self.dispatch);
        let dialog = Dialog::new(
            "model-switch",
            *theme,
            format!("Switch to {}?", prompt.to),
            content(theme, &prompt, &self.dispatch),
        )
        .max_width(480.0)
        .on_dismiss(move |_| cancel(ComposerAction::ResolveSwitch(SwitchChoice::Cancel)));
        match DialogHost::open(app, owner, dialog) {
            Ok(host) => self.switch_dialog = Some((prompt, host)),
            Err(error) => {
                tracing::warn!(error = %diagnostics::chain(&error), "could not show the model switch warning");
            }
        }
    }

    pub fn sync_switch_dialog_input(&self, app: &mut aqua::UiApp) {
        if let Some((_, host)) = &self.switch_dialog {
            host.sync(app);
        }
    }

    pub fn dispose_switch_dialog(&mut self, app: &mut aqua::UiApp) {
        if let Some((_, host)) = self.switch_dialog.take() {
            host.dispose(app);
        }
    }
}

fn content(theme: &ArtisanTheme, prompt: &SwitchPrompt, dispatch: &Dispatch) -> Element {
    let choice = |key: &'static str, tone: ButtonTone, label: &'static str, choice| {
        let dispatch = Rc::clone(dispatch);
        Button::text(key, *theme, tone, label)
            .on_activate(move |_| dispatch(ComposerAction::ResolveSwitch(choice)))
            .build()
    };
    let compaction = match prompt.compaction {
        Compaction::Available => format!(
            "Compact first summarizes the thread on {} while its cache is still warm, then switches.",
            prompt.from
        ),
        Compaction::BusyRun => {
            "Compacting becomes available once the current run finishes.".to_owned()
        }
    };
    let mut buttons = UiBox::new()
        .spaced(Space::S8)
        .style(Style::new().justify(JustifyContent::END))
        .child(choice(
            "model-switch-cancel",
            ButtonTone::Quiet,
            "Cancel",
            SwitchChoice::Cancel,
        ))
        .child(choice(
            "model-switch-anyway",
            ButtonTone::Neutral,
            "Switch anyway",
            SwitchChoice::SwitchAnyway,
        ));
    if prompt.compaction == Compaction::Available {
        buttons = buttons.child(choice(
            "model-switch-compact-first",
            ButtonTone::Primary,
            "Compact first",
            SwitchChoice::CompactFirst,
        ));
    }
    UiBox::new()
        .layout("column")
        .spaced(Space::S12)
        .child(
            TextRole::Prose
                .text(
                    theme,
                    format!(
                        "This thread holds {} of context. {} can't reuse {}'s cache, so your next message sends all of it again uncached. That's slow, and can use up a usage limit in one go.",
                        tokens(prompt.context_tokens),
                        prompt.to,
                        prompt.from
                    ),
                )
                .wrap(true)
                .build(),
        )
        .child(TextRole::Prose.text(theme, compaction).wrap(true).build())
        .child(buttons.build())
        .build()
}

fn tokens(count: u64) -> String {
    match count {
        n if n >= 1_000_000 => format!("{}.{}M tokens", n / 1_000_000, n % 1_000_000 / 100_000),
        n => format!("{}K tokens", (n + 500) / 1_000),
    }
}
