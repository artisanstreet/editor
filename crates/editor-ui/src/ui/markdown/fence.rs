use crate::ui::kit::{Space, Spaced, TextRole};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use aqua_text::document::{
    CopySubstitution, SelectionDocumentId, SelectionSource, SelectionSourceId, SourceSlice,
};
use aqua_text::{StyledSpan, TextStyle};
use aqua_ui::collection::List;
use aqua_ui::element::{Element, ui};
use aqua_ui::input::{EventPhase, InputKind, InputResponse, RoutedInput};
use aqua_ui::style::{Length, Style};
use unicode_width::UnicodeWidthStr;

use super::inline::SWATCH_SLOT;
use super::{CodeFence, CodeTokenKind};
use crate::ui::media::color;
use crate::ui::rich::actions::ShowCopyMenu;
use crate::ui::rich::code::card::{self, CopyPlacement};
use crate::ui::theme::{ArtisanTheme, Oklch, Opacity, ProseTypography};

#[derive(Clone)]
struct Swatch {
    range: Range<usize>,
    slot: Range<usize>,
    value: String,
}

#[derive(Clone)]
struct Line {
    text: Arc<str>,
    tokens: Vec<(Range<usize>, CodeTokenKind)>,
    colors: Vec<Swatch>,
}

pub(super) struct Fence {
    source: String,
    lines: Arc<[Line]>,
    widest: usize,
}

pub(super) type CopyFactory = Rc<dyn Fn(&str, &str) -> Element>;

impl Fence {
    pub(super) fn resident_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.source.capacity()
            + self.lines.len() * std::mem::size_of::<Line>()
            + self
                .lines
                .iter()
                .map(|line| {
                    line.text.len()
                        + line.tokens.capacity()
                            * std::mem::size_of::<(Range<usize>, CodeTokenKind)>()
                        + line.colors.capacity() * std::mem::size_of::<Swatch>()
                        + line
                            .colors
                            .iter()
                            .map(|color| color.value.capacity())
                            .sum::<usize>()
                })
                .sum::<usize>()
    }
    pub(super) fn has_colors(&self) -> bool {
        self.lines.iter().any(|line| !line.colors.is_empty())
    }
    pub(super) fn new(fence: &CodeFence) -> Self {
        let source = fence.source.strip_suffix('\n').unwrap_or(&fence.source);
        let source = source.strip_suffix('\r').unwrap_or(source);
        let whole = 0..source.len();
        let colors = color::color_ranges(source, std::slice::from_ref(&whole));
        let shift = |offset: usize| {
            offset + SWATCH_SLOT.len() * colors.partition_point(|range| range.start < offset)
        };
        let mut shown = String::new();
        let mut swatches = Vec::new();
        let mut cursor = 0;
        for range in &colors {
            shown.push_str(&source[cursor..range.start]);
            let start = shown.len();
            shown.push_str(SWATCH_SLOT);
            let slot = start..shown.len();
            shown.push_str(&source[range.clone()]);
            swatches.push(Swatch {
                range: start..shown.len(),
                slot,
                value: source[range.clone()].to_owned(),
            });
            cursor = range.end;
        }
        shown.push_str(&source[cursor..]);
        let tokens: Vec<_> = fence
            .tokens
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter(|token| {
                token.range.start <= token.range.end
                    && token.range.end <= source.len()
                    && source.is_char_boundary(token.range.start)
                    && source.is_char_boundary(token.range.end)
            })
            .map(|token| (shift(token.range.start)..shift(token.range.end), token.kind))
            .collect();
        let mut ranges = Vec::new();
        let mut start = 0;
        for part in shown.split_inclusive('\n') {
            let end = start + part.trim_end_matches('\n').len();
            ranges.push(start..end);
            start += part.len();
        }
        if ranges.is_empty() || shown.ends_with('\n') {
            ranges.push(start..start);
        }
        let mut lines = Vec::with_capacity(ranges.len());
        for range in ranges {
            let line_tokens = tokens
                .iter()
                .filter_map(|(token, kind)| {
                    let start = token.start.max(range.start);
                    let end = token.end.min(range.end);
                    (start < end).then(|| (start - range.start..end - range.start, *kind))
                })
                .collect();
            let line_colors = swatches
                .iter()
                .filter(|color| color.range.start >= range.start && color.range.end <= range.end)
                .map(|color| Swatch {
                    range: color.range.start - range.start..color.range.end - range.start,
                    slot: color.slot.start - range.start..color.slot.end - range.start,
                    value: color.value.clone(),
                })
                .collect();
            lines.push(Line {
                text: Arc::from(&shown[range]),
                tokens: line_tokens,
                colors: line_colors,
            });
        }
        let widest = lines
            .iter()
            .enumerate()
            .max_by_key(|(_, line)| UnicodeWidthStr::width(line.text.as_ref()))
            .map_or(0, |(index, _)| index);
        Self {
            source: source.to_owned(),
            lines: lines.into(),
            widest,
        }
    }

    pub(super) fn sources(
        &self,
        prefix: &[u64],
        source_id: &mut dyn FnMut(usize) -> SelectionSourceId,
        offset: usize,
    ) -> Vec<SelectionSource> {
        self.lines
            .iter()
            .enumerate()
            .map(|(line, entry)| {
                let mut order = prefix.to_vec();
                order.push(line as u64);
                SelectionSource {
                    id: source_id(offset + line),
                    order,
                    text: entry.text.clone(),
                    separator: Arc::from("\n"),
                    copies: entry
                        .colors
                        .iter()
                        .map(|color| CopySubstitution {
                            range: color.slot.clone(),
                            replacement: Arc::from(""),
                        })
                        .collect(),
                }
            })
            .collect()
    }

    pub(super) fn render(
        &self,
        id: &str,
        theme: &ArtisanTheme,
        document: SelectionDocumentId,
        sources: &[SelectionSource],
        copy: Option<CopyFactory>,
        on_color: Option<Rc<dyn Fn(ShowCopyMenu)>>,
    ) -> Element {
        let lines = self.lines.clone();
        let ids: Arc<[SelectionSourceId]> = sources.iter().map(|source| source.id).collect();
        let theme = *theme;
        let count = lines.len();
        let body = List::new()
            .items(0..count)
            .key(|line| *line)
            .virtualize()
            .width_probe(self.widest)
            .estimated_row_height(Length::Px(ProseTypography::CODE_LINE_PX))
            .style(Style::new().width(Length::Pct(100.0)).height(Length::Px(
                crate::ui::rich::charts::scale::f32_of_count(count.min(24))
                    * ProseTypography::CODE_LINE_PX,
            )))
            .row(move |index| {
                line_view(
                    &lines[*index],
                    &theme,
                    document,
                    ids[*index],
                    on_color.clone(),
                )
            })
            .build()
            .with_key(format!("{id}-lines"));
        let mut content = ui::Box::new()
            .layout("column")
            .spaced(Space::S8)
            .width(Length::Pct(100.0))
            .child(body);
        if count > 24 {
            content = content.child(crate::ui::rich::narrative::callout::text(
                &theme,
                &format!("{count} lines · Scroll to read more"),
                TextRole::Caption,
                theme.colors.text_muted,
            ));
        }
        let placement = if self.source.contains('\n') {
            CopyPlacement::TopRight
        } else {
            CopyPlacement::CenterRight
        };
        card::render(
            &theme,
            content.build(),
            copy.map(|copy| (copy(id, &self.source), placement)),
            false,
        )
    }
}

fn token_color(theme: &ArtisanTheme, kind: CodeTokenKind) -> Oklch {
    match kind {
        CodeTokenKind::Comment => theme.colors.syntax.comment,
        CodeTokenKind::Str => theme.colors.syntax.string,
        CodeTokenKind::Number => theme.colors.syntax.number,
        CodeTokenKind::Keyword => theme.colors.syntax.keyword,
        CodeTokenKind::Type => theme.colors.syntax.r#type,
        CodeTokenKind::Function => theme.colors.syntax.function,
        CodeTokenKind::Plain => theme.colors.text,
    }
}

fn line_view(
    line: &Line,
    theme: &ArtisanTheme,
    document: SelectionDocumentId,
    id: SelectionSourceId,
    on_color: Option<Rc<dyn Fn(ShowCopyMenu)>>,
) -> Element {
    let style = TextStyle {
        letter_spacing_em: Some(0.0),
        ..TextRole::Code.style(theme)
    };
    let mut spans = Vec::new();
    let mut cursor = 0;
    for (range, kind) in &line.tokens {
        if cursor < range.start {
            spans.push(span(
                &line.text[cursor..range.start],
                &style,
                theme.colors.text,
            ));
        }
        if cursor <= range.start {
            spans.push(span(
                &line.text[range.clone()],
                &style,
                token_color(theme, *kind),
            ));
            cursor = range.end;
        }
    }
    if cursor < line.text.len() {
        spans.push(span(&line.text[cursor..], &style, theme.colors.text));
    }
    if spans.is_empty() {
        spans.push(span("", &style, theme.colors.text));
    }
    let paints = line
        .colors
        .iter()
        .filter_map(|color| {
            let start = color.slot.start + '\u{2060}'.len_utf8();
            Some(aqua_ui::tree::TextInlinePaint {
                bytes: start..start + '\u{2003}'.len_utf8(),
                content: aqua_ui::tree::TextInlinePaintContent::Swatch {
                    fill: color::parse(&color.value)?.reference_paint(),
                    edge: theme
                        .colors
                        .text
                        .at(Opacity::SwatchEdge)
                        .to_reference_paint(),
                    centered: false,
                },
            })
        })
        .collect();
    let mut element = ui::Text::new(line.text.to_string())
        .text_style(style)
        .gpui_source_color(theme.colors.text.to_reference_text_color())
        .spans(spans)
        .inline_paints(paints)
        .wrap(false)
        .selection_source_in(
            document,
            SourceSlice {
                source: id,
                range: 0..line.text.len(),
            },
        )
        .build();
    if let Some(on_color) = on_color {
        let colors = line.colors.clone();
        element = element.on_input(InputKind::TextContext, EventPhase::Bubble, move |event| {
            if let RoutedInput::TextContext { pointer, byte } = &event.input
                && let Some(color) = colors.iter().find(|color| color.range.contains(byte))
            {
                on_color(ShowCopyMenu {
                    position: pointer.position,
                    entries: color::color_copy_entries(&color.value),
                });
                return InputResponse::handled();
            }
            InputResponse::default()
        });
    }
    ui::Box::new()
        .height(ProseTypography::CODE_LINE_PX)
        .clip(true)
        .child(element)
        .build()
}

fn span(text: &str, style: &TextStyle, color: Oklch) -> StyledSpan {
    let paint = color.to_reference_text_color();
    StyledSpan::new(text, style.clone()).gpui_source_color([
        paint.hue_degrees,
        paint.saturation,
        paint.lightness,
        paint.alpha,
    ])
}
