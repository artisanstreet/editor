//! Pure text and geometry helpers for the native text editor: UTF-8/UTF-16
//! offset conversion, character and line boundaries, logical vertical
//! movement, and the byte bound. Word units live in
//! [`artisan_ui::text_navigation`].

#![forbid(unsafe_code)]

use std::ops::Range;

use gpui::{Bounds, Pixels, Point};

fn valid_pixels(value: Pixels) -> bool {
    f32::from(value).is_finite()
}

pub(super) fn valid_bounds(bounds: &Bounds<Pixels>) -> bool {
    !bounds.is_empty()
        && valid_pixels(bounds.origin.x)
        && valid_pixels(bounds.origin.y)
        && valid_pixels(bounds.size.width)
        && valid_pixels(bounds.size.height)
        && valid_pixels(bounds.right())
        && valid_pixels(bounds.bottom())
}

pub(super) fn valid_point(point: Point<Pixels>) -> bool {
    valid_pixels(point.x) && valid_pixels(point.y)
}

/// The byte offset where the caret paints, or `None` when a non-empty
/// selection owns the highlight or the offset is stale.
pub(super) fn caret_offset(selection: &Range<usize>, text: &str) -> Option<usize> {
    if !selection.is_empty() {
        return None;
    }
    let offset = selection.end;
    (offset <= text.len() && text.is_char_boundary(offset)).then_some(offset)
}

/// The longest prefix of `text` that fits in `max_bytes` and ends on a
/// character boundary.
pub(super) fn truncate_to_bytes(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    &text[..floor_char_boundary(text, max_bytes)]
}

/// Replaces `range` in `text`, or `None` when the range is not a valid
/// character-aligned range of `text`.
pub(super) fn splice(text: &str, range: Range<usize>, replacement: &str) -> Option<String> {
    if range.start > range.end
        || range.end > text.len()
        || !text.is_char_boundary(range.start)
        || !text.is_char_boundary(range.end)
    {
        return None;
    }
    let mut next = String::with_capacity(text.len() - range.len() + replacement.len());
    next.push_str(&text[..range.start]);
    next.push_str(replacement);
    next.push_str(&text[range.end..]);
    Some(next)
}

pub(super) fn utf16_range_to_utf8(text: &str, range: Range<usize>) -> Option<Range<usize>> {
    if range.start > range.end {
        return None;
    }
    Some(utf16_offset_to_utf8(text, range.start)?..utf16_offset_to_utf8(text, range.end)?)
}

pub(super) fn utf16_offset_to_utf8(text: &str, target: usize) -> Option<usize> {
    let mut utf16_offset = 0usize;
    for (byte_offset, character) in text.char_indices() {
        if utf16_offset == target {
            return Some(byte_offset);
        }
        utf16_offset = utf16_offset.checked_add(character.len_utf16())?;
        if utf16_offset > target {
            return None;
        }
    }
    (utf16_offset == target).then_some(text.len())
}

pub(super) fn utf8_range_to_utf16(text: &str, range: Range<usize>) -> Option<Range<usize>> {
    if range.start > range.end {
        return None;
    }
    Some(utf8_offset_to_utf16(text, range.start)?..utf8_offset_to_utf16(text, range.end)?)
}

pub(super) fn utf8_offset_to_utf16(text: &str, byte_offset: usize) -> Option<usize> {
    if byte_offset > text.len() || !text.is_char_boundary(byte_offset) {
        return None;
    }
    text.get(..byte_offset)?
        .chars()
        .try_fold(0usize, |offset, character| {
            offset.checked_add(character.len_utf16())
        })
}

/// The nearest character boundary at or before `offset`.
pub(super) fn floor_char_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while offset > 0 && !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

/// The start of the character before `offset`.
pub(super) fn previous_character_boundary(text: &str, offset: usize) -> usize {
    text[..floor_char_boundary(text, offset)]
        .char_indices()
        .next_back()
        .map_or(0, |(index, _)| index)
}

/// The end of the character at `offset`.
pub(super) fn next_character_boundary(text: &str, offset: usize) -> usize {
    let offset = floor_char_boundary(text, offset);
    text[offset..]
        .chars()
        .next()
        .map_or(text.len(), |character| offset + character.len_utf8())
}

pub(super) fn logical_line_start(text: &str, offset: usize) -> usize {
    let offset = floor_char_boundary(text, offset);
    text[..offset].rfind('\n').map_or(0, |index| index + 1)
}

pub(super) fn logical_line_end(text: &str, offset: usize) -> usize {
    let offset = floor_char_boundary(text, offset);
    text[offset..]
        .find('\n')
        .map_or(text.len(), |index| offset + index)
}

/// The UTF-16 column of `offset` within its logical line.
pub(super) fn logical_column(text: &str, offset: usize) -> usize {
    let offset = floor_char_boundary(text, offset);
    let line_start = logical_line_start(text, offset);
    utf8_offset_to_utf16(&text[line_start..], offset - line_start).unwrap_or_default()
}

/// The offset one logical line above (`up`) or below the line holding
/// `cursor`, at `goal_column` clamped to the target line. Returns `cursor`
/// when there is no line in that direction.
pub(super) fn logical_vertical_target(
    text: &str,
    cursor: usize,
    up: bool,
    goal_column: usize,
) -> usize {
    let cursor = floor_char_boundary(text, cursor);
    let line_start = logical_line_start(text, cursor);
    let line_end = logical_line_end(text, cursor);

    let (target_start, target_end) = if up {
        if line_start == 0 {
            return cursor;
        }
        let previous_end = line_start - 1;
        (logical_line_start(text, previous_end), previous_end)
    } else {
        if line_end >= text.len() {
            return cursor;
        }
        let next_start = line_end + 1;
        (next_start, logical_line_end(text, next_start))
    };

    let target_line = &text[target_start..target_end];
    let mut column = 0usize;
    let mut target_offset = 0usize;
    for (byte_offset, character) in target_line.char_indices() {
        let next_column = column + character.len_utf16();
        if next_column > goal_column {
            break;
        }
        column = next_column;
        target_offset = byte_offset + character.len_utf8();
    }
    target_start + target_offset
}
