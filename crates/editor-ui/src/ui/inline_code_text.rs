use std::ops::Range;

use aqua_core::GpuiTextColor;
use aqua_text::{FontFamily, FontSlant, FontWeight, StyledSpan, TextStyle};
use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

use super::theme::{ArtisanTheme, ProseTypography};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InlineStyle {
    pub code: bool,
    pub strong: bool,
    pub em: bool,
    pub strike: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineRange {
    pub range: Range<usize>,
    pub style: InlineStyle,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InlineRuns {
    pub text: String,
    pub ranges: Vec<InlineRange>,
}

impl InlineRuns {
    pub fn styled_spans(
        &self,
        base: &TextStyle,
        color: GpuiTextColor,
        theme: &ArtisanTheme,
    ) -> Vec<StyledSpan> {
        let source = [
            color.hue_degrees,
            color.saturation,
            color.lightness,
            color.alpha,
        ];
        self.ranges
            .iter()
            .map(|run| {
                let mut style = base.clone();
                if run.style.code {
                    style.weight = FontWeight::NORMAL;
                    style.families =
                        vec![FontFamily::Named(theme.typography.mono.family.to_owned())];
                    style.letter_spacing_em = Some(0.0);
                } else {
                    if run.style.strong {
                        style.weight = ProseTypography::STRONG_WEIGHT;
                    }
                    if run.style.em {
                        style.slant = FontSlant::Italic;
                    }
                    if run.style.strike {
                        style.strikethrough = true;
                    }
                }
                StyledSpan::new(&self.text[run.range.clone()], style).gpui_source_color(source)
            })
            .collect()
    }
}

pub fn inline_runs(raw: &str) -> InlineRuns {
    let mut fragments: Vec<(String, InlineStyle)> = Vec::new();
    let (mut strong, mut em, mut strike, mut block_code) = (0usize, 0usize, 0usize, false);
    for event in Parser::new_ext(raw, Options::ENABLE_STRIKETHROUGH) {
        let (text, code) = match event {
            Event::Start(Tag::Strong) => {
                strong += 1;
                continue;
            }
            Event::End(TagEnd::Strong) => {
                strong = strong.saturating_sub(1);
                continue;
            }
            Event::Start(Tag::Emphasis) => {
                em += 1;
                continue;
            }
            Event::End(TagEnd::Emphasis) => {
                em = em.saturating_sub(1);
                continue;
            }
            Event::Start(Tag::Strikethrough) => {
                strike += 1;
                continue;
            }
            Event::End(TagEnd::Strikethrough) => {
                strike = strike.saturating_sub(1);
                continue;
            }
            Event::Start(Tag::CodeBlock(_)) => {
                block_code = true;
                continue;
            }
            Event::End(TagEnd::CodeBlock) => {
                block_code = false;
                continue;
            }
            Event::Code(text) => (text.into_string(), true),
            Event::Text(text) | Event::InlineHtml(text) => (text.into_string(), block_code),
            Event::SoftBreak
            | Event::HardBreak
            | Event::End(TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item) => {
                (" ".to_owned(), false)
            }
            _ => continue,
        };
        let style = InlineStyle {
            code,
            strong: strong > 0,
            em: em > 0,
            strike: strike > 0,
        };
        let mut text = text;
        if let Some((previous, previous_style)) = fragments.last_mut() {
            if *previous_style == style {
                previous.push_str(&text);
                continue;
            }
        } else {
            let leading = text.len() - text.trim_start().len();
            text.drain(..leading);
        }
        if !text.is_empty() {
            fragments.push((text, style));
        }
    }
    if let Some((text, _)) = fragments.last_mut() {
        text.truncate(text.trim_end().len());
    }
    let mut runs = InlineRuns::default();
    for (text, style) in fragments {
        if text.is_empty() {
            continue;
        }
        let start = runs.text.len();
        runs.text.push_str(&text);
        runs.ranges.push(InlineRange {
            range: start..runs.text.len(),
            style,
        });
    }
    runs
}

#[cfg(test)]
#[path = "../../tests/ui/inline_code_text.rs"]
mod tests;
