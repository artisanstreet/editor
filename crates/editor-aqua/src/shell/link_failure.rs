use crate::ui::dialog::{Dialog, DialogHost};
use crate::ui::kit::{Button, ButtonTone, Space, Spaced, TextRole};
use crate::ui::theme::ArtisanTheme;
use aqua::UiApp;
use aqua_ui::element::{Box as UiBox, Element};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};

pub const COPY: &str = "link-failure-copy";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkFailure {
    url: String,
    cause: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Event {
    Copy,
    Dismiss,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    CopyUrl(String),
}

#[derive(Default)]
pub struct LinkFailureDialog {
    failure: Option<LinkFailure>,
    host: Option<DialogHost>,
    events: Rc<RefCell<VecDeque<Event>>>,
}

impl std::fmt::Debug for LinkFailureDialog {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LinkFailureDialog")
            .finish_non_exhaustive()
    }
}

impl LinkFailureDialog {
    pub fn show(&mut self, url: String, error: &dyn std::error::Error) {
        self.failure = Some(LinkFailure {
            url,
            cause: ::diagnostics::chain(error),
        });
    }

    #[must_use]
    pub const fn failure(&self) -> Option<&LinkFailure> {
        self.failure.as_ref()
    }

    pub fn drain(&mut self, ui: &mut UiApp) -> Option<Effect> {
        let events = std::mem::take(&mut *self.events.borrow_mut());
        let mut effect = None;
        for event in events {
            match (event, self.failure.take()) {
                (Event::Copy, Some(failure)) => effect = Some(Effect::CopyUrl(failure.url)),
                (Event::Dismiss, Some(_)) | (_, None) => {}
            }
        }
        if self.failure.is_none() {
            self.close(ui);
        }
        effect
    }

    pub(crate) fn close(&mut self, ui: &mut UiApp) {
        self.failure = None;
        if let Some(host) = self.host.take() {
            host.dispose(ui);
        }
        self.events.borrow_mut().clear();
    }

    pub fn mount(&mut self, ui: &mut UiApp, theme: &ArtisanTheme) {
        let Some(failure) = &self.failure else {
            return;
        };
        if self.host.as_ref().is_some_and(|host| host.is_open(ui)) {
            return;
        }
        let Some(owner) = ui.find_debug_selector("workspace-root") else {
            return;
        };
        let dismiss = Rc::clone(&self.events);
        let dialog = Dialog::new(
            "link-failure",
            *theme,
            "Couldn't open the link",
            content(theme, failure, Rc::clone(&self.events)),
        )
        .max_width(480.0)
        .on_dismiss(move |_| dismiss.borrow_mut().push_back(Event::Dismiss));
        match DialogHost::open(ui, owner, dialog) {
            Ok(host) => self.host = Some(host),
            Err(error) => tracing::warn!(%error, "could not show the link failure"),
        }
    }
}

fn content(
    theme: &ArtisanTheme,
    failure: &LinkFailure,
    events: Rc<RefCell<VecDeque<Event>>>,
) -> Element {
    UiBox::new()
        .layout("column")
        .spaced(Space::S8)
        .child(
            TextRole::Label
                .text(
                    &theme,
                    format!("The browser didn't start: {}.", failure.cause),
                )
                .wrap(true)
                .build(),
        )
        .child(
            TextRole::Code
                .text(&theme, failure.url.clone())
                .wrap(true)
                .build(),
        )
        .child(
            Button::text(COPY, *theme, ButtonTone::Primary, "Copy link")
                .on_activate(move |_| events.borrow_mut().push_back(Event::Copy))
                .build(),
        )
        .build()
}
