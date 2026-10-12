//! Shift-free streaming reveal policy for Markdown replies.
//!
//! A streamed reply grows by appended text, and rendering every append as-is
//! moves text that is already on screen: a partial word outgrows its line and
//! wraps, an unclosed `**` turns a run bold (wider glyphs, new breaks), and a
//! table or fence reshapes once its closing line arrives. This module decides
//! which prefix of the received source can be shown without any of that ever
//! happening, and steps a revealed prefix through it one visual unit at a
//! time: a whole word, a whole inline construct, or a whole block.
//!
//! GPUI wraps greedily in one forward pass, so a line break never moves once
//! text after it exists. Showing only complete words, and only constructs
//! whose styling can no longer change, keeps every painted glyph where it
//! first appeared. The only movement left is the reply growing at its bottom.
//!
//! [`RevealFade`] carries the fade of freshly revealed units into the
//! renderer. Its offsets address the renderer's visible-text coordinates
//! (leaf text bytes in document order, one unit per atomic block), never
//! source bytes, so they stay valid across prefixes of one body.

use std::ops::Range;

use gpui::HighlightStyle;
use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};

use crate::markdown::PARSE_OPTIONS;

/// Returns the longest prefix of a still-streaming `source` whose rendering
/// can no longer change.
///
/// Held back, each until it can no longer change shape:
///
/// - the trailing word, which may still grow past its line;
/// - an unterminated fence or table, which only appears once complete;
/// - in the trailing open paragraph, everything from the first character that
///   could still open an inline construct (`*`, `_`, `` ` ``, `[`, `<`), and
///   any trailing run of lines containing `|`, which may become a table;
/// - a trailing line holding only block markers (`-`, `1.`, `#`, `>`), which
///   would otherwise paint an empty item or heading.
///
/// A settled body needs none of this: callers reveal it to its full length.
#[must_use]
pub fn stable_reveal_end(source: &str) -> usize {
    // Holds read the text that would actually be shown: dropping a trailing
    // `dog**` reopens the `**` that closed it.
    let mut end = trailing_word_start(source);
    for hold in structural_holds(&source[..end]) {
        end = end.min(hold);
    }
    marker_line_cut(source, end)
}

/// Steps a revealed prefix through one stable source, one visual unit at a
/// time.
///
/// A unit is a whitespace-delimited word plus its trailing whitespace. A
/// word that reaches into an inline construct (code span, link, emphasis) or
/// a block (fence, table, raw HTML) extends to that construct's end, so those
/// appear whole. A unit never ends on a marker-only line, so a list item or
/// heading appears together with its first word.
#[derive(Clone, Debug)]
pub struct RevealSteps<'a> {
    source: &'a str,
    atomic: Vec<Range<usize>>,
}

impl<'a> RevealSteps<'a> {
    /// Indexes the atomic constructs of `source`, which must already be a
    /// stable prefix (see [`stable_reveal_end`]) or a settled body.
    #[must_use]
    pub fn new(source: &'a str) -> Self {
        Self {
            source,
            atomic: atomic_ranges(source),
        }
    }

    /// Returns the end of the unit after `from`; `from` itself at the end.
    #[must_use]
    pub fn next(&self, from: usize) -> usize {
        let len = self.source.len();
        if from >= len {
            return len;
        }
        let mut end = from;
        loop {
            end = self.skip_whitespace(end);
            if end >= len {
                return len;
            }
            end = match self.atomic_containing(end) {
                Some(range) => range.end,
                None => self.skip_word(end),
            };
            // A word that runs into a construct (`foo**bar**`, `see [x](y).`)
            // carries the whole construct, plus anything glued after it.
            while let Some(range) = self.atomic_straddling(end) {
                end = self.skip_word(range.end);
            }
            end = self.skip_whitespace(end);
            if end >= len || !is_marker_only_line(self.source, end) {
                return end.min(len);
            }
        }
    }

    /// Counts the words still to reveal after `from`, for pacing.
    #[must_use]
    pub fn pending_words(&self, from: usize) -> usize {
        self.source
            .get(from..)
            .map_or(0, |rest| rest.split_whitespace().count())
    }

    fn skip_whitespace(&self, mut at: usize) -> usize {
        while let Some(character) = self.source.get(at..).and_then(|rest| rest.chars().next()) {
            if !character.is_whitespace() {
                break;
            }
            at += character.len_utf8();
        }
        at
    }

    fn skip_word(&self, mut at: usize) -> usize {
        while let Some(character) = self.source.get(at..).and_then(|rest| rest.chars().next()) {
            if character.is_whitespace() {
                break;
            }
            if let Some(range) = self.atomic_containing(at)
                && range.start < at
            {
                at = range.end;
                continue;
            }
            at += character.len_utf8();
        }
        at
    }

    /// The outermost construct with `start <= at < end`.
    fn atomic_containing(&self, at: usize) -> Option<&Range<usize>> {
        self.atomic
            .iter()
            .filter(|range| range.start <= at && at < range.end)
            .max_by_key(|range| range.end - range.start)
    }

    /// The outermost construct with `start < at < end`.
    fn atomic_straddling(&self, at: usize) -> Option<&Range<usize>> {
        self.atomic
            .iter()
            .filter(|range| range.start < at && at < range.end)
            .max_by_key(|range| range.end - range.start)
    }
}

/// Per-offset opacity of a revealing body, in the renderer's visible-text
/// coordinates.
///
/// Each mark starts a segment that runs to the next mark; offsets before the
/// first mark, and segments at full opacity, paint normally.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RevealFade {
    marks: Vec<(usize, f32)>,
}

impl RevealFade {
    /// Builds a fade from `(offset, alpha)` marks sorted by offset.
    #[must_use]
    pub fn new(marks: Vec<(usize, f32)>) -> Self {
        Self { marks }
    }

    /// Whether every offset paints at full opacity.
    #[must_use]
    pub fn is_opaque(&self) -> bool {
        self.marks.iter().all(|(_, alpha)| *alpha >= 1.0)
    }

    /// The opacity at one visible offset.
    #[must_use]
    pub fn alpha_at(&self, offset: usize) -> f32 {
        self.marks
            .iter()
            .rev()
            .find(|(start, _)| *start <= offset)
            .map_or(1.0, |(_, alpha)| alpha.clamp(0.0, 1.0))
    }

    /// The translucent segments of one leaf spanning `base..base + len`, as
    /// leaf-local ranges.
    #[must_use]
    pub fn leaf_spans(&self, base: usize, len: usize) -> Vec<(Range<usize>, f32)> {
        let leaf_end = base + len;
        let mut spans = Vec::new();
        for (index, (start, alpha)) in self.marks.iter().enumerate() {
            let end = self
                .marks
                .get(index + 1)
                .map_or(usize::MAX, |(next, _)| *next);
            let start = (*start).max(base);
            let end = end.min(leaf_end);
            if start < end && *alpha < 1.0 {
                spans.push((start - base..end - base, alpha.clamp(0.0, 1.0)));
            }
        }
        spans
    }
}

/// Overlays fade spans onto one leaf's sorted, disjoint highlights.
///
/// Returns sorted, disjoint ranges: every base style survives, gaining a
/// `fade_out` where a span covers it, and uncovered faded text gets a
/// fade-only style. Fading recolors glyphs only, so layout never changes.
#[must_use]
pub fn apply_reveal_fade(
    base: &[(Range<usize>, HighlightStyle)],
    spans: &[(Range<usize>, f32)],
) -> Vec<(Range<usize>, HighlightStyle)> {
    if spans.is_empty() {
        return base.to_vec();
    }
    let mut bounds: Vec<usize> = base
        .iter()
        .flat_map(|(range, _)| [range.start, range.end])
        .chain(spans.iter().flat_map(|(range, _)| [range.start, range.end]))
        .collect();
    bounds.sort_unstable();
    bounds.dedup();
    let mut merged: Vec<(Range<usize>, HighlightStyle)> = Vec::with_capacity(bounds.len());
    for window in bounds.windows(2) {
        let &[start, end] = window else {
            continue;
        };
        let style = base
            .iter()
            .find(|(range, _)| range.start <= start && end <= range.end)
            .map(|(_, style)| *style);
        let alpha = spans
            .iter()
            .find(|(range, _)| range.start <= start && end <= range.end)
            .map(|(_, alpha)| *alpha);
        let style = match (style, alpha) {
            (None, None) => continue,
            (Some(style), None) => style,
            (style, Some(alpha)) => HighlightStyle {
                fade_out: Some(1.0 - alpha),
                ..style.unwrap_or_default()
            },
        };
        match merged.last_mut() {
            Some((range, last)) if range.end == start && *last == style => range.end = end,
            _ => merged.push((start..end, style)),
        }
    }
    merged
}

/// Start of the trailing unterminated word; the length when the source ends
/// in whitespace.
fn trailing_word_start(source: &str) -> usize {
    source
        .char_indices()
        .rev()
        .find(|(_, character)| character.is_whitespace())
        .map_or(0, |(index, character)| index + character.len_utf8())
}

/// Source offsets before which an unfinished construct begins.
fn structural_holds(source: &str) -> Vec<usize> {
    let mut holds = Vec::new();
    // Inline constructs never cross a blank line, so only text after the
    // last one can still be restyled by a closer that has not arrived.
    let open_region = last_blank_line_end(source);
    let mut verbatim_depth = 0_usize;
    let mut opener_found = false;
    for (event, range) in Parser::new_ext(source, PARSE_OPTIONS).into_offset_iter() {
        match event {
            Event::Start(Tag::CodeBlock(kind)) => {
                if matches!(kind, CodeBlockKind::Fenced(_))
                    && !fence_is_closed(source, range.clone())
                {
                    holds.push(line_start(source, range.start));
                }
                verbatim_depth += 1;
            }
            Event::Start(Tag::HtmlBlock) => verbatim_depth += 1,
            Event::End(TagEnd::CodeBlock | TagEnd::HtmlBlock) => {
                verbatim_depth = verbatim_depth.saturating_sub(1);
            }
            Event::Start(Tag::Table(_)) if !block_is_terminated(source, range.end) => {
                holds.push(line_start(source, range.start));
            }
            Event::Text(_)
                if !opener_found && verbatim_depth == 0 && range.start >= open_region =>
            {
                if let Some(opener) = first_inline_opener(source, range) {
                    holds.push(opener);
                    opener_found = true;
                }
            }
            _ => {}
        }
    }
    holds.extend(trailing_pipe_run(source, open_region));
    holds
}

/// The first literal character in one text event that a later closer could
/// still turn into a code span, emphasis, link, or autolink.
fn first_inline_opener(source: &str, range: Range<usize>) -> Option<usize> {
    let text = source.get(range.clone())?;
    let mut previous: Option<char> = source.get(..range.start)?.chars().next_back();
    for (offset, character) in text.char_indices() {
        let after = source
            .get(range.start + offset + character.len_utf8()..)
            .and_then(|rest| rest.chars().next());
        // An emphasis or tag opener needs a non-space after it ("2 * 3" and
        // "a < b" never open), so a lone one does not hold the paragraph.
        let can_open = after.is_none_or(|next| !next.is_whitespace());
        let opener = match character {
            '`' | '[' => true,
            '*' | '<' => can_open,
            // Intraword underscores (`snake_case`) can never open either.
            '_' => can_open && previous.is_none_or(|before| !before.is_alphanumeric()),
            _ => false,
        };
        if opener && previous != Some('\\') {
            return Some(range.start + offset);
        }
        previous = Some(character);
    }
    None
}

/// Start of the trailing run of lines after `region` that contain a pipe: a
/// delimiter row could still turn them into a table.
fn trailing_pipe_run(source: &str, region: usize) -> Option<usize> {
    let open = source.get(region..)?;
    let mut run_start = None;
    let mut line_end = open.trim_end_matches(['\n', '\r']).len();
    loop {
        let start = open[..line_end].rfind('\n').map_or(0, |index| index + 1);
        if !open[start..line_end].contains('|') {
            break;
        }
        run_start = Some(region + start);
        if start == 0 {
            break;
        }
        line_end = start - 1;
    }
    run_start
}

/// Offset just after the last blank line, or zero when there is none.
fn last_blank_line_end(source: &str) -> usize {
    let mut offset = 0;
    let mut previous_blank_end = 0;
    let mut at_start = true;
    for line in source.split_inclusive('\n') {
        let end = offset + line.len();
        if !at_start && line.ends_with('\n') && line.trim().is_empty() {
            previous_blank_end = end;
        }
        at_start = false;
        offset = end;
    }
    previous_blank_end
}

/// Whether a fenced block's closing fence line has fully arrived.
fn fence_is_closed(source: &str, range: Range<usize>) -> bool {
    let block = source.get(range.clone()).unwrap_or_default();
    let Some((opening, rest)) = block.split_once('\n') else {
        return false;
    };
    let opening = opening.trim_start();
    let Some(fence_char) = opening.chars().next() else {
        return false;
    };
    let fence_len = opening.chars().take_while(|c| *c == fence_char).count();
    let trimmed = rest.trim_end_matches(['\n', '\r']);
    let closing = trimmed.rsplit('\n').next().unwrap_or_default().trim();
    let is_fence = closing.chars().count() >= fence_len && closing.chars().all(|c| c == fence_char);
    // `rest` empty means only the opening line arrived.
    if trimmed.is_empty() || !is_fence {
        return false;
    }
    // The closing line must be terminated: a trailing ``` could still grow
    // into an info string or an opening of the next fence.
    let closing_end = range.start + block.len() - (rest.len() - trimmed.len());
    source
        .get(closing_end..)
        .is_some_and(|tail| tail.starts_with('\n'))
}

/// Whether a block ending at `end` has been closed: by a blank line, or by
/// content on a later line that the parser did not fold into it.
///
/// The parser may or may not count the block's last newline inside its
/// range, so a newline just before `end` counts too.
fn block_is_terminated(source: &str, end: usize) -> bool {
    let mut newlines = usize::from(source.get(..end).is_some_and(|head| head.ends_with('\n')));
    for character in source.get(end..).unwrap_or_default().chars() {
        match character {
            '\n' => newlines += 1,
            ' ' | '\t' | '\r' => {}
            _ => return newlines > 0,
        }
    }
    newlines >= 2
}

/// Start of the line containing `offset`.
fn line_start(source: &str, offset: usize) -> usize {
    source
        .get(..offset)
        .and_then(|head| head.rfind('\n'))
        .map_or(0, |index| index + 1)
}

/// Cuts `end` back to its line start when the line so far holds only block
/// markers.
fn marker_line_cut(source: &str, end: usize) -> usize {
    if is_marker_only_line(source, end) {
        line_start(source, end)
    } else {
        end
    }
}

/// Whether the line containing `end`, up to `end`, is non-empty and holds
/// only list, heading, quote, task, or rule markers.
fn is_marker_only_line(source: &str, end: usize) -> bool {
    let start = line_start(source, end);
    let Some(line) = source.get(start..end) else {
        return false;
    };
    let mut tokens = line.split_whitespace().peekable();
    if tokens.peek().is_none() {
        return false;
    }
    tokens.all(is_marker_token)
}

fn is_marker_token(token: &str) -> bool {
    let all = |marker: char| token.chars().all(|c| c == marker);
    if all('#') || all('-') || all('*') || all('+') || all('>') || all('=') || all('_') {
        return true;
    }
    if matches!(token, "[" | "]" | "[ ]" | "[x]" | "[X]" | "x]" | "X]") {
        return true;
    }
    let digits = token.trim_end_matches(['.', ')']);
    digits.len() < token.len() && !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
}

/// Source ranges that must appear whole: blocks whose shape is only final
/// once complete, and inline constructs whose styling spans several words.
fn atomic_ranges(source: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    for (event, range) in Parser::new_ext(source, PARSE_OPTIONS).into_offset_iter() {
        match event {
            Event::Start(
                Tag::CodeBlock(_)
                | Tag::Table(_)
                | Tag::HtmlBlock
                | Tag::Link { .. }
                | Tag::Image { .. }
                | Tag::Strong
                | Tag::Emphasis,
            )
            | Event::Code(_)
            | Event::InlineHtml(_) => ranges.push(range),
            _ => {}
        }
    }
    ranges
}

#[cfg(test)]
#[path = "markdown_reveal_tests.rs"]
mod tests;
