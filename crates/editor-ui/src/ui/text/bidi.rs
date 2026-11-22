use crate::ui::text::slice::slice;
use std::ops::Range;
use unicode_bidi::ParagraphBidiInfo;

fn paragraph(text: &str, offset: usize) -> Range<usize> {
    let start = slice(text, ..offset).rfind('\n').map_or(0, |at| at + 1);
    let end = slice(text, offset..)
        .find('\n')
        .map_or(text.len(), |at| offset + at);
    start..end
}

fn has_rtl(text: &str) -> bool {
    text.chars().any(|ch| ch >= '\u{0590}')
}

pub(crate) fn paragraph_is_rtl(text: &str, offset: usize) -> bool {
    let range = paragraph(text, offset);
    let paragraph = slice(text, range);
    has_rtl(paragraph)
        && ParagraphBidiInfo::new(paragraph, None)
            .paragraph_level
            .is_rtl()
}

fn char_is_rtl(text: &str, offset: usize) -> bool {
    let range = paragraph(text, offset);
    let paragraph = slice(text, range.clone());
    has_rtl(paragraph)
        && ParagraphBidiInfo::new(paragraph, None)
            .levels
            .get(offset - range.start)
            .is_some_and(unicode_bidi::Level::is_rtl)
}

pub(crate) fn selection_is_rtl(text: &str, selection: Range<usize>, focus: usize) -> bool {
    let last = slice(text, ..selection.end)
        .char_indices()
        .next_back()
        .map_or(selection.start, |(at, _)| at.max(selection.start));
    let first = char_is_rtl(text, selection.start);
    if first == char_is_rtl(text, last) {
        first
    } else {
        paragraph_is_rtl(text, focus)
    }
}
