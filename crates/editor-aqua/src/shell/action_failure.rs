use crate::core::Problem;
use crate::ui::dialog::{Dialog, DialogHost};
use crate::ui::kit::{Button, ButtonTone, ProblemNotice, Space, Spaced, TextRole};
use crate::ui::theme::ArtisanTheme;
use aqua::UiApp;
use aqua_ui::element::{Box as UiBox, Element};
use std::{cell::RefCell, collections::VecDeque, rc::Rc};

pub const COPY: &str = "link-failure-copy";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopySubject {
    Code,
    Content,
    Color,
    MediaPath,
    ResourcePath,
    File,
    Link,
    ShellOutput,
    Text,
}

impl CopySubject {
    const fn noun(self) -> &'static str {
        match self {
            Self::Code => "code",
            Self::Content => "content",
            Self::Color => "color",
            Self::MediaPath => "media path",
            Self::ResourcePath => "file path",
            Self::File => "file",
            Self::Link => "link",
            Self::ShellOutput => "shell output",
            Self::Text => "text",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActionFailure {
    Link {
        url: String,
        problem: Problem,
    },
    Copy {
        subject: CopySubject,
        problem: Problem,
    },
    Search {
        problem: Problem,
    },
    Notices {
        problem: Problem,
    },
}

impl ActionFailure {
    const fn title(&self) -> &'static str {
        match self {
            Self::Link { .. } => "Couldn't open the link",
            Self::Copy { .. } => "Couldn't copy to the clipboard",
            Self::Search { .. } => "Couldn't search in the browser",
            Self::Notices { .. } => "Couldn't open the third-party notices",
        }
    }

    #[must_use]
    pub const fn problem(&self) -> &Problem {
        match self {
            Self::Link { problem, .. }
            | Self::Copy { problem, .. }
            | Self::Search { problem }
            | Self::Notices { problem } => problem,
        }
    }
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

struct Shown {
    host: DialogHost,
    failure: ActionFailure,
}

#[derive(Default)]
pub struct ActionFailureDialog {
    failure: Option<ActionFailure>,
    shown: Option<Shown>,
    events: Rc<RefCell<VecDeque<Event>>>,
}

impl std::fmt::Debug for ActionFailureDialog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ActionFailureDialog")
            .field("failure", &self.failure)
            .finish_non_exhaustive()
    }
}

impl ActionFailureDialog {
    pub fn link(&mut self, url: String, error: &dyn std::error::Error) {
        let problem = Problem::caused_by("The browser didn't start.", error);
        tracing::warn!(error = problem.detail(), "could not open URL");
        self.failure = Some(ActionFailure::Link { url, problem });
    }

    pub fn copy(&mut self, subject: CopySubject, error: &dyn std::error::Error) {
        let problem = Problem::caused_by(format!("The {} wasn't copied.", subject.noun()), error);
        tracing::warn!(
            error = problem.detail(),
            what = subject.noun(),
            "could not copy to the clipboard"
        );
        self.failure = Some(ActionFailure::Copy { subject, problem });
    }

    pub(crate) fn search(&mut self, error: &dyn std::error::Error) {
        let problem = Problem::caused_by("The browser didn't start.", error);
        tracing::warn!(error = problem.detail(), "could not search in the browser");
        self.failure = Some(ActionFailure::Search { problem });
    }

    pub(crate) fn notices(&mut self, error: &dyn std::error::Error) {
        let problem = Problem::caused_by("The notices file didn't open.", error);
        tracing::warn!(
            error = problem.detail(),
            "could not open the third-party notices"
        );
        self.failure = Some(ActionFailure::Notices { problem });
    }

    #[must_use]
    pub const fn failure(&self) -> Option<&ActionFailure> {
        self.failure.as_ref()
    }

    pub fn drain(&mut self, ui: &mut UiApp) -> Option<Effect> {
        let events = std::mem::take(&mut *self.events.borrow_mut());
        let mut effect = None;
        for event in events {
            match (event, self.failure.take()) {
                (Event::Copy, Some(ActionFailure::Link { url, .. })) => {
                    effect = Some(Effect::CopyUrl(url));
                }
                (
                    Event::Copy,
                    Some(
                        ActionFailure::Copy { .. }
                        | ActionFailure::Search { .. }
                        | ActionFailure::Notices { .. },
                    ),
                )
                | (Event::Dismiss, Some(_))
                | (_, None) => {}
            }
        }
        if self.failure.is_none() {
            self.close(ui);
        }
        effect
    }

    pub(crate) fn close(&mut self, ui: &mut UiApp) {
        self.failure = None;
        self.dispose(ui);
        self.events.borrow_mut().clear();
    }

    fn dispose(&mut self, ui: &mut UiApp) {
        if let Some(shown) = self.shown.take() {
            shown.host.dispose(ui);
        }
    }

    pub fn mount(&mut self, ui: &mut UiApp, theme: &ArtisanTheme) {
        let Some(failure) = &self.failure else {
            return;
        };
        if self
            .shown
            .as_ref()
            .is_some_and(|shown| shown.host.is_open(ui) && shown.failure == *failure)
        {
            return;
        }
        let failure = failure.clone();
        self.dispose(ui);
        let Some(owner) = ui.find_debug_selector("workspace-root") else {
            return;
        };
        let dismiss = Rc::clone(&self.events);
        let dialog = Dialog::new(
            "action-failure",
            *theme,
            failure.title(),
            content(theme, &failure, Rc::clone(&self.events)),
        )
        .max_width(480.0)
        .on_dismiss(move |_| dismiss.borrow_mut().push_back(Event::Dismiss));
        match DialogHost::open(ui, owner, dialog) {
            Ok(host) => self.shown = Some(Shown { host, failure }),
            Err(error) => tracing::warn!(%error, "could not show the action failure"),
        }
    }
}

fn content(
    theme: &ArtisanTheme,
    failure: &ActionFailure,
    events: Rc<RefCell<VecDeque<Event>>>,
) -> Element {
    let column = UiBox::new()
        .layout("column")
        .spaced(Space::S8)
        .child(ProblemNotice::new("action-failure-problem", *theme, failure.problem()).build());
    match failure {
        ActionFailure::Link { url, .. } => column
            .child(TextRole::Code.text(theme, url.clone()).wrap(true).build())
            .child(
                Button::text(COPY, *theme, ButtonTone::Primary, "Copy link")
                    .on_activate(move |_| events.borrow_mut().push_back(Event::Copy))
                    .build(),
            )
            .build(),
        ActionFailure::Copy { .. }
        | ActionFailure::Search { .. }
        | ActionFailure::Notices { .. } => column.build(),
    }
}
