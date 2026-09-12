//! Bounded HTML metadata parsing for rich-link resolution.
//!
//! The parser follows the reference metadata precedence: `og:title`, then
//! `twitter:title`, then the document `<title>`. Text fields are
//! entity-decoded, whitespace-collapsed, trimmed, and bounded to
//! [`RICH_LINK_PAGE_NAME_MAX_CHARS`] scalar values. No path, credential, or
//! raw HTML byte leaves this module: the outputs are bounded display text.

#![forbid(unsafe_code)]

use super::RICH_LINK_PAGE_NAME_MAX_CHARS;

/// Normalized text-only metadata extracted from one HTML document.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParsedRichLinkHtml {
    /// Document `<title>` text, normalized and bounded.
    pub title: Option<String>,
    /// First `og:title` metadata value.
    pub open_graph: Option<String>,
    /// First `twitter:title` metadata value.
    pub twitter: Option<String>,
}

impl ParsedRichLinkHtml {
    /// Returns the reference-ordered page name, if any candidate exists.
    #[must_use]
    pub fn page_name(&self) -> Option<&str> {
        self.open_graph
            .as_deref()
            .or(self.twitter.as_deref())
            .or(self.title.as_deref())
    }
}

/// Parses metadata with the reference precedence and bounds.
///
/// The scanner never allocates from document structure and tolerates the
/// malformed, unclosed tags real pages carry. Text fields are entity-decoded,
/// whitespace-collapsed, trimmed, and bounded to
/// [`RICH_LINK_PAGE_NAME_MAX_CHARS`] scalar values.
#[must_use]
pub fn parse_rich_link_html(html: &str) -> ParsedRichLinkHtml {
    let lower = html.to_ascii_lowercase();
    let mut parsed = ParsedRichLinkHtml::default();
    let mut cursor = 0_usize;
    while let Some(offset) = lower[cursor..].find('<') {
        let tag_start = cursor + offset;
        let rest_lower = &lower[tag_start + 1..];
        if rest_lower.starts_with("!--") {
            match rest_lower.find("-->") {
                Some(end) => cursor = tag_start + 1 + end + 3,
                None => break,
            }
            continue;
        }
        if rest_lower.starts_with('!') || rest_lower.starts_with('/') || rest_lower.starts_with('?')
        {
            match find_tag_end(&lower, tag_start + 1) {
                Some(end) => cursor = end + 1,
                None => break,
            }
            continue;
        }
        let name_end = rest_lower
            .find(|character: char| {
                character.is_ascii_whitespace() || character == '>' || character == '/'
            })
            .unwrap_or(rest_lower.len());
        let name = &rest_lower[..name_end];
        let Some(tag_end) = find_tag_end(&lower, tag_start + 1 + name_end) else {
            break;
        };
        match name {
            "title" if parsed.title.is_none() => {
                let text_start = tag_end + 1;
                let (text, next) = match lower[text_start..].find("</title") {
                    Some(close) => {
                        let text_end = text_start + close;
                        let after = lower[text_end..]
                            .find('>')
                            .map_or(lower.len(), |offset| text_end + offset + 1);
                        (&html[text_start..text_end], after)
                    }
                    None => (&html[text_start..], lower.len()),
                };
                parsed.title = normalize_rich_link_text(&decode_rich_link_entities(text));
                cursor = next;
                continue;
            }
            "meta" => {
                let attributes = &html[tag_start + 1 + name_end..tag_end];
                let meta = parse_meta_attributes(attributes);
                apply_meta(&mut parsed, &meta);
            }
            _ => {}
        }
        cursor = tag_end + 1;
    }
    parsed
}

fn apply_meta(parsed: &mut ParsedRichLinkHtml, meta: &MetaAttributes) {
    let Some(content) = meta.content.as_deref() else {
        return;
    };
    let key = meta
        .property
        .as_deref()
        .or(meta.name.as_deref())
        .map(|value| value.trim().to_ascii_lowercase());
    match key.as_deref() {
        Some("og:title") if parsed.open_graph.is_none() => {
            parsed.open_graph = normalize_rich_link_text(content);
        }
        Some("twitter:title") if parsed.twitter.is_none() => {
            parsed.twitter = normalize_rich_link_text(content);
        }
        _ => {}
    }
}

#[derive(Default)]
struct MetaAttributes {
    property: Option<String>,
    name: Option<String>,
    content: Option<String>,
}

fn parse_meta_attributes(attributes: &str) -> MetaAttributes {
    let bytes = attributes.as_bytes();
    let mut meta = MetaAttributes::default();
    let mut index = 0_usize;
    while index < bytes.len() {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let name_start = index;
        while index < bytes.len()
            && !bytes[index].is_ascii_whitespace()
            && bytes[index] != b'='
            && bytes[index] != b'/'
        {
            index += 1;
        }
        if name_start == index {
            index += 1;
            continue;
        }
        let attribute_name = attributes[name_start..index].to_ascii_lowercase();
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let value = if bytes.get(index) == Some(&b'=') {
            index += 1;
            while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                index += 1;
            }
            read_attribute_value(attributes, &mut index)
        } else {
            String::new()
        };
        match attribute_name.as_str() {
            "property" => meta.property = Some(value),
            "name" => meta.name = Some(value),
            "content" => meta.content = Some(value),
            _ => {}
        }
    }
    meta
}

fn read_attribute_value(attributes: &str, index: &mut usize) -> String {
    let bytes = attributes.as_bytes();
    if bytes
        .get(*index)
        .is_some_and(|byte| *byte == b'"' || *byte == b'\'')
    {
        let quote = bytes[*index];
        *index += 1;
        let value_start = *index;
        while *index < bytes.len() && bytes[*index] != quote {
            *index += 1;
        }
        let value = decode_rich_link_entities(&attributes[value_start..*index]);
        if *index < bytes.len() {
            *index += 1;
        }
        return value;
    }
    let value_start = *index;
    while *index < bytes.len() && !bytes[*index].is_ascii_whitespace() {
        *index += 1;
    }
    decode_rich_link_entities(&attributes[value_start..*index])
}

fn find_tag_end(lower: &str, from: usize) -> Option<usize> {
    let bytes = lower.as_bytes();
    let mut quote: Option<u8> = None;
    let mut index = from;
    while index < bytes.len() {
        let byte = bytes[index];
        match quote {
            Some(active) => {
                if byte == active {
                    quote = None;
                }
            }
            None => {
                if byte == b'"' || byte == b'\'' {
                    quote = Some(byte);
                } else if byte == b'>' {
                    return Some(index);
                }
            }
        }
        index += 1;
    }
    None
}

/// Collapses whitespace, trims, and bounds one metadata text field.
///
/// Returns `None` when the normalized field is empty.
#[must_use]
pub fn normalize_rich_link_text(value: &str) -> Option<String> {
    let collapsed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    Some(
        collapsed
            .chars()
            .take(RICH_LINK_PAGE_NAME_MAX_CHARS)
            .collect(),
    )
}

fn decode_rich_link_entities(value: &str) -> String {
    if !value.contains('&') {
        return value.to_owned();
    }
    let mut decoded = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find('&') {
        decoded.push_str(&rest[..start]);
        let after = &rest[start..];
        let Some(end) = after.find(';') else {
            decoded.push_str(after);
            return decoded;
        };
        if end > 32 {
            decoded.push_str(&after[..=end]);
            rest = &after[end + 1..];
            continue;
        }
        if let Some(character) = decode_entity(&after[1..end]) {
            decoded.push(character);
        } else {
            decoded.push_str(&after[..=end]);
        }
        rest = &after[end + 1..];
    }
    decoded.push_str(rest);
    decoded
}

fn decode_entity(entity: &str) -> Option<char> {
    if let Some(number) = entity.strip_prefix('#') {
        let value = if let Some(hex) = number
            .strip_prefix('x')
            .or_else(|| number.strip_prefix('X'))
        {
            u32::from_str_radix(hex, 16).ok()?
        } else {
            number.parse::<u32>().ok()?
        };
        return char::from_u32(value);
    }
    let character = match entity {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        "mdash" => '\u{2014}',
        "ndash" => '\u{2013}',
        "hellip" => '\u{2026}',
        "copy" => '\u{a9}',
        "reg" => '\u{ae}',
        "trade" => '\u{2122}',
        "lsquo" => '\u{2018}',
        "rsquo" => '\u{2019}',
        "ldquo" => '\u{201c}',
        "rdquo" => '\u{201d}',
        "middot" => '\u{b7}',
        "bull" => '\u{2022}',
        _ => return None,
    };
    Some(character)
}
