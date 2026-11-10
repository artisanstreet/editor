use super::Span;
use std::ops::Range;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct InlineLink {
    pub(crate) range: Range<usize>,
    pub(crate) destination: String,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum InlineLayer {
    Code { in_link: bool },
    Emphasis,
    Strong { in_link: bool },
    Link,
    Command,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FlatInline {
    pub(crate) source: String,
    pub(crate) runs: Vec<(Range<usize>, usize)>,
    pub(crate) chains: Vec<Vec<InlineLayer>>,
    pub(crate) links: Vec<InlineLink>,
    pub(crate) code_ranges: Vec<Range<usize>>,
    pub(crate) file_icons: Vec<(usize, (editor_assets::AssetId, editor_assets::AssetId))>,
    pub(crate) file_slots: Vec<Range<usize>>,
    pub(crate) swatches: bool,
    pub(crate) colors: Vec<InlineColor>,
}

pub(crate) const SWATCH_SLOT: &str = "\u{2060}\u{2003}\u{2060}";

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct InlineColor {
    pub(crate) range: Range<usize>,
    pub(crate) slot: Range<usize>,
    pub(crate) value: String,
    pub(crate) alone: bool,
}

pub(crate) fn flatten_inline_with_swatches(spans: &[Span]) -> FlatInline {
    let mut flat = FlatInline {
        swatches: true,
        ..FlatInline::default()
    };
    let mut chain = Vec::new();
    flatten(spans, &mut chain, &mut flat, false);
    flat
}

fn push_swatch(flat: &mut FlatInline, value: &str, alone: bool) -> Range<usize> {
    let start = flat.source.len();
    flat.source.push_str(SWATCH_SLOT);
    let slot = start..flat.source.len();
    if alone {
        flat.colors.push(InlineColor {
            range: slot.clone(),
            slot: slot.clone(),
            value: value.to_owned(),
            alone,
        });
    }
    slot
}

fn push_prose(flat: &mut FlatInline, text: &str, chain: &[InlineLayer]) {
    let colors = if flat.swatches {
        crate::ui::media::color::color_ranges(text, &[])
    } else {
        Vec::new()
    };
    let mut cursor = 0;
    for color in colors {
        let start = flat.source.len();
        flat.source.push_str(&text[cursor..color.start]);
        let end = flat.source.len();
        emit(flat, start, end, chain);
        push_swatch(flat, &text[color.clone()], true);
        cursor = color.end;
    }
    let start = flat.source.len();
    flat.source.push_str(&text[cursor..]);
    let end = flat.source.len();
    emit(flat, start, end, chain);
}

fn push_code(flat: &mut FlatInline, code: &str, chain: &mut Vec<InlineLayer>, in_link: bool) {
    let colors = if flat.swatches && !in_link {
        let whole = 0..code.len();
        crate::ui::media::color::color_ranges(code, std::slice::from_ref(&whole))
    } else {
        Vec::new()
    };
    if let [only] = colors.as_slice()
        && code[only.clone()] == *code.trim()
    {
        push_swatch(flat, code.trim(), true);
        return;
    }
    let start = flat.source.len();
    let mut cursor = 0;
    for color in colors {
        flat.source.push_str(&code[cursor..color.start]);
        let slot = push_swatch(flat, &code[color.clone()], false);
        flat.source.push_str(&code[color.clone()]);
        flat.colors.push(InlineColor {
            range: slot.start..flat.source.len(),
            slot,
            value: code[color.clone()].to_owned(),
            alone: false,
        });
        cursor = color.end;
    }
    flat.source.push_str(&code[cursor..]);
    let end = flat.source.len();
    if start < end {
        flat.code_ranges.push(start..end);
    }
    chain.push(InlineLayer::Code { in_link });
    emit(flat, start, end, chain);
    chain.pop();
}

fn emit(flat: &mut FlatInline, start: usize, end: usize, chain: &[InlineLayer]) {
    if start >= end || chain.is_empty() {
        return;
    }
    let id = flat
        .chains
        .iter()
        .position(|known| known == chain)
        .unwrap_or_else(|| {
            flat.chains.push(chain.to_vec());
            flat.chains.len() - 1
        });
    if let Some((last_range, last_id)) = flat.runs.last_mut()
        && *last_id == id
        && last_range.end == start
    {
        last_range.end = end;
        return;
    }
    flat.runs.push((start..end, id));
}

fn flatten(spans: &[Span], chain: &mut Vec<InlineLayer>, flat: &mut FlatInline, in_link: bool) {
    for span in spans {
        match span {
            Span::Text(inline) | Span::Html(inline) => {
                if matches!(span, Span::Text(_))
                    && !in_link
                    && let Some(linked) = bare_url_spans(inline)
                {
                    flatten(&linked, chain, flat, true);
                    continue;
                }
                if in_link {
                    let start = flat.source.len();
                    flat.source.push_str(inline);
                    let end = flat.source.len();
                    emit(flat, start, end, chain);
                } else {
                    push_prose(flat, inline, chain);
                }
            }
            Span::Code(code) => push_code(flat, code, chain, in_link),
            Span::Command(command) => {
                chain.push(InlineLayer::Command);
                let start = flat.source.len();
                flat.source.push_str(command);
                let end = flat.source.len();
                emit(flat, start, end, chain);
                chain.pop();
            }
            Span::Emphasis(inner) => {
                chain.push(InlineLayer::Emphasis);
                flatten(inner, chain, flat, in_link);
                chain.pop();
            }
            Span::Strong(inner) => {
                chain.push(InlineLayer::Strong { in_link });
                flatten(inner, chain, flat, in_link);
                chain.pop();
            }
            Span::Link { label, destination } => {
                if is_openable_link_destination(destination) {
                    flatten_link(label, destination, chain, flat);
                } else {
                    flatten(label, chain, flat, in_link);
                }
            }
        }
    }
}

fn flatten_link(
    label: &[Span],
    destination: &str,
    chain: &mut Vec<InlineLayer>,
    flat: &mut FlatInline,
) {
    let start = flat.source.len();
    chain.push(InlineLayer::Link);
    if let Some(path) = file_link_path(destination)
        && flat.swatches
    {
        let slot_start = flat.source.len();
        flat.source.push_str(SWATCH_SLOT);
        flat.source.push('\u{00a0}');
        flat.file_slots.push(slot_start..flat.source.len());
        flat.file_icons.push((
            slot_start + '\u{2060}'.len_utf8(),
            crate::ui::icon::file_icon(&path),
        ));
    }
    flatten(label, chain, flat, true);
    chain.pop();
    let end = flat.source.len();
    if start < end {
        flat.links.push(InlineLink {
            range: start..end,
            destination: destination.to_owned(),
        });
    }
}

fn bare_url_spans(text: &str) -> Option<Vec<Span>> {
    let mut result = Vec::new();
    let mut consumed = 0;
    let mut cursor = 0;
    while cursor < text.len() {
        let Some(relative) = text[cursor..].find("http") else {
            break;
        };
        let start = cursor + relative;
        cursor = start + 4;
        if !(text[start..].starts_with("https://") || text[start..].starts_with("http://"))
            || text[..start]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
        {
            continue;
        }
        let tail = &text[start..];
        let mut end = tail
            .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\'' | '`'))
            .unwrap_or(tail.len());
        let mut balance = [0isize; 3];
        for ch in tail[..end].chars() {
            for (index, (open, close)) in [('(', ')'), ('[', ']'), ('{', '}')].iter().enumerate() {
                if ch == *open {
                    balance[index] -= 1;
                }
                if ch == *close {
                    balance[index] += 1;
                }
            }
        }
        while let Some(last) = tail[..end].chars().next_back() {
            let closer = [')', ']', '}'].iter().position(|close| *close == last);
            if matches!(last, '.' | ',' | ';' | ':' | '!' | '?') {
                end -= last.len_utf8();
            } else if let Some(index) = closer.filter(|index| balance[*index] > 0) {
                balance[index] -= 1;
                end -= last.len_utf8();
            } else {
                break;
            }
        }
        let destination = &tail[..end];
        let authority = destination
            .split_once("://")
            .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or_default())
            .unwrap_or_default();
        if authority.is_empty() || !is_openable_link_destination(destination) {
            continue;
        }
        if consumed < start {
            result.push(Span::Text(text[consumed..start].to_owned()));
        }
        result.push(Span::Link {
            label: vec![Span::Text(destination.to_owned())],
            destination: destination.to_owned(),
        });
        consumed = start + end;
        cursor = consumed;
    }
    if result.is_empty() {
        return None;
    }
    if consumed < text.len() {
        result.push(Span::Text(text[consumed..].to_owned()));
    }
    Some(result)
}

fn is_openable_link_destination(destination: &str) -> bool {
    let lower = destination.to_ascii_lowercase();
    lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("mailto:")
        || file_link_path(destination).is_some()
}

pub(super) fn file_link_path(destination: &str) -> Option<std::path::PathBuf> {
    let path = if destination.to_ascii_lowercase().starts_with("file:") {
        let url = url::Url::parse(destination).ok()?;
        if url.host_str().is_some_and(|host| host != "localhost") {
            return None;
        }
        url.to_file_path().ok()?
    } else {
        let path = destination.split('#').next()?;
        let mut path = path;
        for _ in 0..2 {
            if let Some((base, line)) = path.rsplit_once(':')
                && !line.is_empty()
                && line.bytes().all(|byte| byte.is_ascii_digit())
            {
                path = base;
            }
        }
        std::path::PathBuf::from(path)
    };
    let raw = destination.split('#').next().unwrap_or_default();
    let has_scheme = raw.split_once(':').is_some_and(|(prefix, _)| {
        !prefix.contains(['/', '\\'])
            && !prefix.is_empty()
            && prefix
                .bytes()
                .next()
                .is_some_and(|b| b.is_ascii_alphabetic())
            && prefix
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'))
    });
    (!path.as_os_str().is_empty()
        && !destination.starts_with("//")
        && (!has_scheme
            || destination.to_ascii_lowercase().starts_with("file:")
            || raw.split_once(':').is_some_and(|(_, suffix)| {
                !suffix.is_empty()
                    && suffix
                        .split(':')
                        .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
            })))
    .then_some(path)
}
