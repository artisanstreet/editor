//! Inline prose fragments for model reasoning summaries.
//!
//! Native port of `conversation_summary_fragments` (plus the headline and
//! first-sentence reduction of `conversation_summary_line`) from
//! `modules/frontend/src/lib/conversation/trace.ts`, rendered the way
//! `modules/frontend/src/lib/components/inline-code-text.svelte` paints them:
//! backticked runs in the mono face, matched `**`/`__`/`*`/`_`/`~~` pairs in
//! their weight and posture, block furniture stripped, unclosed marks left
//! literal, and no colour changes — tone always inherits the parent.
//!
//! Two deliberate fork boundaries, documented here rather than hidden:
//!
//! - Code faces ride the frozen shared text-runs compiler (see below); the
//!   reference `0.9em` relative sizing has no per-range GPUI equivalent, so
//!   code keeps the inherited size (honest, documented).
//! - Strong maps to the shared [`ProseTypography::STRONG_WEIGHT`] static
//!   (630, served by the vendored face — never a variable-weight claim),
//!   emphasis to italic, strike to a 1px line-through. No variable-weight
//!   claims: shared variable typography belongs to its owning lane.
//! - Code faces ride the frozen shared text-runs compiler
//!   (`docs/reference-text-runs-interface.md`, worker wt-reference-text-runs):
//!   exact mono family with zero tracking via overrides. The only known
//!   sizing gap is the reference `0.9em` relative code size, which has no
//!   per-range GPUI equivalent, so code keeps the inherited size.

use std::ops::Range;

use gpui::{
    AnyElement, FontStyle, FontWeight, HighlightStyle, IntoElement, SharedString,
    StrikethroughStyle, Styled, div, px,
};

use crate::selectable_text::{SelectableText, TextRunOverride};
use crate::theme::{ArtisanTheme, ProseTypography};

/// One marked run of model prose: plain, emphasised, struck, or code text.
///
/// Ranges are subsumed into owned text because callers concatenate fragments
/// before painting; byte offsets below always address that concatenation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InlineFragment {
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

/// Splits one line of model prose into plain, emphasised, and code runs.
///
/// Mirrors `conversation_summary_fragments`: block furniture a one-line
/// rendering cannot honour (headings, quote marks, list markers) is stripped
/// per line; inline code spans split first with an unclosed backtick
/// treating the rest as arriving code; emphasis pairs resolve longest-first
/// with word-edge rules for underscores. Only pairs count: a mark whose
/// partner never arrives stays literal.
#[must_use]
pub fn inline_fragments(raw: &str) -> Vec<InlineFragment> {
    let stripped = strip_block_marks(raw);
    let mut fragments = Vec::new();
    let mut consumed = 0_usize;
    let bytes = stripped.as_bytes();
    let mut index = 0_usize;
    while index < bytes.len() {
        if bytes[index] != b'`' {
            index += 1;
            continue;
        }
        let after = index + 1;
        let mut close = None;
        let mut scan = after;
        while scan < bytes.len() {
            if bytes[scan] == b'\n' {
                break;
            }
            if bytes[scan] == b'`' {
                close = Some(scan);
                break;
            }
            scan += 1;
        }
        let Some(close) = close else {
            // Unclosed backtick: the rest is arriving code, never a bare mark.
            push_prose(&mut fragments, &stripped[consumed..index]);
            let arriving = &stripped[after..];
            if !arriving.is_empty() {
                fragments.push(InlineFragment {
                    text: arriving.to_owned(),
                    code: true,
                    strong: false,
                    em: false,
                    strike: false,
                });
            }
            consumed = bytes.len();
            break;
        };
        push_prose(&mut fragments, &stripped[consumed..index]);
        fragments.push(InlineFragment {
            text: stripped[after..close].to_owned(),
            code: true,
            strong: false,
            em: false,
            strike: false,
        });
        consumed = close + 1;
        index = close + 1;
    }
    push_prose(&mut fragments, &stripped[consumed..]);
    fragments
}

/// Flattens fragments to plain visible text for sweep renderers that own
/// their runs (such as shimmer text): marks stay removed, faces do not
/// transfer.
#[must_use]
pub fn flatten_fragments(fragments: &[InlineFragment]) -> String {
    let mut text = String::new();
    for fragment in fragments {
        text.push_str(&fragment.text);
    }
    text
}

/// Reduces one reasoning body to the single line the thinking line says.
///
/// Mirrors `conversation_summary_line`: the newest `**headline**` wins over
/// any paragraph beneath it, otherwise the newest paragraph's first finished
/// sentence stands in with whitespace collapsed. Only a finished sentence
/// may take the line; unfinished phases yield `None` so the caller keeps the
/// previous settled value instead of rewriting mid-thought.
#[must_use]
pub fn summary_line(text: &str) -> Option<String> {
    let sections = split_sections(text);
    for section in sections.iter().rev() {
        if let Some(headline) = parse_headline(section) {
            if !headline.is_empty() {
                return Some(headline);
            }
        }
    }
    for section in sections.iter().rev() {
        let unstarred = section.replace("**", "");
        if let Some(sentence) = first_sentence(&unstarred) {
            let line: String = sentence
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            if !line.is_empty() {
                return Some(line);
            }
        }
    }
    None
}

/// Builds the paint inputs for one fragment list: flattened text, one
/// highlight list, and the code ranges.
///
/// Every code range carries an explicit 400-weight highlight even though the
/// inherited body weight is 410: the shared text-runs compiler resolves
/// family and spacing overrides against these same ranges, and the weight
/// keeps code on the vendored 400 static. Ranges come out sorted,
/// non-overlapping, and on character boundaries, exactly as both GPUI calls
/// require.
#[must_use]
pub fn fragment_runs(
    fragments: &[InlineFragment],
) -> (
    String,
    Vec<(Range<usize>, HighlightStyle)>,
    Vec<Range<usize>>,
) {
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

/// Renders fragments through the shared text-runs compiler contract: exact
/// mono family with zero tracking on code ranges, shared 630 strong,
/// italic, line-through, tone and size inherited so the line keeps its
/// surface. Selection, copy, and link behavior ride the retained
/// selectable element exactly like every other transcript leaf.
#[must_use]
pub fn render_inline_fragments(
    fragments: &[InlineFragment],
    theme: ArtisanTheme,
    selector: impl Into<SharedString>,
) -> AnyElement {
    let (flat, highlights, code_ranges) = fragment_runs(fragments);
    let mono = SharedString::from(theme.typography.mono.family);
    let overrides = code_ranges
        .into_iter()
        .map(|range| TextRunOverride {
            range,
            font_family: Some(mono.clone()),
            letter_spacing: Some(px(0.0)),
        })
        .collect::<Vec<_>>();
    let selector = selector.into();
    let element_id = selector.clone();
    div()
        .debug_selector(move || selector.clone())
        .child(
            SelectableText::retained(element_id, flat, theme, highlights)
                .with_text_run_overrides(overrides),
        )
        .into_any_element()
}

/// Renders raw model prose as inline fragments in one call.
#[must_use]
pub fn render_inline_text(
    text: &str,
    theme: ArtisanTheme,
    selector: impl Into<SharedString>,
) -> AnyElement {
    render_inline_fragments(&inline_fragments(text), theme, selector)
}

fn push_prose(fragments: &mut Vec<InlineFragment>, prose: &str) {
    if prose.is_empty() {
        return;
    }
    for fragment in emphasis_fragments(prose, false, false, false) {
        if !fragment.text.is_empty() {
            fragments.push(fragment);
        }
    }
}

/// Block furniture a one-line rendering has no block to hang on, stripped
/// per line with bounded repetition (a quote can hold a list).
fn strip_block_marks(raw: &str) -> String {
    raw.split('\n')
        .map(|line| {
            let mut line = line;
            for _ in 0..8 {
                let Some(stripped) = strip_one_mark(line) else {
                    break;
                };
                line = stripped;
            }
            line.to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn strip_one_mark(line: &str) -> Option<&str> {
    let trimmed = line.trim_start_matches([' ', '\t']);
    let bytes = trimmed.as_bytes();
    // Headings: 1-6 '#' plus blank.
    let mut hashes = 0_usize;
    while hashes < bytes.len() && hashes < 6 && bytes[hashes] == b'#' {
        hashes += 1;
    }
    if hashes > 0
        && hashes < bytes.len()
        && (bytes[hashes] == b' ' || bytes[hashes] == b'\t')
    {
        return Some(trimmed[hashes..].trim_start_matches([' ', '\t']));
    }
    // Quotes.
    if !bytes.is_empty()
        && bytes[0] == b'>'
        && bytes.len() > 1
        && (bytes[1] == b' ' || bytes[1] == b'\t')
    {
        return Some(trimmed[1..].trim_start_matches([' ', '\t']));
    }
    // List markers: 1-2 of '-', '*', '+' plus blank.
    let mut bullets = 0_usize;
    while bullets < bytes.len()
        && bullets < 2
        && (bytes[bullets] == b'-' || bytes[bullets] == b'*' || bytes[bullets] == b'+')
    {
        bullets += 1;
    }
    if bullets > 0
        && bullets < bytes.len()
        && (bytes[bullets] == b' ' || bytes[bullets] == b'\t')
    {
        return Some(trimmed[bullets..].trim_start_matches([' ', '\t']));
    }
    // Ordered markers: 1-3 digits plus '.' or ')' plus blank.
    let mut digits = 0_usize;
    while digits < bytes.len() && digits < 3 && bytes[digits].is_ascii_digit() {
        digits += 1;
    }
    if digits > 0
        && digits + 1 < bytes.len()
        && (bytes[digits] == b'.' || bytes[digits] == b')')
        && (bytes[digits + 1] == b' ' || bytes[digits + 1] == b'\t')
    {
        return Some(trimmed[digits + 1..].trim_start_matches([' ', '\t']));
    }
    None
}

struct EmphasisMark {
    flag: EmphasisFlag,
    mark: &'static str,
}

#[derive(Clone, Copy)]
enum EmphasisFlag {
    Strong,
    Em,
    Strike,
}

/// Longest marks first, so `**` is never read as two `*`.
const EMPHASIS_MARKS: [EmphasisMark; 5] = [
    EmphasisMark {
        flag: EmphasisFlag::Strong,
        mark: "**",
    },
    EmphasisMark {
        flag: EmphasisFlag::Strong,
        mark: "__",
    },
    EmphasisMark {
        flag: EmphasisFlag::Strike,
        mark: "~~",
    },
    EmphasisMark {
        flag: EmphasisFlag::Em,
        mark: "*",
    },
    EmphasisMark {
        flag: EmphasisFlag::Em,
        mark: "_",
    },
];

fn is_word_char(value: char) -> bool {
    // Approximation of the reference `[\p{L}\p{N}]` over the local text.
    value.is_alphanumeric()
}

/// Whether `mark` opens at `index`. Callers pass a character boundary;
/// marks themselves are ASCII, so every derived offset stays aligned.
fn opens_emphasis(text: &str, index: usize, mark: &str) -> bool {
    let after = text[index + mark.len()..].chars().next();
    let Some(next) = after else {
        return false;
    };
    if next.is_whitespace() {
        return false;
    }
    if !mark.starts_with('_') {
        return true;
    }
    match text[..index].chars().next_back() {
        None => true,
        Some(previous) => !is_word_char(previous),
    }
}

/// Whether `mark` closes at `index`. Same boundary contract as
/// [`opens_emphasis`].
fn closes_emphasis(text: &str, index: usize, mark: &str) -> bool {
    let previous = text[..index].chars().next_back();
    let Some(previous) = previous else {
        return false;
    };
    if previous.is_whitespace() {
        return false;
    }
    if !mark.starts_with('_') {
        return true;
    }
    match text[index + mark.len()..].chars().next() {
        None => true,
        Some(next) => !is_word_char(next),
    }
}

fn find_emphasis_close(text: &str, from: usize, mark: &str) -> Option<usize> {
    // Char-stepped scan from one past the opener end, mirroring the source
    // loop while never splitting a boundary: `from` is a boundary and marks
    // are ASCII, so stepping by character keeps every probe index aligned.
    let mut index = from.saturating_add(1);
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    while index + mark.len() <= text.len() {
        debug_assert!(text.is_char_boundary(index));
        if text[index..].starts_with(mark) && closes_emphasis(text, index, mark) {
            return Some(index);
        }
        index += text[index..].chars().next().map_or(1, |next| next.len_utf8());
    }
    None
}

fn emphasis_fragments(text: &str, strong: bool, em: bool, strike: bool) -> Vec<InlineFragment> {
    let mut fragments = Vec::new();
    let mut plain_start: Option<usize> = None;
    // `index` always rests on a character boundary: it starts at zero and
    // advances by whole characters or whole ASCII marks, so every slice and
    // probe below is aligned no matter the script.
    let mut index = 0_usize;
    while index < text.len() {
        debug_assert!(text.is_char_boundary(index));
        let mut matched = false;
        for candidate in &EMPHASIS_MARKS {
            let mark = candidate.mark;
            if !text[index..].starts_with(mark) || !opens_emphasis(text, index, mark) {
                continue;
            }
            let Some(close) = find_emphasis_close(text, index + mark.len(), mark) else {
                continue;
            };
            if let Some(start) = plain_start.take() {
                let plain = &text[start..index];
                if !plain.is_empty() {
                    fragments.push(InlineFragment {
                        text: plain.to_owned(),
                        code: false,
                        strong,
                        em,
                        strike,
                    });
                }
            }
            let (next_strong, next_em, next_strike) = match candidate.flag {
                EmphasisFlag::Strong => (true, em, strike),
                EmphasisFlag::Em => (strong, true, strike),
                EmphasisFlag::Strike => (strong, em, true),
            };
            fragments.extend(emphasis_fragments(
                &text[index + mark.len()..close],
                next_strong,
                next_em,
                next_strike,
            ));
            index = close + mark.len();
            matched = true;
            break;
        }
        if matched {
            continue;
        }
        if plain_start.is_none() {
            plain_start = Some(index);
        }
        index += text[index..].chars().next().map_or(1, |next| next.len_utf8());
    }
    if let Some(start) = plain_start {
        let plain = &text[start..];
        if !plain.is_empty() {
            fragments.push(InlineFragment {
                text: plain.to_owned(),
                code: false,
                strong,
                em,
                strike,
            });
        }
    }
    fragments
    .into_iter()
    .filter(|fragment| !fragment.text.is_empty())
    .collect()
}

/// Splits sections on blank lines, trims, and drops empties.
///
/// Owns its strings: sections join borrowed lines into new allocations, so
/// callers never hold slices of a temporary.
fn split_sections(text: &str) -> Vec<String> {
    let mut sections = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in text.split('\n') {
        if line.trim().is_empty() {
            if !current.is_empty() {
                sections.push(current.join("\n").trim().to_owned());
                current = Vec::new();
            }
            continue;
        }
        current.push(line);
    }
    if !current.is_empty() {
        sections.push(current.join("\n").trim().to_owned());
    }
    sections.into_iter().filter(|section| !section.is_empty()).collect()
}

/// Latest `**headline**` inner text, or `None`. The close must sit on the
/// same line the opener started, matching the source single-line grammar.
fn parse_headline(section: &str) -> Option<String> {
    let body = section.strip_prefix("**")?;
    let close = body.find("**")?;
    let inner = body[..close].trim();
    if inner.is_empty() || inner.contains('\n') {
        return None;
    }
    Some(inner.to_owned())
}

/// First `.!?`-terminated sentence (`**` already removed by the caller).
fn first_sentence(section: &str) -> Option<&str> {
    let bytes = section.as_bytes();
    let mut index = 0_usize;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'.' || byte == b'!' || byte == b'?' {
            let rest = &section[index + 1..];
            if rest.is_empty() || rest.starts_with([' ', '\t', '\n', '\r']) {
                return Some(&section[..=index]);
            }
        }
        index += 1;
    }
    None
}
