//! Pure composition policy for the document attention title.
//!
//! This is the native counterpart of
//! `modules/frontend/src/lib/root/attention-title.ts` together with the
//! title-marker helpers in `modules/protocol/src/attention-title.ts`. The
//! document title is a shared channel for route text, the development marker,
//! reader-attention state, and a Forge-repair request, so this module keeps
//! their composition deterministic and convergent. It does not read or write
//! a document, count threads, or perform Forge or protocol I/O.

/// The visible development-instance marker used in a document title.
pub const DEV_TITLE_MARKER: &str = "[Dev]";

/// The doubled U+2060 WORD JOINER marker used to request Forge repair.
pub const FORGE_REPAIR_TITLE_MARKER: &str = "\u{2060}\u{2060}";

const DEV_TITLE_PREFIX: &str = "[Dev] ";
const ATTENTION_WORD_JOINER: &str = "\u{2060}";

/// The parsed portion of one protocol attention marker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AttentionMarker {
    count: u32,
    awaiting_answer: bool,
    end: usize,
}

/// Builds the protocol attention marker for a JavaScript-number-equivalent
/// count.
///
/// The count is truncated toward zero and clamped at zero, matching
/// `Math.max(0, Math.trunc(count))` in `AttentionTitleMarkerFor`. A zero count
/// that is awaiting an answer uses the standalone `(?)` spelling; all other
/// awaiting counts append `?` to their digits. Every result ends in exactly
/// one U+2060 WORD JOINER.
///
/// As in the protocol helper, non-finite JavaScript-number cases are retained
/// as `NaN` or `Infinity` text, while negative infinity clamps to zero. Normal
/// attention counts are small enough to use the one-to-four-digit forms that
/// the title parser trusts.
#[must_use]
pub fn attention_title_marker_for(count: f64, awaiting_answer: bool) -> String {
    let digits = javascript_nonnegative_integer_text(count);
    if digits == "0" && awaiting_answer {
        return format!("(?){ATTENTION_WORD_JOINER}");
    }

    let question = if awaiting_answer { "?" } else { "" };
    format!("({digits}{question}){ATTENTION_WORD_JOINER}")
}

/// Returns the first protocol attention marker's count, if one is present.
///
/// This matches the position-independent protocol expression: a marker has
/// one to four ASCII digits, optionally followed by `?`, or is the standalone
/// `(?)` form, and must be terminated by U+2060 WORD JOINER. Plain
/// parenthesized text, malformed widths, and markers without the joiner are
/// not protocol markers.
#[must_use]
pub fn attention_count_from_title(title: &str) -> Option<u32> {
    first_attention_marker(title).map(|marker| marker.count)
}

/// Returns whether the first protocol attention marker awaits an answer.
///
/// A question mark is considered meaningful only inside a valid joiner-
/// terminated marker, exactly as in `TitleSignalsAwaitingAnswer`.
#[must_use]
pub fn title_signals_awaiting_answer(title: &str) -> bool {
    first_attention_marker(title).is_some_and(|marker| marker.awaiting_answer)
}

/// Returns whether a title contains the doubled Forge-repair marker.
///
/// This is an occurrence check, matching `TitleRequestsForgeRepair`; it is
/// intentionally independent of attention-marker position and title parsing.
#[must_use]
pub fn title_requests_forge_repair(title: &str) -> bool {
    title.contains(FORGE_REPAIR_TITLE_MARKER)
}

/// Rewrites a document title to carry one attention marker, or none.
///
/// An existing `[Dev] ` prefix remains first. After that prefix, only a
/// leading one-to-four-ASCII-digit or question-form attention marker with a
/// following ASCII space is removed. This deliberately leaves ordinary
/// `(3)` route titles, malformed markers, and markers at other positions
/// untouched. Every doubled U+2060 Forge-repair marker is removed, then one
/// requested repair suffix is appended.
///
/// `None` removes the attention marker and ignores `awaiting_answer`, matching
/// the TypeScript `undefined` branch. With `Some(count)`, the marker helper
/// supplies the truncated nonnegative count and question suffix.
#[must_use]
pub fn attention_marked_title(
    title: &str,
    count: Option<f64>,
    requests_forge_repair: bool,
    awaiting_answer: bool,
) -> String {
    let (development_prefix, bare_title) = match title.strip_prefix(DEV_TITLE_PREFIX) {
        Some(bare_title) => (DEV_TITLE_PREFIX, bare_title),
        None => ("", title),
    };

    let bare_title = strip_attention_marker_prefix(bare_title);
    let bare_title = bare_title.replace(FORGE_REPAIR_TITLE_MARKER, "");
    let repair_suffix = if requests_forge_repair {
        FORGE_REPAIR_TITLE_MARKER
    } else {
        ""
    };

    match count {
        Some(count) => format!(
            "{development_prefix}{} {bare_title}{repair_suffix}",
            attention_title_marker_for(count, awaiting_answer)
        ),
        None => format!("{development_prefix}{bare_title}{repair_suffix}"),
    }
}

/// Removes one owned attention prefix from a title body.
///
/// The final ASCII space is part of the composition boundary. The protocol
/// marker helpers intentionally accept a marker without that space, but the
/// title rewriter removes only the exact prefix emitted by its own writer.
fn strip_attention_marker_prefix(title: &str) -> &str {
    let Some(marker) = parse_attention_marker_at(title, 0) else {
        return title;
    };

    title
        .get(marker.end..)
        .and_then(|rest| rest.strip_prefix(' '))
        .unwrap_or(title)
}

/// Finds the first valid protocol marker anywhere in `title`.
fn first_attention_marker(title: &str) -> Option<AttentionMarker> {
    for (index, byte) in title.as_bytes().iter().enumerate() {
        if *byte != b'(' {
            continue;
        }
        if let Some(marker) = parse_attention_marker_at(title, index) {
            return Some(marker);
        }
    }
    None
}

/// Parses a protocol marker beginning at `start` and returns its byte end.
fn parse_attention_marker_at(title: &str, start: usize) -> Option<AttentionMarker> {
    let bytes = title.as_bytes();
    if bytes.get(start) != Some(&b'(') {
        return None;
    }

    let mut cursor = start + 1;
    let (count, awaiting_answer) = if bytes.get(cursor) == Some(&b'?') {
        cursor += 1;
        (0, true)
    } else {
        let digits_start = cursor;
        while bytes.get(cursor).is_some_and(u8::is_ascii_digit) {
            cursor += 1;
        }
        let digits_end = cursor;
        let digit_count = digits_end - digits_start;
        if !(1..=4).contains(&digit_count) {
            return None;
        }

        let awaiting_answer = bytes.get(cursor) == Some(&b'?');
        if awaiting_answer {
            cursor += 1;
        }

        let count = bytes[digits_start..digits_end]
            .iter()
            .fold(0_u32, |value, digit| value * 10 + u32::from(digit - b'0'));
        (count, awaiting_answer)
    };

    let rest = title.get(cursor..)?.strip_prefix(")\u{2060}")?;
    let end = title.len() - rest.len();
    Some(AttentionMarker {
        count,
        awaiting_answer,
        end,
    })
}

/// Formats the count portion as the reached JavaScript helper does.
fn javascript_nonnegative_integer_text(count: f64) -> String {
    if count.is_nan() {
        return "NaN".to_owned();
    }
    if count <= 0.0 {
        return "0".to_owned();
    }

    let truncated = count.trunc();
    if truncated.is_infinite() {
        return "Infinity".to_owned();
    }

    if truncated < 1e21 {
        return format!("{truncated:.0}");
    }

    javascript_scientific_text(truncated)
}

/// Converts Rust's shortest float debug form to JavaScript's exponent sign.
fn javascript_scientific_text(value: f64) -> String {
    let text = format!("{value:?}");
    let Some((mantissa, exponent)) = text.split_once('e') else {
        return text;
    };
    if exponent.starts_with('+') || exponent.starts_with('-') {
        text
    } else {
        format!("{mantissa}e+{exponent}")
    }
}
