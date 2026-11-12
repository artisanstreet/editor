use super::{ContentText, selection_source, source_slice};
use editor_core::RangeState;
use editor_ui::{
    button::Button,
    glass::{GlassStrength, glass_card_shadows, glass_layers},
    theme::{ArtisanTheme, Elevation, RadiusStep, RadiusTokens},
};
use aqua_text::document::{SelectionDocumentId, SelectionSource, SelectionSourceId};
use aqua_ui::{
    collection::List,
    element::{Box as UiBox, Element, Text},
    style::{Length, Style},
};
use std::{rc::Rc, sync::Arc};

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

pub fn content_view(
    view: &ContentText,
    theme: ArtisanTheme,
    document: SelectionDocumentId,
    source_id: SelectionSourceId,
    action: Rc<dyn Fn(ContentAction)>,
) -> ContentView {
    let mut body = UiBox::new()
        .layout("column")
        .gap(theme.spacing.steps(1.0))
        .width(Length::Pct(100.0));
    let mut selection = None;
    match view.window().state() {
        RangeState::Loading => {
            body = body.child(
                Text::new("Loading source…")
                    .gpui_source_color(theme.colors.text.to_reference_text_color())
                    .build(),
            )
        }
        RangeState::Failed(error) => {
            let callback = Rc::clone(&action);
            body = body
                .child(
                    Text::new(error.clone())
                        .gpui_source_color(theme.colors.text.to_reference_text_color())
                        .build(),
                )
                .child(
                    Button::text("source-retry", theme, "Retry")
                        .variant(editor_ui::button::ButtonVariant::Outline)
                        .on_activate(move |_| callback(ContentAction::Retry))
                        .build(),
                );
        }
        RangeState::Ready(page) => {
            let start = page.start;
            let end = page.end;
            let total = page.total;
            body = body.child(
                Text::new(format!(
                    "Raw source bytes {start}–{end} of {total}. Selection copies this loaded page."
                ))
                .gpui_source_color(theme.colors.text.to_reference_text_color())
                .build(),
            );
            let text: Arc<str> = Arc::from(page.text.as_str());
            selection = Some(selection_source(source_id, start, Arc::clone(&text)));
            let row_text = Arc::clone(&text);
            let list = List::new()
                .items(view.rows.iter().cloned().enumerate())
                .key(|(index, _)| format!("raw-source-row-{index}"))
                .row(move |(_, range)| {
                    Text::new(row_text[range.clone()].to_owned())
                        .gpui_source_color(theme.colors.text.to_reference_text_color())
                        .selection_source_in(document, source_slice(source_id, range.clone()))
                        .build()
                })
                .virtualize()
                .measured_uniform_rows()
                .style(
                    Style::new()
                        .height(Length::Px(320.0))
                        .width(Length::Pct(100.0)),
                )
                .build();
            let previous = Rc::clone(&action);
            let next = Rc::clone(&action);
            body = body.child(list).child(
                UiBox::new()
                    .gap(theme.spacing.steps(1.0))
                    .child(
                        Button::text("source-prev", theme, "Previous")
                            .variant(editor_ui::button::ButtonVariant::Ghost)
                            .disabled(start == 0)
                            .on_activate(move |_| previous(ContentAction::Previous))
                            .build(),
                    )
                    .child(
                        Button::text("source-next", theme, "Next")
                            .variant(editor_ui::button::ButtonVariant::Ghost)
                            .disabled(end == total)
                            .on_activate(move |_| next(ContentAction::Next))
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
            .gap(theme.spacing.steps(1.0))
            .child(
                Button::text("source-copy", theme, "Copy full raw source")
                    .variant(editor_ui::button::ButtonVariant::Outline)
                    .disabled(view.busy())
                    .on_activate(move |_| copy(ContentAction::Copy))
                    .build(),
            )
            .child(
                Button::text("source-export", theme, "Export source")
                    .variant(editor_ui::button::ButtonVariant::Outline)
                    .disabled(view.busy())
                    .on_activate(move |_| export(ContentAction::Export))
                    .build(),
            )
            .build(),
    );
    if let Some(error) = &view.error {
        body = body.child(
            Text::new(error.clone())
                .gpui_source_color(theme.colors.text.to_reference_text_color())
                .build(),
        );
    }
    ContentView {
        element: code_card(theme, body.build()),
        selection,
    }
}

pub(crate) fn code_card(theme: ArtisanTheme, content: Element) -> Element {
    let radius = RadiusTokens::value(RadiusStep::X2l);
    UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .radius(radius)
        .padding(theme.density.card_padding_compact)
        .style(
            Style::new()
                .min_width(Length::Px(0.0))
                .gpui_source_text_color(theme.colors.text.to_reference_text_color())
                .shadows(
                    glass_card_shadows(&theme, Elevation::Raised)
                        .into_iter()
                        .map(Into::into)
                        .collect(),
                ),
        )
        .child(glass_layers(&theme, GlassStrength::Quiet, radius))
        .child(content)
        .build()
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
