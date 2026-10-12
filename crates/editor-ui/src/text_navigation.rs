//! Chrome-parity text navigation shared by every text surface.
//!
//! Word movement (`ctrl`/`alt` + arrows), word deletion, and double/triple
//! click units follow Blink's editing code rather than an ad-hoc character
//! class, so muscle memory from the browser carries over:
//!
//! - Word boundaries come from the Unicode word segmentation (UAX #29) that
//!   Blink gets from ICU, with Chromium's own rule patch applied
//!   (`chromium/deps/icu` `source/data/brkitr/rules/word.txt`): `.` joins
//!   digits (`32.3`) but never letters (`abc.def`), and `:` never joins.
//! - [`next_word_position`] and [`previous_word_position`] port
//!   `NextWordPositionInternal` / `PreviousWordPositionInternal` from
//!   `third_party/blink/renderer/core/editing/position_units_word.cc`,
//!   including punctuation runs and the Windows rule that moving right skips
//!   the spaces after a word (`EditingBehavior::ShouldSkipSpaceWhenMovingRight`).
//! - [`double_click_range`] is `SelectClosestWordFromHitTestResult`: the
//!   word segment under the pointer, plus trailing spaces where Chrome sets
//!   `selectTrailingWhitespaceEnabled` (Windows).
//! - [`paragraph_range_at`] is triple-click's paragraph granularity,
//!   including the paragraph break.
//! - [`caret_index_for_position`] places a caret at the character boundary
//!   nearest the pointer, as Blink's hit testing does, instead of the start
//!   of the glyph under it.
//!
//! All offsets are UTF-8 byte offsets on character boundaries.

use std::ops::Range;
use std::sync::OnceLock;

use gpui::{Pixels, Point, TextLayout};
use unicode_segmentation::UnicodeSegmentation as _;

/// Which platform convention word movement follows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WordBehavior {
    /// Windows: moving right lands at the start of the next word, and a
    /// double-click also selects the spaces after the word.
    SkipSpaces,
    /// macOS and Linux: moving right stops at the end of the word.
    StopAtWordEnd,
}

impl WordBehavior {
    /// The convention of the desktop the user is on.
    ///
    /// Under WSL the Editor is a Linux process on a Windows desktop, where
    /// every other app (Chrome included) follows the Windows convention.
    #[must_use]
    pub fn platform() -> Self {
        static BEHAVIOR: OnceLock<WordBehavior> = OnceLock::new();
        *BEHAVIOR.get_or_init(|| {
            let windows_desktop = cfg!(target_os = "windows")
                || (cfg!(target_os = "linux")
                    && (std::env::var_os("WSL_DISTRO_NAME").is_some()
                        || std::env::var_os("WSL_INTEROP").is_some()));
            if windows_desktop {
                Self::SkipSpaces
            } else {
                Self::StopAtWordEnd
            }
        })
    }
}

fn is_line_break(character: char) -> bool {
    matches!(character, '\n' | '\r')
}

/// Blink's `IsWhitespace`: only these four count, unlike `char::is_whitespace`.
fn is_blink_whitespace(character: char) -> bool {
    matches!(character, '\u{00a0}' | ' ' | '\n' | '\t')
}

/// Characters outside the Basic Multilingual Plane, which Blink sees as
/// surrogate code units.
fn is_astral(character: char) -> bool {
    u32::from(character) > 0xffff
}

/// Blink's `IsWordBreak`: a character that can end or start a word stop.
fn is_word_break(character: char) -> bool {
    (!character.is_control() && !is_blink_whitespace(character))
        || is_astral(character)
        || is_line_break(character)
        || character == '_'
}

/// Blink's `IsWordBoundary`: punctuation, symbols, and astral characters.
fn is_word_boundary(character: char) -> bool {
    is_astral(character)
        || (!character.is_alphanumeric() && !character.is_whitespace() && !character.is_control())
}

fn char_at(text: &str, offset: usize) -> Option<char> {
    text.get(offset..)?.chars().next()
}

fn char_before(text: &str, offset: usize) -> Option<char> {
    text.get(..offset)?.chars().next_back()
}

/// Whether Chromium's word rules refuse to join across `character`.
fn splits_word(text: &str, offset: usize, character: char) -> bool {
    match character {
        // Removed from MidLetter by Chromium's patch.
        ':' | '\u{fe55}' | '\u{ff1a}' => true,
        // Moved from MidNumLet to MidNum: joins digits only.
        '.' | '\u{ff0e}' => {
            !(char_before(text, offset).is_some_and(char::is_numeric)
                && char_at(text, offset + character.len_utf8()).is_some_and(char::is_numeric))
        }
        _ => false,
    }
}

/// Every word-break offset of `text`, ascending, including `0` and the end.
fn word_breaks(text: &str) -> Vec<usize> {
    let mut breaks = vec![0];
    for (start, segment) in text.split_word_bound_indices() {
        for (index, character) in segment.char_indices().skip(1) {
            let offset = start + index;
            if splits_word(text, offset, character) {
                breaks.push(offset);
                breaks.push(offset + character.len_utf8());
            }
        }
        breaks.push(start + segment.len());
    }
    breaks.sort_unstable();
    breaks.dedup();
    breaks
}

fn following(breaks: &[usize], offset: usize) -> Option<usize> {
    breaks.iter().copied().find(|&value| value > offset)
}

fn preceding(breaks: &[usize], offset: usize) -> Option<usize> {
    breaks.iter().rev().copied().find(|&value| value < offset)
}

/// Windows skips the spaces after a word stop, but never a line break.
fn skip_whitespace(text: &str, offset: usize, behavior: WordBehavior) -> usize {
    if behavior != WordBehavior::SkipSpaces {
        return offset;
    }
    text[offset..]
        .char_indices()
        .find(|(_, character)| !character.is_whitespace() || is_line_break(*character))
        .map_or(text.len(), |(index, _)| offset + index)
}

/// Where `ctrl`+Right (`alt`+Right on macOS) moves the caret from `offset`.
#[must_use]
pub fn next_word_position(text: &str, offset: usize, behavior: WordBehavior) -> usize {
    let offset = floor_char_boundary(text, offset);
    if offset >= text.len() {
        return text.len();
    }
    let breaks = word_breaks(text);
    let skips = behavior == WordBehavior::SkipSpaces;
    let mut punctuation: Option<usize> = None;
    let mut runner = following(&breaks, offset);
    while let Some(at) = runner {
        let next = char_at(text, at);
        let previous = char_before(text, at);
        let pending = if skips { punctuation } else { None };
        if next.is_some_and(is_line_break) {
            return skip_whitespace(text, pending.unwrap_or(at), behavior);
        }
        if let Some(next) = next
            && is_word_boundary(next)
        {
            if previous.is_some_and(char::is_alphanumeric) {
                return skip_whitespace(text, at, behavior);
            }
            if skips && punctuation.is_none() && previous.is_some_and(is_blink_whitespace) {
                punctuation = Some(at);
            }
            runner = following(&breaks, at);
            continue;
        }
        if let Some(pending) = pending {
            return skip_whitespace(text, pending, behavior);
        }
        if next.is_some() {
            if previous.is_some_and(is_word_break) {
                return skip_whitespace(text, at, behavior);
            }
            if skips && previous.is_some_and(is_blink_whitespace) && next.is_some_and(is_word_break)
            {
                return skip_whitespace(text, at, behavior);
            }
        }
        runner = following(&breaks, at);
    }
    text.len()
}

/// Where `ctrl`+Left (`alt`+Left on macOS) moves the caret from `offset`.
#[must_use]
pub fn previous_word_position(text: &str, offset: usize) -> usize {
    let offset = floor_char_boundary(text, offset);
    if offset == 0 {
        return 0;
    }
    let breaks = word_breaks(text);
    let mut punctuation: Option<usize> = None;
    let mut runner = preceding(&breaks, offset);
    while let Some(at) = runner {
        let next = char_at(text, at);
        if let Some(next) = next
            && is_word_boundary(next)
        {
            if char_before(text, at).is_some_and(char::is_alphanumeric) {
                return at;
            }
            punctuation = Some(at);
            runner = preceding(&breaks, at);
            continue;
        }
        if let Some(punctuation) = punctuation {
            return punctuation;
        }
        if next.is_some_and(is_word_break) {
            return at;
        }
        runner = preceding(&breaks, at);
    }
    0
}

/// The word segment a double-click at `offset` selects, without trailing
/// whitespace: the segment holding the character at `offset`, or the one
/// before it at the end of a line or of the text.
#[must_use]
pub fn word_range_at(text: &str, offset: usize) -> Range<usize> {
    let offset = floor_char_boundary(text, offset);
    let breaks = word_breaks(text);
    let at_line_end = char_at(text, offset).is_none_or(is_line_break);
    if at_line_end {
        let start = preceding(&breaks, offset).unwrap_or(offset);
        // A line break is never a word of its own.
        if char_at(text, start).is_some_and(is_line_break) {
            return offset..offset;
        }
        return start..offset;
    }
    let start = breaks
        .iter()
        .rev()
        .copied()
        .find(|&value| value <= offset)
        .unwrap_or(0);
    let end = following(&breaks, offset).unwrap_or(text.len());
    start..end
}

/// What a double-click at `offset` selects: [`word_range_at`], extended
/// over the following spaces (not line breaks) on Windows.
#[must_use]
pub fn double_click_range(text: &str, offset: usize, behavior: WordBehavior) -> Range<usize> {
    let word = word_range_at(text, offset);
    if word.is_empty() {
        return word;
    }
    word.start..skip_whitespace(text, word.end, behavior)
}

/// What a triple-click at `offset` selects: the line between line breaks,
/// including the line break that ends it.
#[must_use]
pub fn paragraph_range_at(text: &str, offset: usize) -> Range<usize> {
    let offset = floor_char_boundary(text, offset);
    let start = text[..offset].rfind('\n').map_or(0, |index| index + 1);
    let end = text[offset..]
        .find('\n')
        .map_or(text.len(), |index| offset + index + 1);
    start..end
}

/// The unit a mouse selection was started with: Chrome selects by the
/// click count of the press that started it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SelectionUnit {
    /// A single click: caret placement, drag by character.
    #[default]
    Character,
    /// A double click: select and drag by word.
    Word,
    /// A triple (or further) click: select and drag by paragraph.
    Paragraph,
}

impl SelectionUnit {
    /// The unit of a click with GPUI's `click_count`.
    #[must_use]
    pub fn from_click_count(click_count: usize) -> Self {
        match click_count {
            0 | 1 => Self::Character,
            2 => Self::Word,
            _ => Self::Paragraph,
        }
    }

    /// What the click itself selects around `offset`: the caret, the word
    /// a double-click selects (with Windows' trailing spaces), or the
    /// paragraph a triple-click selects.
    #[must_use]
    pub fn range_at(self, text: &str, offset: usize) -> Range<usize> {
        match self {
            Self::Character => offset..offset,
            Self::Word => {
                let word = double_click_range(text, offset, WordBehavior::platform());
                if word.is_empty() {
                    offset..offset
                } else {
                    word
                }
            }
            Self::Paragraph => paragraph_range_at(text, offset),
        }
    }

    /// The unit a drag reaches at `offset`. Dragging extends by bare words,
    /// without trailing spaces, as Blink's word granularity does.
    #[must_use]
    pub fn drag_range_at(self, text: &str, offset: usize) -> Range<usize> {
        match self {
            Self::Word => {
                let word = word_range_at(text, offset);
                if word.is_empty() {
                    offset..offset
                } else {
                    word
                }
            }
            Self::Character | Self::Paragraph => self.range_at(text, offset),
        }
    }

    /// The selection a drag to `offset` makes when the press selected
    /// `origin`: the union of `origin` and the unit at `offset`, and
    /// whether it runs backwards (the pointer is before `origin`).
    #[must_use]
    pub fn extend(self, text: &str, origin: &Range<usize>, offset: usize) -> (Range<usize>, bool) {
        let target = self.drag_range_at(text, offset);
        if target.start < origin.start {
            (target.start..origin.end, true)
        } else {
            (origin.start..target.end.max(origin.end), false)
        }
    }
}

/// The nearest character boundary at or before `offset`.
#[must_use]
pub fn floor_char_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

/// The caret offset nearest to `position` in a painted text layout.
///
/// `TextLayout::index_for_position` returns the glyph *under* the pointer,
/// so a click on the right half of a letter would land before it. This
/// resolves each visual row with GPUI's closest-boundary lookup instead.
/// Points above the text map to `0`, below it to the end; points beside a
/// row map to that row's start or end.
#[must_use]
pub fn caret_index_for_position(layout: &TextLayout, position: Point<Pixels>) -> usize {
    let bounds = layout.bounds();
    let line_height = layout.line_height();
    let text_len = layout.len();
    if position.y < bounds.top() {
        return 0;
    }
    let mut line_top = bounds.top();
    let mut line_start = 0usize;
    for line in layout.line_layouts() {
        let rows = line.wrap_boundaries().len() + 1;
        #[expect(
            clippy::cast_precision_loss,
            reason = "a line's visual row count is far below f32's exact integer range"
        )]
        let line_bottom = line_top + line_height * rows as f32;
        if position.y < line_bottom {
            let local = gpui::point(position.x - bounds.left(), position.y - line_top);
            let index = match line.closest_index_for_position(local, line_height) {
                Ok(index) | Err(index) => index,
            };
            return (line_start + index).min(text_len);
        }
        line_top = line_bottom;
        line_start += line.len() + 1;
    }
    text_len
}

#[cfg(test)]
mod tests {
    use super::{
        SelectionUnit, WordBehavior, double_click_range, next_word_position, paragraph_range_at,
        previous_word_position, word_range_at,
    };

    /// Runs `step` on the `|` caret of `marked` and returns the text with
    /// the resulting caret marked, like Blink's `visible_units_word_test.cc`.
    fn run(marked: &str, step: impl Fn(&str, usize) -> usize) -> String {
        let offset = marked.find('|').expect("caret marker");
        let text = marked.replacen('|', "", 1);
        let result = step(&text, offset);
        format!("{}|{}", &text[..result], &text[result..])
    }

    fn next(marked: &str) -> String {
        run(marked, |text, offset| {
            next_word_position(text, offset, WordBehavior::StopAtWordEnd)
        })
    }

    fn next_skipping(marked: &str) -> String {
        run(marked, |text, offset| {
            next_word_position(text, offset, WordBehavior::SkipSpaces)
        })
    }

    fn previous(marked: &str) -> String {
        run(marked, previous_word_position)
    }

    #[test]
    fn next_word_matches_blink_basic() {
        assert_eq!(next("|(1) abc def"), "(|1) abc def");
        assert_eq!(next("(|1) abc def"), "(1|) abc def");
        assert_eq!(next("(1|) abc def"), "(1)| abc def");
        assert_eq!(next("(1)| abc def"), "(1) abc| def");
        assert_eq!(next("(1) a|bc def"), "(1) abc| def");
        assert_eq!(next("(1) abc| def"), "(1) abc def|");
        assert_eq!(next("(1) abc def|"), "(1) abc def|");
    }

    #[test]
    fn next_word_skipping_spaces_matches_blink_windows() {
        assert_eq!(next_skipping("|(1) abc def"), "(|1) abc def");
        assert_eq!(next_skipping("(|1) abc def"), "(1|) abc def");
        assert_eq!(next_skipping("(1|) abc def"), "(1) |abc def");
        assert_eq!(next_skipping("(1)| abc def"), "(1) |abc def");
        assert_eq!(next_skipping("(1) |abc def"), "(1) abc |def");
        assert_eq!(next_skipping("(1) abc| def"), "(1) abc |def");
        assert_eq!(next_skipping("(1) abc |def"), "(1) abc def|");
    }

    #[test]
    fn next_word_matches_blink_punctuation() {
        assert_eq!(next("|abc.def"), "abc|.def");
        assert_eq!(next("abc|.def"), "abc.|def");
        assert_eq!(next("abc.|def"), "abc.def|");
        assert_eq!(next("|abc...def"), "abc|...def");
        assert_eq!(next("abc.|..def"), "abc...|def");
        assert_eq!(next("abc...|def"), "abc...def|");
        assert_eq!(next("|abc ((())) def"), "abc| ((())) def");
        assert_eq!(next("abc |((())) def"), "abc ((()))| def");
        assert_eq!(next("|abc 32.3 def"), "abc| 32.3 def");
        assert_eq!(next("abc |32.3 def"), "abc 32.3| def");
    }

    #[test]
    fn next_word_matches_blink_emoji() {
        assert_eq!(next("|abc \u{1F602} def"), "abc| \u{1F602} def");
        assert_eq!(next("abc |\u{1F602} def"), "abc \u{1F602}| def");
        assert_eq!(next_skipping("|abc \u{1F602} def"), "abc |\u{1F602} def");
        assert_eq!(next_skipping("abc |\u{1F602} def"), "abc \u{1F602} |def");
    }

    #[test]
    fn next_word_stops_at_line_breaks() {
        assert_eq!(next("|abc\ndef"), "abc|\ndef");
        assert_eq!(next("abc|\ndef"), "abc\n|def");
        assert_eq!(next_skipping("abc |  \ndef"), "abc   |\ndef");
    }

    #[test]
    fn previous_word_matches_blink() {
        assert_eq!(previous("(1) abc def|"), "(1) abc |def");
        assert_eq!(previous("(1) abc |def"), "(1) |abc def");
        assert_eq!(previous("(1) a|bc def"), "(1) |abc def");
        assert_eq!(previous("(1) |abc def"), "(1|) abc def");
        assert_eq!(previous("(1|) abc def"), "(|1) abc def");
        assert_eq!(previous("(|1) abc def"), "|(1) abc def");
        assert_eq!(previous("abc.def|"), "abc.|def");
        assert_eq!(previous("abc.|def"), "abc|.def");
        assert_eq!(previous("abc ((()))| def"), "abc |((())) def");
        assert_eq!(previous("abc\n|def"), "abc|\ndef");
    }

    #[test]
    fn identifiers_and_contractions_are_one_word() {
        assert_eq!(next("|snake_case rest"), "snake_case| rest");
        assert_eq!(next("|can't stop"), "can't| stop");
        assert_eq!(previous("snake_case|"), "|snake_case");
    }

    #[test]
    fn double_click_selects_the_word_under_the_pointer() {
        let text = "Two things that aren't my builds:";
        assert_eq!(&text[word_range_at(text, 5)], "things");
        assert_eq!(&text[word_range_at(text, 18)], "aren't");
        assert_eq!(&text[word_range_at(text, text.len())], ":");
        let windows = double_click_range(text, 5, WordBehavior::SkipSpaces);
        assert_eq!(&text[windows], "things ");
        let linux = double_click_range(text, 5, WordBehavior::StopAtWordEnd);
        assert_eq!(&text[linux], "things");
        let path = "see main.rs:12";
        assert_eq!(&path[word_range_at(path, 5)], "main");
    }

    #[test]
    fn double_click_never_selects_a_line_break() {
        let text = "abc\n\ndef";
        assert_eq!(word_range_at(text, 4), 4..4);
        assert_eq!(&text[word_range_at(text, 3)], "abc");
        let trailing = double_click_range("abc  \nx", 0, WordBehavior::SkipSpaces);
        assert_eq!(trailing, 0..5);
    }

    #[test]
    fn triple_click_selects_the_line_with_its_break() {
        let text = "one\ntwo three\nfour";
        assert_eq!(&text[paragraph_range_at(text, 6)], "two three\n");
        assert_eq!(&text[paragraph_range_at(text, 16)], "four");
    }

    #[test]
    fn click_counts_map_to_units() {
        assert_eq!(SelectionUnit::from_click_count(1), SelectionUnit::Character);
        assert_eq!(SelectionUnit::from_click_count(2), SelectionUnit::Word);
        assert_eq!(SelectionUnit::from_click_count(3), SelectionUnit::Paragraph);
        assert_eq!(SelectionUnit::from_click_count(4), SelectionUnit::Paragraph);
    }

    #[test]
    fn word_drag_grows_by_whole_words_in_both_directions() {
        let text = "alpha beta gamma";
        let origin = SelectionUnit::Word.range_at(text, 7);
        assert_eq!(origin.start, 6);
        // Forward: from "beta" into "gamma" covers all of "gamma".
        let (forward, reversed) = SelectionUnit::Word.extend(text, &origin, 12);
        assert_eq!(forward, 6..16);
        assert!(!reversed);
        // Backward: into "alpha" keeps "beta" whole and takes all of "alpha".
        let (backward, reversed) = SelectionUnit::Word.extend(text, &origin, 2);
        assert_eq!(backward.start, 0);
        assert_eq!(backward.end, origin.end);
        assert!(reversed);
        // Inside the original word the selection stays that word.
        assert_eq!(
            SelectionUnit::Word.extend(text, &origin, 8),
            (origin.clone(), false)
        );
    }

    #[test]
    fn paragraph_drag_grows_by_whole_lines() {
        let text = "one\ntwo\nthree";
        let origin = SelectionUnit::Paragraph.range_at(text, 5);
        assert_eq!(origin, 4..8);
        assert_eq!(
            SelectionUnit::Paragraph.extend(text, &origin, 10),
            (4..13, false)
        );
        assert_eq!(
            SelectionUnit::Paragraph.extend(text, &origin, 1),
            (0..8, true)
        );
    }
}
