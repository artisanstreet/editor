use crate::ui::text::slice::slice;
use std::ops::Range;

use aqua_text::{InlineCellError, StyledSpan, TextStyle};
use editor_assets::AssetId;

use super::editor::StyledToken;
use crate::ui::theme::{Oklch, Opacity};

pub(crate) const MENTION_GLYPH_CELL_PX: f32 = 20.0;

#[derive(Clone, Debug, PartialEq)]
pub struct GlyphToken {
    pub range: Range<usize>,
    pub asset: AssetId,
    pub color: Oklch,
}

#[derive(Debug)]
pub struct CompiledTokens {
    pub spans: Vec<StyledSpan>,
    pub glyphs: Vec<GlyphToken>,
}

pub fn compile_tokens(
    text: &str,
    base: &TextStyle,
    foreground: Oklch,
    tokens: &[StyledToken],
    sigil_advance: impl Fn(char) -> f32,
) -> Result<CompiledTokens, InlineCellError> {
    let mut spans = Vec::new();
    let mut glyphs = Vec::new();
    let mut end = 0;
    for token in tokens {
        let range = &token.range;
        if end > range.start || range.start >= range.end || text.get(range.clone()).is_none() {
            continue;
        }
        if end < range.start {
            spans.push(StyledSpan::new(slice(text, end..range.start), base.clone()));
        }
        let mut style = base.clone();
        if let Some(weight) = token.font_weight {
            style.weight = weight;
        }
        let mut start = range.start;
        if let Some(asset) = token.glyph
            && let Some(sigil) = slice(text, range.clone()).chars().next()
        {
            let sigil_end = start + sigil.len_utf8();
            let cell = MENTION_GLYPH_CELL_PX.max(sigil_advance(sigil));
            let span = StyledSpan::new(slice(text, start..sigil_end), style.clone())
                .gpui_source_color([0.0, 0.0, 0.0, 0.0])
                .inline_cell(cell)?;
            spans.push(span);
            glyphs.push(GlyphToken {
                range: start..sigil_end,
                asset,
                color: token.color.unwrap_or(foreground).at(Opacity::MentionGlyph),
            });
            start = sigil_end;
        }
        if start < range.end {
            let mut span = StyledSpan::new(slice(text, start..range.end), style);
            if let Some(color) = token.color {
                let hsla = color.to_reference_text_color();
                span = span.gpui_source_color([
                    hsla.hue_degrees,
                    hsla.saturation,
                    hsla.lightness,
                    hsla.alpha,
                ]);
            }
            spans.push(span);
        }
        end = range.end;
    }
    if end < text.len() {
        spans.push(StyledSpan::new(slice(text, end..), base.clone()));
    }
    Ok(CompiledTokens { spans, glyphs })
}
