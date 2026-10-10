//! Mentions as the editor paints and edits them.
//!
//! Each recognized mention paints in its kind's colour with a glyph in
//! place of its sigil: the sigil stays in the text (the draft is never
//! rewritten) but paints transparent, and its run is widened so the glyph
//! and a gap fit where the sigil was. Editing treats a mention as one unit:
//! a delete at its edge removes it whole and the arrows step over it.

#![forbid(unsafe_code)]

use super::*;
use crate::composer_mentions::{
    MentionCatalog, MentionKind, MentionSpan, mention_color, mention_deletion, recognize_mentions,
    step_over_mention,
};
use artisan_ui::text_runs::{TextRunOverride, compile_text_runs};
use gpui::{FontWeight, Hsla, TextStyle};

/// Width a mention's glyph takes in place of its sigil, gap included.
const MENTION_GLYPH_CELL_PX: f32 = 20.0;
/// Painted size of a mention's glyph.
const MENTION_GLYPH_SIZE_PX: f32 = 15.0;

/// The colour a mention's glyph paints in: its text colour, slightly muted.
pub(crate) fn mention_glyph_color(theme: &ArtisanTheme, kind: MentionKind) -> Hsla {
    mention_color(theme, kind).opacity(0.85)
}

impl NativeComposer {
    /// Replaces what the composer can name. Notifies only on a change, so a
    /// per-render caller cannot loop.
    pub(crate) fn set_mention_catalog(&mut self, catalog: MentionCatalog, cx: &mut Context<Self>) {
        if self.mentions != catalog {
            self.mentions = catalog;
            self.layout = None;
            cx.notify();
        }
    }

    /// The mentions the draft holds now.
    pub(super) fn mention_spans(&self) -> Vec<MentionSpan> {
        recognize_mentions(self.state.draft(), &self.mentions)
    }

    /// The range one delete removes when the caret is collapsed at or inside
    /// a mention.
    pub(super) fn mention_deletion_range(&self, forward: bool) -> Option<Range<usize>> {
        if !self.selection.is_empty() || self.marked_range.is_some() {
            return None;
        }
        mention_deletion(&self.mention_spans(), self.selection.end, forward)
    }

    /// A caret target moved off the inside of a mention.
    pub(super) fn caret_target_over_mentions(&self, target: usize, forward: bool) -> usize {
        step_over_mention(&self.mention_spans(), target, forward)
    }

    /// The editor's text: the draft with its mentions coloured and the
    /// selection washed behind the glyphs. A command paints capitalized
    /// (`/goal` reads `Goal`) while the draft keeps what engines read; the
    /// painted text has the draft's exact byte length, so carets, selection,
    /// and hit tests stay aligned.
    pub(super) fn composer_text(
        &self,
        theme: &ArtisanTheme,
        desktop_theme: DesktopTheme,
    ) -> ComposerText {
        let draft = self.state.draft();
        let mut painted = draft.to_owned();
        let mut highlights = Vec::new();
        let mut sigils = Vec::new();
        for span in self.mention_spans() {
            let Some(sigil) = draft[span.range.clone()].chars().next() else {
                continue;
            };
            let sigil_end = span.range.start + sigil.len_utf8();
            highlights.push((
                span.range.start..sigil_end,
                HighlightStyle {
                    color: Some(gpui::transparent_black()),
                    ..Default::default()
                },
            ));
            highlights.push((
                sigil_end..span.range.end,
                HighlightStyle {
                    color: Some(mention_color(theme, span.kind)),
                    font_weight: Some(FontWeight::MEDIUM),
                    ..Default::default()
                },
            ));
            sigils.push((span.range.start..sigil_end, sigil));
            if span.kind == MentionKind::Command
                && let Some(first) = painted.get_mut(sigil_end..=sigil_end)
            {
                first.make_ascii_uppercase();
            }
        }
        if !self.selection.is_empty() {
            // Background only: a selected mention keeps its colour.
            highlights.push((
                self.selection.clone(),
                HighlightStyle {
                    background_color: Some(desktop_theme.selected),
                    ..Default::default()
                },
            ));
        }
        ComposerText {
            styled: Some(StyledText::new(SharedString::from(painted.clone()))),
            text: SharedString::from(painted),
            highlights,
            sigils,
        }
    }

    /// The editor's text layout and the mentions it holds, for painting
    /// their glyphs.
    pub(super) fn mention_glyph_inputs(&self) -> Option<(gpui::TextLayout, Vec<MentionSpan>)> {
        let spans = self.mention_spans();
        (!spans.is_empty())
            .then(|| self.layout.clone().map(|layout| (layout, spans)))
            .flatten()
    }
}

/// Lays out and prepaints one glyph over every mention's sigil, at the
/// positions the editor's text was just laid out at.
pub(super) fn prepaint_mention_glyphs(
    layout: &gpui::TextLayout,
    spans: Vec<MentionSpan>,
    window: &mut Window,
    cx: &mut App,
) -> Vec<AnyElement> {
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let line_height = layout.line_height();
    let glyph = px(MENTION_GLYPH_SIZE_PX);
    let mut glyphs = Vec::new();
    for span in spans {
        let Some(origin) = layout.position_for_index(span.range.start) else {
            continue;
        };
        if !valid_point(origin) {
            continue;
        }
        let mut element = asset_glyph(span.icon)
            .size(glyph)
            .text_color(mention_glyph_color(&theme, span.kind))
            .into_any_element();
        let top = origin.y + (line_height - glyph) / 2.0;
        element.prepaint_as_root(point(origin.x, top), size(glyph, glyph).into(), window, cx);
        glyphs.push(element);
    }
    glyphs
}

/// The editor's styled text. Runs are compiled at layout time against the
/// inherited text style, so each sigil can be widened to its glyph's cell by
/// measuring the sigil in the font actually used.
pub(super) struct ComposerText {
    styled: Option<StyledText>,
    text: SharedString,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    sigils: Vec<(Range<usize>, char)>,
}

impl ComposerText {
    /// The text as painted.
    #[cfg(test)]
    pub(super) fn painted_text(&self) -> &str {
        &self.text
    }

    /// The layout handle the composer measures carets and hits against.
    pub(super) fn layout(&self) -> Option<&gpui::TextLayout> {
        self.styled.as_ref().map(StyledText::layout)
    }

    fn sigil_overrides(&self, style: &TextStyle, window: &Window) -> Vec<TextRunOverride> {
        if self.sigils.is_empty() {
            return Vec::new();
        }
        let font_size = style.font_size.to_pixels(window.rem_size());
        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&style.font());
        self.sigils
            .iter()
            .map(|(range, sigil)| {
                let advance = text_system
                    .advance(font_id, font_size, *sigil)
                    .map_or(px(0.0), |advance| advance.width);
                TextRunOverride {
                    range: range.clone(),
                    font_family: None,
                    letter_spacing: Some((px(MENTION_GLYPH_CELL_PX) - advance).max(px(0.0))),
                }
            })
            .collect()
    }
}

impl Element for ComposerText {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let style = window.text_style();
        let overrides = self.sigil_overrides(&style, window);
        let runs = compile_text_runs(&self.text, &style, &self.highlights, &overrides);
        let styled = self
            .styled
            .take()
            .unwrap_or_else(|| StyledText::new(self.text.clone()))
            .with_runs(runs);
        let styled = self.styled.insert(styled);
        let (layout_id, ()) = styled.request_layout(None, inspector_id, window, cx);
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        state: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        if let Some(styled) = self.styled.as_mut() {
            styled.prepaint(None, inspector_id, bounds, state, window, cx);
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(styled) = self.styled.as_mut() {
            styled.paint(
                None,
                inspector_id,
                bounds,
                request_layout,
                prepaint,
                window,
                cx,
            );
        }
    }
}

impl IntoElement for ComposerText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}
