use std::{rc::Rc, sync::Arc};

use aqua_text::document::{
    SelectionDocumentId, SelectionSource, SelectionSourceError, SelectionSourceId, SourceSlice,
};
use aqua_ui::{
    collection::List,
    element::{Box as UiBox, Element},
    style::{Length, Style},
};

use super::{LineWindow, PreviewError, ResourcePreview};
use crate::ui::{
    dialog::Dialog,
    kit::{Button, ButtonTone, Hover, Space, Spaced, Surface, TextRole},
    theme::ArtisanTheme,
};

const WELL: Surface = Surface::Inset;
const LINE: f32 = TextRole::Code.spec().line;
const GUTTER: f32 = 64.0;
const CHROME: f32 = 180.0;
const MIN_HEIGHT: f32 = 120.0;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    CopyPath { generation: u64, text: String },
    CopyFile { generation: u64, text: Arc<str> },
    Dismiss { generation: u64 },
}

struct Prepared {
    window: LineWindow,
    target_line: usize,
}

#[derive(Debug, thiserror::Error)]
#[error("the preview of {path} was shown before its lines were registered for selection")]
pub struct UnpreparedError {
    path: String,
}

pub struct ViewState {
    generation: u64,
    document: SelectionDocumentId,
    source: SelectionSourceId,
    prepared: Option<Arc<Prepared>>,
    jump: Option<usize>,
}

impl ViewState {
    pub fn new(generation: u64, document: SelectionDocumentId, source: SelectionSourceId) -> Self {
        Self {
            generation,
            document,
            source,
            prepared: None,
            jump: None,
        }
    }

    pub fn prepare(
        &mut self,
        preview: &ResourcePreview,
        app: &mut aqua::UiApp,
    ) -> Result<bool, SelectionSourceError> {
        let Some(Ok(content)) = &preview.content else {
            return Ok(false);
        };
        let Some(window) = content.window() else {
            return Ok(false);
        };
        if self.prepared.is_some() {
            return Ok(false);
        }
        app.register_selection_source_in(
            self.document,
            SelectionSource {
                id: self.source,
                order: vec![0],
                text: Arc::clone(window.text()),
                separator: Arc::from(""),
                copies: Vec::new(),
            },
        )?;
        self.jump = Some(
            preview
                .target_line
                .saturating_sub(window.start_line())
                .min(window.len() - 1),
        );
        self.prepared = Some(Arc::new(Prepared {
            window: window.clone(),
            target_line: preview.target_line,
        }));
        Ok(true)
    }

    pub fn pending_jump(&self) -> Option<usize> {
        self.jump
    }

    pub fn sync_jump(&mut self, app: &mut aqua::UiApp) -> bool {
        let Some(index) = self.jump else {
            return false;
        };
        if !app.reveal_collection_item_aligned(
            "resource-lines",
            index,
            aqua_ui::collection::CollectionRevealAlignment::Start,
        ) {
            return false;
        }
        self.jump = None;
        true
    }

    pub fn jumped(&mut self, index: usize) {
        if self.jump == Some(index) {
            self.jump = None;
        }
    }

    pub fn release(&mut self, app: &mut aqua::UiApp) {
        if self.prepared.take().is_some() {
            app.unregister_selection_source_in(self.document, self.source);
        }
        self.jump = None;
    }

    pub fn content(
        &self,
        preview: &ResourcePreview,
        theme: ArtisanTheme,
        viewport_height: f32,
        emit: Rc<dyn Fn(Action)>,
    ) -> Result<Element, UnpreparedError> {
        let generation = self.generation;
        let path = preview.path.clone();
        let copy_path = Rc::clone(&emit);
        let mut body = UiBox::new().layout("column").spaced(Space::S12).child(
            Button::text(
                "resource-copy-path",
                theme,
                ButtonTone::Neutral,
                "Copy path",
            )
            .on_activate(move |_| {
                copy_path(Action::CopyPath {
                    generation,
                    text: path.clone(),
                })
            })
            .build(),
        );
        match &preview.content {
            None => body = body.child(body_text(&theme, "Reading source…")),
            Some(Err(error)) => body = body.child(body_text(&theme, &failure(error))),
            Some(Ok(content)) => {
                let full = Arc::clone(content.full());
                let copy_file = Rc::clone(&emit);
                body = body
                    .child(
                        Button::text(
                            "resource-copy-text",
                            theme,
                            ButtonTone::Neutral,
                            "Copy file",
                        )
                        .on_activate(move |_| {
                            copy_file(Action::CopyFile {
                                generation,
                                text: Arc::clone(&full),
                            })
                        })
                        .build(),
                    )
                    .child(body_text(&theme, &content.note()));
                if content.window().is_some() {
                    let prepared = self.prepared.as_ref().ok_or_else(|| UnpreparedError {
                        path: preview.path.clone(),
                    })?;
                    body = body.child(lines_view(
                        &theme,
                        viewport_height,
                        self.document,
                        self.source,
                        Arc::clone(prepared),
                    ));
                }
            }
        }
        Ok(body.build())
    }

    pub fn dialog(
        &self,
        preview: &ResourcePreview,
        theme: ArtisanTheme,
        viewport_height: f32,
        emit: Rc<dyn Fn(Action)>,
    ) -> Result<Dialog, UnpreparedError> {
        let content = self.content(preview, theme, viewport_height, Rc::clone(&emit))?;
        let generation = self.generation;
        Ok(
            Dialog::new("resource-preview", theme, preview.path.clone(), content)
                .max_width(1100.0)
                .on_dismiss(move |_| emit(Action::Dismiss { generation })),
        )
    }
}

fn failure(error: &PreviewError) -> String {
    match error {
        PreviewError::Unpaired { .. } => {
            "This reference has no available paired Forge. You can copy its path.".to_owned()
        }
        PreviewError::Read { source, .. } => {
            format!(
                "Couldn't read this resource: {}",
                ::diagnostics::chain(source)
            )
        }
        PreviewError::NotText { .. } => "This resource is not a UTF-8 text file.".to_owned(),
    }
}

fn body_text(theme: &ArtisanTheme, text: &str) -> Element {
    TextRole::Label
        .text(theme, text.to_owned())
        .wrap(true)
        .build()
}

fn lines_view(
    theme: &ArtisanTheme,
    viewport_height: f32,
    document: SelectionDocumentId,
    source: SelectionSourceId,
    prepared: Arc<Prepared>,
) -> Element {
    let theme = *theme;
    let count = prepared.window.len();
    let lines = List::new()
        .items(0..count)
        .key(|index| *index)
        .row(move |index| {
            let line = prepared.window.start_line() + *index;
            let range = prepared.window.range(*index);
            let text = prepared.window.line(*index);
            let visible = &text[..text.floor_char_boundary(text.len().min(16 * 1024))];
            let target = line == prepared.target_line;
            let mut row = UiBox::new()
                .width(Length::Pct(100.0))
                .height(LINE)
                .spaced(Space::S12);
            if target {
                row = row
                    .style(Hover::Neutral.style(&theme))
                    .style(crate::ui::kit::Radius::style(
                        WELL.child_radius().unwrap_or(crate::ui::kit::Radius::Row),
                    ));
            }
            row.child(
                UiBox::new()
                    .width(GUTTER)
                    .style(Style::new().flex_shrink(0.0))
                    .child(
                        TextRole::Code
                            .text(&theme, (line + 1).to_string())
                            .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
                            .build(),
                    )
                    .build(),
            )
            .child(
                TextRole::Code
                    .text(&theme, visible.to_owned())
                    .selection_source_in(
                        document,
                        SourceSlice {
                            source,
                            range: range.start..range.start + visible.len(),
                        },
                    )
                    .wrap(false)
                    .build(),
            )
            .build()
        })
        .virtualize()
        .measured_uniform_rows()
        .estimated_row_height(Length::Px(LINE))
        .style(
            Style::new()
                .width(Length::Pct(100.0))
                .height(Length::Px((viewport_height - CHROME).max(MIN_HEIGHT))),
        )
        .build()
        .with_key("resource-lines");
    WELL.build(&theme, vec![lines])
}

#[cfg(test)]
#[path = "../../../tests/shell/resource_preview_view.rs"]
mod tests;
