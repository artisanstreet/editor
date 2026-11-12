use super::{ContentText, selection_source, source_slice};
use editor_core::RangeState;
use editor_ui::kit::{Button, ButtonTone, Spaced, Space, Surface, TextRole};
use editor_ui::theme::ArtisanTheme;
use aqua_text::document::{SelectionDocumentId, SelectionSource, SelectionSourceId};
use aqua_ui::{
    collection::List,
    element::{Box as UiBox, Element},
    style::{Length, Style},
};
use std::{rc::Rc, sync::Arc};

const VISIBLE_ROWS: f32 = 12.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContentAction {
    Retry,
    Previous,
    Next,
    Copy,
    Export,
}

pub struct ContentView {
    pub element: Element,
    pub selection: Option<SelectionSource>,
}

pub(crate) fn viewport(row: f32, max_height: Option<f32>) -> f32 {
    let full = row * VISIBLE_ROWS;
    max_height.map_or(full, |height| height.clamp(row, full))
}

pub(crate) fn stack() -> UiBox {
    UiBox::new().layout("column").spaced(Space::S4)
}

pub(crate) fn button(
    key: &str,
    theme: ArtisanTheme,
    tone: ButtonTone,
    label: &str,
    action: impl Fn() + 'static,
) -> Button {
    Button::text(key.to_owned(), theme, tone, label)
        .within(Surface::Inset)
        .on_activate(move |_| action())
}

pub(crate) fn source_lines(
    key: &'static str,
    theme: ArtisanTheme,
    text: Arc<str>,
    rows: Vec<std::ops::Range<usize>>,
    height: f32,
    document: SelectionDocumentId,
    source_id: SelectionSourceId,
) -> Element {
    List::new()
        .items(rows.into_iter().enumerate())
        .key(move |(index, _)| format!("{key}-{index}"))
        .row(move |(_, range)| {
            TextRole::Code
                .text(&theme, text[range.clone()].to_owned())
                .selection_source_in(document, source_slice(source_id, range.clone()))
                .build()
        })
        .virtualize()
        .measured_uniform_rows()
        .style(
            Style::new()
                .height(Length::Px(height))
                .width(Length::Pct(100.0)),
        )
        .build()
}

pub fn content_view(
    view: &ContentText,
    theme: ArtisanTheme,
    document: SelectionDocumentId,
    source_id: SelectionSourceId,
    action: Rc<dyn Fn(ContentAction)>,
) -> ContentView {
    let mut body = stack().width(Length::Pct(100.0));
    let mut selection = None;
    match view.window().state() {
        RangeState::Loading => {
            body = body.child(TextRole::Label.text(&theme, "Loading source…").build())
        }
        RangeState::Failed(error) => {
            let callback = Rc::clone(&action);
            body = body
                .child(TextRole::Label.text(&theme, error.clone()).build())
                .child(
                    button("source-retry", theme, ButtonTone::Neutral, "Retry", move || {
                        callback(ContentAction::Retry)
                    })
                    .build(),
                );
        }
        RangeState::Ready(page) => {
            let start = page.start;
            let end = page.end;
            let total = page.total;
            body = body.child(
                TextRole::Caption
                    .text(
                        &theme,
                        format!(
                            "Raw source bytes {start}–{end} of {total}. Selection copies this loaded page."
                        ),
                    )
                    .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
                    .build(),
            );
            let text: Arc<str> = Arc::from(page.text.as_str());
            selection = Some(selection_source(source_id, start, Arc::clone(&text)));
            let list = source_lines(
                "raw-source-row",
                theme,
                text,
                view.rows.clone(),
                viewport(TextRole::Code.spec().line, None),
                document,
                source_id,
            );
            let previous = Rc::clone(&action);
            let next = Rc::clone(&action);
            body = body.child(list).child(
                UiBox::new()
                    .spaced(Space::S4)
                    .child(
                        button("source-prev", theme, ButtonTone::Quiet, "Previous", move || {
                            previous(ContentAction::Previous)
                        })
                        .disabled(start == 0)
                        .build(),
                    )
                    .child(
                        button("source-next", theme, ButtonTone::Quiet, "Next", move || {
                            next(ContentAction::Next)
                        })
                        .disabled(end == total)
                        .build(),
                    )
                    .build(),
            );
        }
    }
    let copy = Rc::clone(&action);
    let export = Rc::clone(&action);
    body = body.child(
        UiBox::new()
            .spaced(Space::S4)
            .child(
                button(
                    "source-copy",
                    theme,
                    ButtonTone::Neutral,
                    "Copy full raw source",
                    move || copy(ContentAction::Copy),
                )
                .disabled(view.busy())
                .build(),
            )
            .child(
                button("source-export", theme, ButtonTone::Neutral, "Export source", move || {
                    export(ContentAction::Export)
                })
                .disabled(view.busy())
                .build(),
            )
            .build(),
    );
    if let Some(error) = &view.error {
        body = body.child(TextRole::Label.text(&theme, error.clone()).build());
    }
    ContentView {
        element: code_card(theme, body.build()),
        selection,
    }
}

pub(crate) fn code_card(theme: ArtisanTheme, content: Element) -> Element {
    Surface::Card
        .build(&theme, vec![Surface::Inset.build(&theme, vec![content])])
        .style(
            Style::new()
                .width(Length::Pct(100.0))
                .min_width(Length::Pct(0.0)),
        )
}

pub enum ContentEffect {
    SavePath(u64),
    Clipboard(super::CopiedSource),
}

pub fn dispatch_content(
    view: &mut ContentText,
    core: &editor_core::Core,
    action: ContentAction,
) -> Option<ContentEffect> {
    match action {
        ContentAction::Retry => view.retry(core),
        ContentAction::Previous => view.previous(core),
        ContentAction::Next => view.next(core),
        ContentAction::Copy => view.copy(core),
        ContentAction::Export => {
            if let Some(generation) = view.begin_export(core) {
                return Some(ContentEffect::SavePath(generation));
            }
        }
    }
    None
}

pub fn drain_content(view: &mut ContentText, core: &editor_core::Core) -> Option<ContentEffect> {
    view.drain(core).map(ContentEffect::Clipboard)
}

pub fn dispatch_content_changed(
    view: &mut ContentText,
    core: &editor_core::Core,
    action: ContentAction,
) -> (bool, Option<ContentEffect>) {
    let before = view.view_state();
    let effect = dispatch_content(view, core, action);
    (view.view_state() != before, effect)
}

pub fn drain_content_changed(
    view: &mut ContentText,
    core: &editor_core::Core,
) -> (bool, Option<ContentEffect>) {
    let before = view.view_state();
    let effect = drain_content(view, core);
    (view.view_state() != before, effect)
}
