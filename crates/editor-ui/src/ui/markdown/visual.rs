use std::sync::Arc;

use aqua_text::document::{
    CopySubstitution, SelectionDocumentId, SelectionSource, SelectionSourceId, SourceSlice,
};
use aqua_text::{FontFamily, FontSlant, FontWeight, StyledSpan, TextInteractionRange, TextStyle};
use aqua_ui::element::{Element, ui};

use super::Span;
use super::inline::{InlineLayer, file_link_path, flatten_inline_with_swatches};
use crate::ui::media::color;
use crate::ui::rich::actions::{OpenReference, ResourceScope, ShowCopyMenu};
use crate::ui::theme::{ArtisanTheme, Oklch, Opacity, ProseTypography};
use std::ops::Range;
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq)]
pub struct InlineStyle {
    pub color: Option<Oklch>,
    pub weight: Option<FontWeight>,
    pub slant: Option<FontSlant>,
    pub monospace: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InlineRun {
    pub range: Range<usize>,
    pub style: InlineStyle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineLink {
    pub range: Range<usize>,
    pub destination: String,
}

pub struct PreparedInline {
    pub source: SelectionSource,
    pub runs: Vec<InlineRun>,
    pub links: Vec<InlineLink>,
    pub interactions: Vec<TextInteractionRange>,
    pub code_ranges: Vec<Range<usize>>,
    pub file_icons: Vec<(usize, (editor_assets::AssetId, editor_assets::AssetId))>,
    pub colors: Vec<InlineSwatch>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InlineSwatch {
    pub range: Range<usize>,
    pub slot: Range<usize>,
    pub value: String,
    pub alone: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LinkAction {
    OpenReference(OpenReference),
    OpenUrl(String),
}

impl PreparedInline {
    pub(super) fn resident_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.source.text.len()
            + self.source.separator.len()
            + self.source.order.capacity() * std::mem::size_of::<u64>()
            + self.source.copies.capacity() * std::mem::size_of::<CopySubstitution>()
            + self
                .source
                .copies
                .iter()
                .map(|copy| copy.replacement.len())
                .sum::<usize>()
            + self.runs.capacity() * std::mem::size_of::<InlineRun>()
            + self.links.capacity() * std::mem::size_of::<InlineLink>()
            + self
                .links
                .iter()
                .map(|link| link.destination.capacity())
                .sum::<usize>()
            + self.interactions.capacity() * std::mem::size_of::<TextInteractionRange>()
            + self.code_ranges.capacity() * std::mem::size_of::<Range<usize>>()
            + self.file_icons.capacity()
                * std::mem::size_of::<(usize, (editor_assets::AssetId, editor_assets::AssetId))>()
            + self.colors.capacity() * std::mem::size_of::<InlineSwatch>()
            + self
                .colors
                .iter()
                .map(|color| color.value.capacity())
                .sum::<usize>()
    }

    fn styled_spans_for(
        &self,
        theme: &ArtisanTheme,
        tone: BodyTone,
        heading: Option<u8>,
        table_header: Option<bool>,
    ) -> Vec<StyledSpan> {
        let mut base = TextStyle {
            families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
            size_px: theme.typography.size(ProseTypography::BODY_SIZE_PX),
            line_height_px: theme.typography.size(ProseTypography::BODY_LINE_PX),
            weight: ProseTypography::BODY_WEIGHT,
            letter_spacing_em: Some(theme.typography.tracking_em),
            ..TextStyle::default()
        };
        let mut base_color = match tone {
            BodyTone::Muted => theme.colors.text_muted,
            BodyTone::Foreground => theme.colors.text,
        };
        if let Some(level) = heading {
            let spec = ProseTypography::heading(level);
            base.families = vec![FontFamily::Named(
                theme.typography.heading.family.to_owned(),
            )];
            base.size_px = theme.typography.size(spec.size_px);
            base.line_height_px = theme.typography.size(spec.line_px);
            base.weight = ProseTypography::HEADING_WEIGHT;
            base.letter_spacing_em =
                Some(theme.typography.heading_tracking_px(&spec) / spec.size_px);
            base_color = theme.colors.text;
        }
        if let Some(header) = table_header {
            base.size_px = theme.typography.size(ProseTypography::TABLE_SIZE_PX);
            base.line_height_px = theme.typography.size(ProseTypography::TABLE_LINE_PX);
            if header {
                base.weight = ProseTypography::STRONG_WEIGHT;
                base_color = theme.colors.text;
            }
        }
        let mut spans = Vec::new();
        let mut cursor = 0;
        for run in &self.runs {
            if cursor < run.range.start {
                spans.push(styled_span(
                    &self.source.text[cursor..run.range.start],
                    base.clone(),
                    base_color,
                ));
            }
            let mut style = base.clone();
            if let Some(weight) = run.style.weight {
                style.weight = weight;
            }
            if let Some(slant) = run.style.slant {
                style.slant = slant;
            }
            if run.style.monospace {
                style.families = vec![FontFamily::Named(theme.typography.mono.family.to_owned())];
                style.letter_spacing_em = Some(0.0);
            }
            spans.push(styled_span(
                &self.source.text[run.range.clone()],
                style,
                run.style.color.unwrap_or(base_color),
            ));
            cursor = run.range.end;
        }
        if cursor < self.source.text.len() {
            spans.push(styled_span(&self.source.text[cursor..], base, base_color));
        }
        spans
    }

    pub fn link_action(&self, id: u64, scope: Option<ResourceScope>) -> Option<LinkAction> {
        let destination = &self.links.get(usize::try_from(id).ok()?)?.destination;
        Some(if let Some(path) = file_link_path(destination) {
            LinkAction::OpenReference(OpenReference {
                path: path.to_string_lossy().into_owned(),
                line: file_link_line(destination),
                scope,
            })
        } else {
            LinkAction::OpenUrl(destination.clone())
        })
    }

    pub fn color_action(&self, byte: usize, position: aqua_core::Point) -> Option<ShowCopyMenu> {
        let color = self
            .colors
            .iter()
            .find(|color| color.range.contains(&byte))?;
        Some(ShowCopyMenu {
            position,
            entries: color::color_copy_entries(&color.value),
        })
    }
}

fn styled_span(text: &str, style: TextStyle, color: Oklch) -> StyledSpan {
    let color = color.to_reference_text_color();
    StyledSpan::new(text, style).gpui_source_color([
        color.hue_degrees,
        color.saturation,
        color.lightness,
        color.alpha,
    ])
}

fn file_link_line(destination: &str) -> Option<u32> {
    if let Some((_, fragment)) = destination.split_once('#') {
        return fragment.strip_prefix('L').unwrap_or(fragment).parse().ok();
    }
    let mut parts = destination.rsplit(':');
    let last = parts.next()?.parse::<u32>().ok()?;
    Some(
        parts
            .next()
            .and_then(|part| part.parse().ok())
            .unwrap_or(last),
    )
}

pub fn prepare_inline(
    spans: &[Span],
    theme: &ArtisanTheme,
    source_id: SelectionSourceId,
    order: Vec<u64>,
    separator: impl Into<Arc<str>>,
) -> PreparedInline {
    let flat = flatten_inline_with_swatches(spans);
    let mut copies: Vec<_> = flat
        .colors
        .iter()
        .map(|color| CopySubstitution {
            range: color.slot.clone(),
            replacement: if color.alone {
                Arc::from(color.value.as_str())
            } else {
                Arc::from("")
            },
        })
        .collect();
    copies.extend(
        flat.file_slots
            .iter()
            .cloned()
            .map(|range| CopySubstitution {
                range,
                replacement: Arc::from(""),
            }),
    );
    copies.sort_by_key(|copy| copy.range.start);
    let styles: Vec<_> = flat
        .chains
        .iter()
        .map(|chain| {
            let mut style = InlineStyle {
                color: None,
                weight: None,
                slant: None,
                monospace: false,
            };
            for layer in chain {
                match layer {
                    InlineLayer::Code { in_link } => {
                        style.weight = Some(FontWeight::NORMAL);
                        style.monospace = true;
                        if !in_link {
                            style.color = Some(theme.colors.text_muted);
                        }
                    }
                    InlineLayer::Strong { in_link } => {
                        style.weight = Some(ProseTypography::STRONG_WEIGHT);
                        if !in_link {
                            style.color = Some(theme.colors.text);
                        }
                    }
                    InlineLayer::Emphasis => style.slant = Some(FontSlant::Italic),
                    InlineLayer::Link => {
                        style.color = Some(theme.colors.link);
                        style.weight = Some(ProseTypography::LINK_WEIGHT);
                    }
                    InlineLayer::Command => {
                        style.color = Some(theme.colors.mention_command);
                        style.weight = Some(FontWeight::MEDIUM);
                    }
                }
            }
            style
        })
        .collect();
    let runs = flat
        .runs
        .into_iter()
        .map(|(range, index)| InlineRun {
            style: styles[index].clone(),
            range,
        })
        .collect();
    let links: Vec<_> = flat
        .links
        .into_iter()
        .map(|link| InlineLink {
            range: link.range,
            destination: link.destination,
        })
        .collect();
    let interactions = links
        .iter()
        .enumerate()
        .map(|(index, link)| TextInteractionRange {
            id: index as u64,
            range: link.range.clone(),
        })
        .collect();
    PreparedInline {
        source: SelectionSource {
            id: source_id,
            order,
            text: flat.source.into(),
            separator: separator.into(),
            copies,
        },
        runs,
        links,
        interactions,
        code_ranges: flat.code_ranges,
        file_icons: flat.file_icons,
        colors: flat
            .colors
            .into_iter()
            .map(|color| InlineSwatch {
                range: color.range,
                slot: color.slot,
                value: color.value,
                alone: color.alone,
            })
            .collect(),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BodyTone {
    Muted,
    Foreground,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeafError {
    StyledRuns,
    Links,
    InlineDecorations,
}

pub struct ProseLeaf {
    pub element: Element,
    pub source: SelectionSource,
}

pub fn raw_leaf(
    raw: &str,
    theme: &ArtisanTheme,
    tone: BodyTone,
    document: SelectionDocumentId,
    source_id: SelectionSourceId,
    order: Vec<u64>,
    separator: impl Into<Arc<str>>,
) -> ProseLeaf {
    let color = match tone {
        BodyTone::Muted => theme.colors.text_muted,
        BodyTone::Foreground => theme.colors.text,
    };
    let style = TextStyle {
        families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
        size_px: theme.typography.size(ProseTypography::BODY_SIZE_PX),
        line_height_px: theme.typography.size(ProseTypography::BODY_LINE_PX),
        weight: ProseTypography::BODY_WEIGHT,
        letter_spacing_em: Some(theme.typography.tracking_em),
        ..TextStyle::default()
    };
    let text: Arc<str> = Arc::from(raw);
    let element = ui::Text::new(raw.to_owned())
        .text_style(style.clone())
        .gpui_source_color(color.to_reference_text_color())
        .spans(vec![styled_span(raw, style, color)])
        .wrap(true)
        .selection_source_in(
            document,
            SourceSlice {
                source: source_id,
                range: 0..text.len(),
            },
        )
        .build();
    ProseLeaf {
        element,
        source: SelectionSource {
            id: source_id,
            order,
            text,
            separator: separator.into(),
            copies: Vec::new(),
        },
    }
}

pub fn interactive_leaf(
    spans: &[Span],
    theme: &ArtisanTheme,
    tone: BodyTone,
    document: SelectionDocumentId,
    source_id: SelectionSourceId,
    order: Vec<u64>,
    separator: impl Into<Arc<str>>,
    on_link: Option<Rc<dyn Fn(LinkAction)>>,
    on_color: Option<Rc<dyn Fn(ShowCopyMenu)>>,
    reference_scope: Option<ResourceScope>,
    heading: Option<u8>,
) -> Result<ProseLeaf, LeafError> {
    interactive_leaf_impl(
        spans,
        theme,
        tone,
        document,
        source_id,
        order,
        separator,
        on_link,
        on_color,
        reference_scope,
        heading,
        None,
        Vec::new(),
    )
}

pub(super) fn interactive_leaf_with_opacity(
    spans: &[Span],
    theme: &ArtisanTheme,
    tone: BodyTone,
    document: SelectionDocumentId,
    source_id: SelectionSourceId,
    order: Vec<u64>,
    separator: impl Into<Arc<str>>,
    on_link: Option<Rc<dyn Fn(LinkAction)>>,
    on_color: Option<Rc<dyn Fn(ShowCopyMenu)>>,
    reference_scope: Option<ResourceScope>,
    heading: Option<u8>,
    opacity: Vec<aqua_core::scene::GpuiTextOpacityRange>,
) -> Result<ProseLeaf, LeafError> {
    interactive_leaf_impl(
        spans,
        theme,
        tone,
        document,
        source_id,
        order,
        separator,
        on_link,
        on_color,
        reference_scope,
        heading,
        None,
        opacity,
    )
}

pub fn interactive_table_leaf(
    spans: &[Span],
    theme: &ArtisanTheme,
    tone: BodyTone,
    header: bool,
    document: SelectionDocumentId,
    source_id: SelectionSourceId,
    order: Vec<u64>,
    separator: impl Into<Arc<str>>,
    on_link: Option<Rc<dyn Fn(LinkAction)>>,
    on_color: Option<Rc<dyn Fn(ShowCopyMenu)>>,
    reference_scope: Option<ResourceScope>,
) -> Result<ProseLeaf, LeafError> {
    interactive_leaf_impl(
        spans,
        theme,
        tone,
        document,
        source_id,
        order,
        separator,
        on_link,
        on_color,
        reference_scope,
        None,
        Some(header),
        Vec::new(),
    )
}

pub(super) fn interactive_table_leaf_with_opacity(
    spans: &[Span],
    theme: &ArtisanTheme,
    tone: BodyTone,
    header: bool,
    document: SelectionDocumentId,
    source_id: SelectionSourceId,
    order: Vec<u64>,
    separator: impl Into<Arc<str>>,
    on_link: Option<Rc<dyn Fn(LinkAction)>>,
    on_color: Option<Rc<dyn Fn(ShowCopyMenu)>>,
    reference_scope: Option<ResourceScope>,
    opacity: Vec<aqua_core::scene::GpuiTextOpacityRange>,
) -> Result<ProseLeaf, LeafError> {
    interactive_leaf_impl(
        spans,
        theme,
        tone,
        document,
        source_id,
        order,
        separator,
        on_link,
        on_color,
        reference_scope,
        None,
        Some(header),
        opacity,
    )
}

fn interactive_leaf_impl(
    spans: &[Span],
    theme: &ArtisanTheme,
    tone: BodyTone,
    document: SelectionDocumentId,
    source_id: SelectionSourceId,
    order: Vec<u64>,
    separator: impl Into<Arc<str>>,
    on_link: Option<Rc<dyn Fn(LinkAction)>>,
    on_color: Option<Rc<dyn Fn(ShowCopyMenu)>>,
    reference_scope: Option<ResourceScope>,
    heading: Option<u8>,
    table_header: Option<bool>,
    opacity: Vec<aqua_core::scene::GpuiTextOpacityRange>,
) -> Result<ProseLeaf, LeafError> {
    let prepared = prepare_inline(spans, theme, source_id, order, separator);
    if !prepared.colors.is_empty() && on_color.is_none() {
        return Err(LeafError::InlineDecorations);
    }
    if !prepared.links.is_empty() && on_link.is_none() {
        return Err(LeafError::Links);
    }
    let styled = prepared.styled_spans_for(theme, tone, heading, table_header);
    let mut base = TextStyle {
        families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
        size_px: theme.typography.size(ProseTypography::BODY_SIZE_PX),
        line_height_px: theme.typography.size(ProseTypography::BODY_LINE_PX),
        weight: ProseTypography::BODY_WEIGHT,
        letter_spacing_em: Some(theme.typography.tracking_em),
        ..TextStyle::default()
    };
    if let Some(level) = heading {
        let spec = ProseTypography::heading(level);
        base.families = vec![FontFamily::Named(
            theme.typography.heading.family.to_owned(),
        )];
        base.size_px = theme.typography.size(spec.size_px);
        base.line_height_px = theme.typography.size(spec.line_px);
        base.weight = ProseTypography::HEADING_WEIGHT;
        base.letter_spacing_em = Some(theme.typography.heading_tracking_px(&spec) / spec.size_px);
    }
    if let Some(header) = table_header {
        base.size_px = theme.typography.size(ProseTypography::TABLE_SIZE_PX);
        base.line_height_px = theme.typography.size(ProseTypography::TABLE_LINE_PX);
        if header {
            base.weight = ProseTypography::STRONG_WEIGHT;
        }
    }
    let source = prepared.source.clone();
    let interactions = prepared.interactions.clone();
    let mut paints = Vec::new();
    for (offset, (light, dark)) in &prepared.file_icons {
        let id = if theme.colors.mode.is_dark() {
            dark
        } else {
            light
        };
        let asset = editor_assets::get(*id);
        let base_color = if heading.is_some() || table_header == Some(true) {
            theme.colors.text
        } else {
            match tone {
                BodyTone::Muted => theme.colors.text_muted,
                BodyTone::Foreground => theme.colors.text,
            }
        };
        paints.push(aqua_ui::tree::TextInlinePaint {
            bytes: *offset..*offset + '\u{2003}'.len_utf8(),
            content: aqua_ui::tree::TextInlinePaintContent::Svg {
                source: Arc::from(asset.source),
                tint: (asset.presentation == editor_assets::Presentation::Tinted)
                    .then(|| base_color.to_reference_paint()),
            },
        });
    }
    for swatch in &prepared.colors {
        if let Some(color) = color::parse(&swatch.value) {
            let offset = swatch.slot.start + '\u{2060}'.len_utf8();
            paints.push(aqua_ui::tree::TextInlinePaint {
                bytes: offset..offset + '\u{2003}'.len_utf8(),
                content: aqua_ui::tree::TextInlinePaintContent::Swatch {
                    fill: color.reference_paint(),
                    edge: theme
                        .colors
                        .text
                        .at(Opacity::SwatchEdge)
                        .to_reference_paint(),
                    centered: swatch.alone,
                },
            });
        }
    }
    let prepared = Rc::new(prepared);
    let base_color = if heading.is_some() || table_header == Some(true) {
        theme.colors.text
    } else {
        match tone {
            BodyTone::Muted => theme.colors.text_muted,
            BodyTone::Foreground => theme.colors.text,
        }
    };
    let mut element = ui::Text::new(source.text.to_string())
        .text_style(base)
        .gpui_source_color(base_color.to_reference_text_color())
        .spans(styled)
        .gpui_text_opacity(opacity)
        .inline_paints(paints)
        .wrap(true)
        .interaction_ranges(interactions)
        .selection_source_in(
            document,
            SourceSlice {
                source: source_id,
                range: 0..source.text.len(),
            },
        )
        .build();
    if let Some(level) = heading {
        element = element.role(aqua_core::Role::Heading(level));
    }
    if !source.copies.is_empty() {
        element = element.aria_label(accessible_text(&source));
    }
    if let Some(on_link) = on_link {
        let prepared = prepared.clone();
        element = element.on_input(
            aqua_ui::input::InputKind::TextRangeActivated,
            aqua_ui::input::EventPhase::Bubble,
            move |event| {
                if let aqua_ui::input::RoutedInput::TextRangeActivated { id, .. } = &event.input
                    && let Some(action) = prepared.link_action(*id, reference_scope.clone())
                {
                    on_link(action);
                    return aqua_ui::input::InputResponse::handled();
                }
                aqua_ui::input::InputResponse::default()
            },
        );
    }
    if let Some(on_color) = on_color {
        element = element.on_input(
            aqua_ui::input::InputKind::TextContext,
            aqua_ui::input::EventPhase::Bubble,
            move |event| {
                if let aqua_ui::input::RoutedInput::TextContext { pointer, byte } = &event.input
                    && let Some(action) = prepared.color_action(*byte, pointer.position)
                {
                    on_color(action);
                    return aqua_ui::input::InputResponse::handled();
                }
                aqua_ui::input::InputResponse::default()
            },
        );
    }
    Ok(ProseLeaf { element, source })
}

fn accessible_text(source: &SelectionSource) -> String {
    let mut text = String::with_capacity(source.text.len());
    let mut cursor = 0;
    for copy in &source.copies {
        if let Some(kept) = source.text.get(cursor..copy.range.start) {
            text.push_str(kept);
        }
        text.push_str(&copy.replacement);
        cursor = cursor.max(copy.range.end);
    }
    if let Some(rest) = source.text.get(cursor..) {
        text.push_str(rest);
    }
    text
}

pub fn prose_leaf(
    spans: &[Span],
    theme: &ArtisanTheme,
    tone: BodyTone,
    document: SelectionDocumentId,
    source_id: SelectionSourceId,
    order: Vec<u64>,
    separator: impl Into<Arc<str>>,
) -> Result<ProseLeaf, LeafError> {
    let flat = flatten_inline_with_swatches(spans);
    if !flat.file_slots.is_empty() || !flat.colors.is_empty() {
        return Err(LeafError::InlineDecorations);
    }
    if !flat.links.is_empty() {
        return Err(LeafError::Links);
    }
    if !flat.runs.is_empty() {
        return Err(LeafError::StyledRuns);
    }
    let color = match tone {
        BodyTone::Muted => theme.colors.text_muted,
        BodyTone::Foreground => theme.colors.text,
    };
    let style = TextStyle {
        families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
        size_px: theme.typography.size(ProseTypography::BODY_SIZE_PX),
        line_height_px: theme.typography.size(ProseTypography::BODY_LINE_PX),
        weight: ProseTypography::BODY_WEIGHT,
        letter_spacing_em: Some(theme.typography.tracking_em),
        ..TextStyle::default()
    };
    let text: Arc<str> = flat.source.into();
    let span = styled_span(&text, style.clone(), color);
    let element = ui::Text::new(text.to_string())
        .text_style(style)
        .gpui_source_color(color.to_reference_text_color())
        .spans(vec![span])
        .wrap(true)
        .selection_source_in(
            document,
            SourceSlice {
                source: source_id,
                range: 0..text.len(),
            },
        )
        .build();
    Ok(ProseLeaf {
        element,
        source: SelectionSource {
            id: source_id,
            order,
            text,
            separator: separator.into(),
            copies: Vec::new(),
        },
    })
}

#[cfg(test)]
#[path = "../../../tests/markdown/visual.rs"]
mod tests;
