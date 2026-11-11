use std::{ops::Range, rc::Rc, sync::Arc};

use aqua_text::{
    FontFamily, FontWeight, TextStyle,
    document::{
        SelectionDocumentId, SelectionSource, SelectionSourceError, SelectionSourceId, SourceSlice,
    },
};
use aqua_ui::{
    collection::List,
    element::{Box as UiBox, Element, Text},
    style::{Length, Style},
};

use super::{PreviewText, ResourcePreview};
use crate::ui::{
    button::{Button, ButtonVariant},
    dialog::Dialog,
    theme::ArtisanTheme,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    CopyPath { generation: u64, text: String },
    CopyFile { generation: u64, text: Arc<str> },
    Dismiss { generation: u64 },
}

struct Prepared {
    text: Arc<str>,
    ranges: Vec<Range<usize>>,
    start_line: usize,
    target_line: usize,
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
        if self.prepared.is_some() {
            return Ok(false);
        }
        let prepared = Arc::new(prepare_text(content, preview.target_line));
        app.register_selection_source_in(
            self.document,
            SelectionSource {
                id: self.source,
                order: vec![0],
                text: Arc::clone(&prepared.text),
                separator: Arc::from(""),
                copies: Vec::new(),
            },
        )?;
        self.jump = (!content.offsets.is_empty()).then(|| {
            preview
                .target_line
                .saturating_sub(content.start_line)
                .min(content.offsets.len() - 1)
        });
        self.prepared = Some(prepared);
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
    ) -> Result<Element, &'static str> {
        let generation = self.generation;
        let path = preview.path.clone();
        let copy_path = Rc::clone(&emit);
        let mut body = UiBox::new().layout("column").gap(12.0).child(
            Button::text("resource-copy-path", theme, "Copy path")
                .variant(ButtonVariant::Outline)
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
            Some(Err(error)) => body = body.child(body_text(&theme, error)),
            Some(Ok(content)) => {
                let Some(prepared) = &self.prepared else {
                    return Err("Resource preview selection has not been registered.");
                };
                let full: Arc<str> = Arc::from(content.text.as_str());
                let copy_file = Rc::clone(&emit);
                body = body
                    .child(
                        Button::text("resource-copy-text", theme, "Copy file")
                            .variant(ButtonVariant::Outline)
                            .on_activate(move |_| {
                                copy_file(Action::CopyFile {
                                    generation,
                                    text: Arc::clone(&full),
                                })
                            })
                            .build(),
                    )
                    .child(body_text(&theme, &content.note()))
                    .child(lines_view(
                        &theme,
                        viewport_height,
                        self.document,
                        self.source,
                        Arc::clone(prepared),
                    ));
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
    ) -> Result<Dialog, &'static str> {
        let content = self.content(preview, theme, viewport_height, Rc::clone(&emit))?;
        let generation = self.generation;
        Ok(
            Dialog::new("resource-preview", theme, preview.path.clone(), content)
                .max_width(1100.0)
                .on_dismiss(move |_| emit(Action::Dismiss { generation })),
        )
    }
}

fn prepare_text(content: &PreviewText, target_line: usize) -> Prepared {
    let mut text = String::new();
    let mut ranges = Vec::with_capacity(content.offsets.len());
    for (index, range) in content.offsets.iter().enumerate() {
        if index > 0 {
            text.push('\n');
        }
        let start = text.len();
        text.push_str(&content.text[range.clone()]);
        ranges.push(start..text.len());
    }
    Prepared {
        text: Arc::from(text),
        ranges,
        start_line: content.start_line,
        target_line,
    }
}

fn body_text(theme: &ArtisanTheme, text: &str) -> Element {
    Text::new(text.to_owned())
        .gpui_source_color(theme.colors.text.to_reference_text_color())
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
    let count = prepared.ranges.len();
    List::new()
        .items(0..count)
        .key(|index| *index)
        .row(move |index| {
            let line = prepared.start_line + *index;
            let range = &prepared.ranges[*index];
            let text = &prepared.text[range.clone()];
            let visible = &text[..text.floor_char_boundary(text.len().min(16 * 1024))];
            let style = TextStyle {
                families: vec![FontFamily::Named(theme.typography.mono.family.to_owned())],
                size_px: 13.0,
                line_height_px: 24.0,
                weight: FontWeight::NORMAL,
                ..TextStyle::default()
            };
            let color = if line == prepared.target_line {
                theme.colors.selected
            } else {
                theme.colors.surface
            };
            UiBox::new()
                .width(Length::Pct(100.0))
                .height(24.0)
                .gap(12.0)
                .style(
                    Style::new().gpui_background(aqua_core::GpuiBackground::Solid(
                        color.to_reference_paint(),
                    )),
                )
                .child(
                    UiBox::new()
                        .width(64.0)
                        .style(Style::new().flex_shrink(0.0))
                        .child(
                            Text::new((line + 1).to_string())
                                .text_style(style.clone())
                                .gpui_source_color(
                                    theme.colors.text_muted.to_reference_text_color(),
                                )
                                .build(),
                        )
                        .build(),
                )
                .child(
                    Text::new(visible.to_owned())
                        .text_style(style)
                        .gpui_source_color(theme.colors.text.to_reference_text_color())
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
        .estimated_row_height(Length::Px(24.0))
        .style(
            Style::new()
                .width(Length::Pct(100.0))
                .height(Length::Px((viewport_height - 180.0).max(120.0))),
        )
        .build()
        .with_key("resource-lines")
}

#[cfg(test)]
#[path = "../../../tests/shell/resource_preview_view.rs"]
mod tests;
