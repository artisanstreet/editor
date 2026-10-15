//! Compact styled summaries parsed with the same CommonMark grammar as full prose.

use std::ops::Range;

use gpui::{FontStyle, FontWeight, HighlightStyle, SharedString, StrikethroughStyle, px};

use crate::text_runs::TextRunOverride;
use crate::theme::{ArtisanTheme, ProseTypography};

/// One marked run of model prose: plain, emphasised, struck, or code text.
///
/// Ranges are subsumed into owned text because callers concatenate fragments
/// before painting; byte offsets below always address that concatenation.
#[derive(Clone, Debug, Eq, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Markdown styles combine independently"
)]
struct InlineFragment {
    /// Visible text with all marks removed.
    pub text: String,
    /// Backticked run: mono face.
    pub code: bool,
    /// `**`/`__` pair: shared strong face.
    pub strong: bool,
    /// `*`/`_` pair: italic.
    pub em: bool,
    /// `~~` pair: line-through.
    pub strike: bool,
}

/// CommonMark inline content flattened into styled runs; block markers are omitted.
#[must_use]
fn inline_fragments(raw: &str) -> Vec<InlineFragment> {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};
    let mut fragments = Vec::<InlineFragment>::new();
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
        let mut fragment = InlineFragment {
            text,
            code,
            strong: strong > 0,
            em: em > 0,
            strike: strike > 0,
        };
        if let Some(previous) = fragments.last_mut() {
            if (previous.code, previous.strong, previous.em, previous.strike)
                == (fragment.code, fragment.strong, fragment.em, fragment.strike)
            {
                previous.text.push_str(&fragment.text);
                continue;
            }
        } else {
            fragment.text = fragment.text.trim_start().to_owned();
        }
        if !fragment.text.is_empty() {
            fragments.push(fragment);
        }
    }
    if let Some(last) = fragments.last_mut() {
        last.text.truncate(last.text.trim_end().len());
    }
    fragments.retain(|fragment| !fragment.text.is_empty());
    fragments
}

type FragmentRuns = (
    String,
    Vec<(Range<usize>, HighlightStyle)>,
    Vec<Range<usize>>,
);

#[must_use]
fn fragment_runs(fragments: &[InlineFragment]) -> FragmentRuns {
    let mut flat = String::new();
    let mut highlights: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    let mut code_ranges: Vec<Range<usize>> = Vec::new();
    for fragment in fragments {
        if fragment.text.is_empty() {
            continue;
        }
        let start = flat.len();
        flat.push_str(&fragment.text);
        let range = start..flat.len();
        if fragment.code {
            highlights.push((
                range.clone(),
                HighlightStyle {
                    font_weight: Some(FontWeight::NORMAL),
                    ..HighlightStyle::default()
                },
            ));
            code_ranges.push(range);
            continue;
        }
        let mut style = HighlightStyle::default();
        let mut styled = false;
        if fragment.strong {
            style.font_weight = Some(ProseTypography::STRONG_WEIGHT);
            styled = true;
        }
        if fragment.em {
            style.font_style = Some(FontStyle::Italic);
            styled = true;
        }
        if fragment.strike {
            style.strikethrough = Some(StrikethroughStyle {
                thickness: px(1.0),
                color: None,
            });
            styled = true;
        }
        if styled {
            highlights.push((range, style));
        }
    }
    (flat, highlights, code_ranges)
}

/// Owned paint inputs for one inline summary: flattened text plus the exact
/// highlight and compiler override ranges addressing it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InlineRuns {
    /// Visible text with all marks removed.
    pub text: String,
    /// Weight/style/strike runs (code ranges carry the 400 static).
    pub highlights: Vec<(Range<usize>, HighlightStyle)>,
    /// Frozen mono family with zero tracking for code ranges, compiled at
    /// layout by the shared text-runs contract.
    pub overrides: Vec<TextRunOverride>,
}

/// Compiles raw model prose into paint inputs: fragments, flattened text,
/// highlight runs, and compiler overrides, all addressing the same bytes.
#[must_use]
pub fn inline_runs(text: &str, theme: ArtisanTheme) -> InlineRuns {
    let (flat, highlights, code_ranges) = fragment_runs(&inline_fragments(text));
    let mono = SharedString::from(theme.typography.mono.family);
    let overrides = code_ranges
        .into_iter()
        .map(|range| TextRunOverride {
            range,
            font_family: Some(mono.clone()),
            letter_spacing: Some(px(0.0)),
        })
        .collect();
    InlineRuns {
        text: flat,
        highlights,
        overrides,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_summary_uses_commonmark_code_escapes_and_nested_styles() {
        let theme = ArtisanTheme::default();
        let runs = inline_runs(
            r"Use ``a ` b`` and **bold _nested_** \*literal\* [label](https://example.com)",
            theme,
        );
        assert_eq!(runs.text, "Use a ` b and bold nested *literal* label");
        assert_eq!(runs.overrides.len(), 1);
        assert_eq!(&runs.text[runs.overrides[0].range.clone()], "a ` b");
        assert_eq!(
            runs.overrides[0].font_family.as_deref(),
            Some(theme.typography.mono.family)
        );
        assert!(runs.highlights.iter().any(|(range, style)| {
            &runs.text[range.clone()] == "nested"
                && style.font_weight == Some(ProseTypography::STRONG_WEIGHT)
                && style.font_style == Some(FontStyle::Italic)
        }));
    }
}
