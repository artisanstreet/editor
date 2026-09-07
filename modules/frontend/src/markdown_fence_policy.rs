//! Pure Markdown fence scanning and highlighting admission policy.
//!
//! This is the native counterpart of the scanner portion of
//! `modules/frontend/src/lib/components/markdown/highlighting.ts`. It knows
//! which fenced blocks are present, which grammar an info label requests, and
//! which block is still open while a response streams. It deliberately does
//! not parse Markdown, load grammars, tokenize code, or render HTML/GPUI.

#![allow(clippy::module_name_repetitions)]

/// The complete alias table used by conversation fenced-code highlighting.
///
/// The first item in each pair is the label accepted in an info string and the
/// second is the canonical grammar identity requested from the highlighter.
const CONVERSATION_FENCE_LANGUAGES: &[(&str, &str)] = &[
    ("astro", "astro"),
    ("bash", "bash"),
    ("c", "c"),
    ("c#", "csharp"),
    ("c++", "cpp"),
    ("cpp", "cpp"),
    ("csharp", "csharp"),
    ("cs", "csharp"),
    ("css", "css"),
    ("cxx", "cpp"),
    ("go", "go"),
    ("golang", "go"),
    ("htm", "html"),
    ("html", "html"),
    ("java", "java"),
    ("javascript", "javascript"),
    ("js", "javascript"),
    ("jsx", "jsx"),
    ("markdown", "markdown"),
    ("md", "markdown"),
    ("mjs", "javascript"),
    ("ps", "powershell"),
    ("ps1", "powershell"),
    ("powershell", "powershell"),
    ("py", "python"),
    ("python", "python"),
    ("rs", "rust"),
    ("rust", "rust"),
    ("sh", "bash"),
    ("shell", "bash"),
    ("sql", "sql"),
    ("svelte", "svelte"),
    ("toml", "toml"),
    ("ts", "typescript"),
    ("tsx", "tsx"),
    ("typescript", "typescript"),
    ("vue", "vue"),
    ("xhtml", "xml"),
    ("xml", "xml"),
    ("yaml", "yaml"),
    ("yml", "yaml"),
    ("json", "json"),
];

/// The parsed state of one fenced block.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationFence {
    /// The info label before filename or metadata syntax, lowercased.
    pub label: Option<String>,
    /// The canonical grammar requested by [`Self::label`], when known.
    pub language: Option<String>,
    /// Every body line, represented as a newline-terminated line.
    pub body: String,
    /// Whether a matching closing run was found.
    pub closed: bool,
}

/// Returns the marker prefix and the byte offset at which a fence info string
/// begins.
fn marker_prefix(line: &str) -> Option<(char, usize, usize)> {
    let bytes = line.as_bytes();
    let mut marker_start = 0;

    while marker_start < bytes.len() && bytes[marker_start] == b' ' && marker_start < 3 {
        marker_start += 1;
    }

    // A fourth leading space means that this is indented code, not a fence.
    if bytes.get(marker_start) == Some(&b' ') {
        return None;
    }

    let marker = match bytes.get(marker_start) {
        Some(b'`') => '`',
        Some(b'~') => '~',
        _ => return None,
    };

    let mut run_end = marker_start;
    while bytes.get(run_end) == Some(&(marker as u8)) {
        run_end += 1;
    }

    let run_length = run_end - marker_start;
    (run_length >= 3).then_some((marker, run_length, run_end))
}

/// Removes the CR in a CRLF line ending while rejecting an embedded CR.
///
/// The legacy scanner splits source on LF and therefore retains CR in body
/// lines. Syntax still treats one final CR as the line ending so common CRLF
/// fence input has the same structure as LF input; body bytes remain exact.
fn syntax_line(line: &str) -> Option<&str> {
    let line = line.strip_suffix('\r').unwrap_or(line);
    (!line.contains('\r')).then_some(line)
}

/// Parses a valid fence opener from one LF-delimited source line.
fn opening(line: &str) -> Option<(char, usize, &str)> {
    let (marker, run_length, info_start) = marker_prefix(line)?;
    let line = syntax_line(line)?;
    let info_start = info_start.min(line.len());
    let mut info_start = info_start;

    while let Some(byte) = line.as_bytes().get(info_start)
        && matches!(byte, b' ' | b'\t')
    {
        info_start += 1;
    }

    let info = &line[info_start..];

    // CommonMark forbids backticks anywhere in a backtick fence's info.
    if marker == '`' && info.contains('`') {
        return None;
    }

    Some((marker, run_length, info))
}

/// Returns whether a character is whitespace in the JavaScript regular
/// expressions and `String#trim` used by the legacy scanner.
fn is_legacy_whitespace(character: char) -> bool {
    matches!(
        character,
        '\u{0009}'
            | '\u{000a}'
            | '\u{000b}'
            | '\u{000c}'
            | '\u{000d}'
            | '\u{0020}'
            | '\u{00a0}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

/// Returns the first requested label token from an opener-like line.
fn requested_label(line: &str) -> Option<String> {
    let (_, _, info_start) = marker_prefix(line)?;
    let info_start = info_start.min(line.len());
    let mut info_start = info_start;

    while let Some(byte) = line.as_bytes().get(info_start)
        && matches!(byte, b' ' | b'\t')
    {
        info_start += 1;
    }

    let info = &line[info_start..];
    let token_end = info
        .char_indices()
        .find(|&(_, character)| {
            is_legacy_whitespace(character) || matches!(character, '`' | '~' | '{')
        })
        .map_or(info.len(), |(index, _)| index);

    (token_end > 0).then(|| normalize_label(&info[..token_end]))?
}

/// Normalizes the part of an info string that identifies a fence language.
fn normalize_label(info: &str) -> Option<String> {
    let before_filename = info.split_once('[').map_or(info, |(before, _)| before);
    let before_metadata = before_filename
        .split_once('{')
        .map_or(before_filename, |(before, _)| before);
    let label = before_metadata
        .trim_matches(is_legacy_whitespace)
        .to_lowercase();

    (!label.is_empty()).then_some(label)
}

/// Resolves one normalized label through the complete legacy alias table.
fn grammar_for_label(label: &str) -> Option<&'static str> {
    CONVERSATION_FENCE_LANGUAGES
        .iter()
        .find(|&&(alias, _)| alias == label)
        .map(|&(_, grammar)| grammar)
}

/// Returns the known grammar identities requested by fenced-code opener
/// lines, deduplicated in their first-seen order.
///
/// This intentionally follows the legacy admission regular expression rather
/// than the complete fence scanner: a label can request a grammar as soon as
/// an opener-like line appears, even if a later info-string validation means
/// that the block itself is not admitted for scanning.
#[must_use]
pub fn requested_conversation_fence_languages(markdown: &str) -> Vec<String> {
    let mut requested = Vec::new();

    for line in markdown.split('\n') {
        let Some(label) = requested_label(line) else {
            continue;
        };
        let Some(grammar) = grammar_for_label(&label) else {
            continue;
        };
        if !requested.iter().any(|known| known == grammar) {
            requested.push(grammar.to_owned());
        }
    }

    requested
}

/// Returns whether one LF-delimited line is a valid closing run for `marker`.
fn is_closing(line: &str, marker: char, opening_length: usize) -> bool {
    let Some(line) = syntax_line(line) else {
        return false;
    };
    let bytes = line.as_bytes();
    let mut run_start = 0;
    let mut leading_spaces = 0;

    while run_start < bytes.len() && bytes[run_start] == b' ' && leading_spaces < 3 {
        run_start += 1;
        leading_spaces += 1;
    }
    if bytes.get(run_start) == Some(&b' ') {
        return false;
    }

    let Some(&first) = bytes.get(run_start) else {
        return false;
    };
    if first != marker as u8 {
        return false;
    }

    let mut run_end = run_start;
    while bytes.get(run_end) == Some(&first) {
        run_end += 1;
    }
    if run_end - run_start < opening_length {
        return false;
    }

    bytes[run_end..]
        .iter()
        .all(|&byte| matches!(byte, b' ' | b'\t'))
}

/// Returns all fenced blocks in source order.
///
/// Openers allow at most three leading spaces and a homogeneous backtick or
/// tilde run of at least three markers. A backtick in a backtick opener's info
/// string invalidates that opener. Once an opener is admitted, marker-looking
/// text is body content until a same-character run at least as long as the
/// opener appears with only permitted indentation and trailing whitespace.
#[must_use]
pub fn scan_conversation_fences(markdown: &str) -> Vec<ConversationFence> {
    let mut lines = markdown.split('\n');
    let mut fences = Vec::new();

    while let Some(line) = lines.next() {
        let Some((marker, opening_length, info)) = opening(line) else {
            continue;
        };

        let mut body_lines = Vec::new();
        let mut closed = false;
        for line in lines.by_ref() {
            if is_closing(line, marker, opening_length) {
                closed = true;
                break;
            }
            body_lines.push(line);
        }

        let body = if body_lines.is_empty() {
            String::new()
        } else {
            let mut body = body_lines.join("\n");
            body.push('\n');
            body
        };
        let label = normalize_label(info);
        let language = label
            .as_deref()
            .and_then(grammar_for_label)
            .map(str::to_owned);

        fences.push(ConversationFence {
            label,
            language,
            body,
            closed,
        });
    }

    fences
}

/// Returns the normalized body of the first fenced block that remains open.
///
/// The scanner stores each body line with a terminating LF. Those trailing
/// LF characters are parser bookkeeping for the open-body admission check and
/// are removed here, while every other character remains exact.
#[must_use]
pub fn open_conversation_fence_body(markdown: &str) -> Option<String> {
    scan_conversation_fences(markdown)
        .into_iter()
        .find(|fence| !fence.closed)
        .map(|fence| normalize_fence_body(&fence.body))
}

/// Returns whether `code` is exactly the currently open fence body after
/// removing only trailing LF characters from `code`.
///
/// `open_body` is the normalized value returned by
/// [`open_conversation_fence_body`]. It is compared byte-for-byte after the
/// candidate code is normalized, matching the legacy predicate's contract.
#[must_use]
pub fn is_open_conversation_fence_body(code: &str, open_body: Option<&str>) -> bool {
    let Some(open_body) = open_body else {
        return false;
    };

    normalize_fence_body(code) == open_body
}

/// Removes parser-generated trailing LF characters from a fence body.
fn normalize_fence_body(body: &str) -> String {
    body.trim_end_matches('\n').to_owned()
}
