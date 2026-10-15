//! Read-only source previews fetched from the Forge that produced a reference.

use editor_core::ForgeClient;
use editor_ui::theme::ActiveTheme as _;
use editor_ui::{
    button::{Button, ButtonVariant},
    dialog::Dialog,
    markdown_renderer::OpenReference,
    selectable_text::{
        SelectableText, SelectionLeaf, SelectionSource, register_selection_source,
        release_selection_scope,
    },
};
use gpui::{
    Context, EventEmitter, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    ScrollStrategy, SharedString, Styled as _, UniformListScrollHandle, Window, div, px,
    uniform_list,
};
use std::{rc::Rc, sync::Arc};

pub struct Dismissed;
pub struct ResourcePreview {
    path: SharedString,
    target_line: usize,
    content: Option<Result<Arc<PreviewText>, String>>,
    scroll: UniformListScrollHandle,
    jump: bool,
    selection_scope: SharedString,
    selection_source: Option<Rc<SelectionSource>>,
    request: Option<gpui::Task<()>>,
    release: Option<gpui::Subscription>,
}
impl EventEmitter<Dismissed> for ResourcePreview {}

impl ResourcePreview {
    pub fn new(
        reference: OpenReference,
        client: Option<ForgeClient>,
        cx: &mut Context<Self>,
    ) -> Self {
        let selection_scope: SharedString = format!("source-preview-{:?}", cx.entity_id()).into();
        let mut request = None;
        let target_line = reference.line.unwrap_or(1).saturating_sub(1) as usize;
        let path = reference.path;
        let content = if let (Some(scope), Some(client)) = (reference.scope, client) {
            let fetch_path = path.clone();
            request = Some(cx.spawn(async move |this, cx| {
                let result = client.resource(&scope.thread, &fetch_path, false).await;
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        let bytes = result
                            .map_err(|error| format!("Couldn't read this resource: {error:#}"))?;
                        let text = String::from_utf8(bytes)
                            .map_err(|_| "This resource is not a UTF-8 text file.".to_owned())?;
                        Ok::<_, String>(Arc::new(PreviewText::new(text, target_line)))
                    })
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.selection_source = result
                        .as_ref()
                        .ok()
                        .map(|text| text.selection_source(this.selection_scope.clone()));
                    this.content = Some(result);
                    this.request = None;
                    this.jump = true;
                    cx.notify();
                });
            }));
            None
        } else {
            Some(Err(
                "This reference has no available paired Forge. You can copy its path.".into(),
            ))
        };
        Self {
            path,
            target_line,
            content,
            scroll: UniformListScrollHandle::new(),
            jump: false,
            selection_scope,
            selection_source: None,
            request,
            release: None,
        }
    }
}

impl Render for ResourcePreview {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.release.is_none() {
            let scope = self.selection_scope.clone();
            self.release = Some(
                window.observe_release(&cx.entity(), cx, move |_, window, cx| {
                    release_selection_scope(&scope, window.window_handle().window_id(), cx);
                }),
            );
        }
        if let Some(source) = &self.selection_source {
            register_selection_source(source, window, cx);
        }
        let theme = *cx.theme();
        let content = match &self.content {
            None => div().child("Reading source…").into_any_element(),
            Some(Err(error)) => div().child(error.clone()).into_any_element(),
            Some(Ok(lines)) => {
                if self.jump {
                    self.scroll.scroll_to_item(
                        self.target_line
                            .saturating_sub(lines.start_line)
                            .min(lines.offsets.len().saturating_sub(1)),
                        ScrollStrategy::Top,
                    );
                    self.jump = false;
                }
                let lines = lines.clone();
                let target_line = self.target_line;
                let scope = self.selection_scope.clone();
                uniform_list("resource-lines", lines.offsets.len(), move |range, _, _| {
                    range
                        .map(|index| {
                            let line = lines.start_line + index;
                            let text = &lines.text[lines.offsets[index].clone()];
                            let visible =
                                &text[..text.floor_char_boundary(text.len().min(16 * 1024))];
                            div()
                                .id(index)
                                .h(px(24.0))
                                .flex()
                                .gap(px(12.0))
                                .text_size(px(13.0))
                                .bg(if line == target_line {
                                    theme.colors.selected.to_paint()
                                } else {
                                    theme.colors.sidebar.to_paint()
                                })
                                .font_family(theme.typography.mono.family)
                                .tracking(0.0)
                                .child(
                                    div()
                                        .w(px(64.0))
                                        .flex_shrink_0()
                                        .text_color(theme.colors.workspace_secondary.to_paint())
                                        .child((line + 1).to_string()),
                                )
                                .child(
                                    SelectableText::retained(
                                        ("source-line", index),
                                        SharedString::from(visible.to_owned()),
                                        theme,
                                        Vec::new(),
                                    )
                                    .selection_order(vec![line as u64], "\n")
                                    .selection_scope(scope.clone()),
                                )
                        })
                        .collect()
                })
                .h((window.viewport_size().height - px(180.0)).max(px(120.0)))
                .track_scroll(&self.scroll)
                .into_any_element()
            }
        };
        let source = self
            .content
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .cloned();
        let preview_note = source.as_ref().map(|text| {
            format!(
                "Lines {}–{} of {}. Long lines are clipped visually. Select All copies these preview lines; Copy file copies the full file.",
                text.start_line + 1,
                text.start_line + text.offsets.len(),
                text.total_lines
            )
        });
        let path = self.path.clone();
        let body = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .child(
                Button::text("resource-copy-path", theme, "Copy path")
                    .variant(ButtonVariant::Outline)
                    .on_activate(move |_, _, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(path.to_string()))
                    }),
            )
            .children(source.map(|text| {
                Button::text("resource-copy-text", theme, "Copy file")
                    .variant(ButtonVariant::Outline)
                    .on_activate(move |_, _, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.text.clone()))
                    })
            }))
            .children(preview_note)
            .child(content);
        Dialog::new("resource-preview", theme, self.path.clone(), body)
            .max_width(px(1100.0))
            .on_dismiss({
                let owner = cx.entity().downgrade();
                move |_, _, cx| {
                    let _ = owner.update(cx, |_, cx| cx.emit(Dismissed));
                }
            })
    }
}

// Keep the fetched UTF-8 bytes once and at most 5,000 offsets around the reference.
// A file full of blank lines cannot allocate millions of per-line strings.
struct PreviewText {
    text: String,
    start_line: usize,
    offsets: Vec<std::ops::Range<usize>>,
    total_lines: usize,
}
impl PreviewText {
    fn selection_source(&self, scope: SharedString) -> Rc<SelectionSource> {
        SelectionSource::new(
            scope,
            vec![0],
            self.offsets
                .iter()
                .enumerate()
                .map(|(index, range)| SelectionLeaf {
                    order: vec![(self.start_line + index) as u64],
                    text: self.text[range.clone()].to_owned().into(),
                    separator: "\n",
                    copies: Rc::from([]),
                })
                .collect(),
        )
    }
    fn new(text: String, target_line: usize) -> Self {
        let total_lines = text.lines().count();
        let start_line = target_line
            .min(total_lines.saturating_sub(1))
            .saturating_sub(2_500);
        let base = text.as_ptr() as usize;
        let offsets = text
            .lines()
            .skip(start_line)
            .take(5_000)
            .map(|line| {
                let start = line.as_ptr() as usize - base;
                start..start + line.len()
            })
            .collect();
        Self {
            text,
            start_line,
            offsets,
            total_lines,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_preview_bounds_line_metadata_and_keeps_target() {
        let text = "x\n".repeat(100_000);
        let preview = PreviewText::new(text, 80_000);
        assert_eq!(preview.offsets.len(), 5_000);
        assert_eq!(preview.start_line, 77_500);
        assert_eq!(preview.total_lines, 100_000);
        assert_eq!(&preview.text[preview.offsets[2_500].clone()], "x");
    }
}
