//! Pure development-instance detection and document-title presentation policy.
//!
//! This is the native counterpart of `lib/root/dev-instance.ts`. The caller
//! owns fetching and decoding `/health`; this leaf models only the decoded
//! value classes needed to preserve the source's structural check and the
//! exact title rewrite. It does not depend on a JSON representation or touch
//! browser APIs.

#![forbid(unsafe_code)]

/// The exact marker used to identify a title belonging to a development
/// Forge.
pub const DEV_TITLE_MARKER: &str = "[Dev]";

/// A decoded value that can be reached through a health object's
/// `development` property.
///
/// The scalar payloads other than the boolean are intentionally elided: the
/// policy only needs their decoded type. `Object` represents any other
/// decoded object value and is never treated as the required boolean.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodedHealthValue {
    /// A decoded JSON null value.
    Null,
    /// A decoded boolean value.
    Boolean(bool),
    /// A decoded JSON number.
    Number,
    /// A decoded JSON string.
    String,
    /// A decoded JSON array.
    Array,
    /// A decoded JSON object.
    Object,
}

/// The decoded health input relevant to development-instance detection.
///
/// `Object` retains only the reachable `development` property. `Some` means
/// the property exists, whether it was an own property or was reachable from
/// the decoded object's prototype in the source runtime; `None` means the
/// property is missing. Other object fields are irrelevant to this policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecodedHealth {
    /// No value was decoded by the caller; use the outer `Option` for this
    /// absence when calling [`is_development_instance`].
    Null,
    /// A decoded boolean, which is not an object health body.
    Boolean(bool),
    /// A decoded JSON number, which is not an object health body.
    Number,
    /// A decoded JSON string, which is not an object health body.
    String,
    /// A decoded JSON array, which is not an object health body here because
    /// it has no reachable `development` property in the decoded projection.
    Array,
    /// A decoded object and its reachable `development` property, if any.
    Object {
        /// The decoded value reached through `development`, or `None` when
        /// the property is absent.
        development: Option<DecodedHealthValue>,
    },
}

impl DecodedHealth {
    /// Creates the object projection with its already-decoded
    /// `development` property.
    #[must_use]
    pub const fn object(development: Option<DecodedHealthValue>) -> Self {
        Self::Object { development }
    }
}

/// Returns whether a decoded `/health` value identifies a development Forge.
///
/// This is deliberately strict: the value must be present, non-null, an
/// object, and have a reachable `development` value that is exactly the
/// boolean `true`. Missing values, null, all non-object values, and every
/// other property type return `false`.
#[must_use]
pub const fn is_development_instance(health: Option<DecodedHealth>) -> bool {
    matches!(
        health,
        Some(DecodedHealth::Object {
            development: Some(DecodedHealthValue::Boolean(true)),
        })
    )
}

/// Prefixes a title with [`DEV_TITLE_MARKER`] exactly once.
///
/// A title beginning with the exact, case-sensitive marker is returned
/// unchanged, including a marker followed immediately by more text. All
/// other titles receive exactly one joining space before the source's
/// JavaScript-compatible `trimEnd` operation removes trailing ECMAScript
/// whitespace. Thus an empty or whitespace-only title becomes `[Dev]`.
#[must_use]
pub fn dev_marked_title(title: &str) -> String {
    if title.starts_with(DEV_TITLE_MARKER) {
        return title.to_owned();
    }

    format!("{DEV_TITLE_MARKER} {title}")
        .trim_end_matches(is_ecmascript_trim_end_whitespace)
        .to_owned()
}

/// Returns whether a character belongs to ECMAScript's `trimEnd` set.
///
/// Rust's Unicode-aware trimming includes U+0085, while JavaScript does not;
/// spelling out the ECMAScript set keeps the two operations distinct.
fn is_ecmascript_trim_end_whitespace(character: char) -> bool {
    matches!(
        character,
        '\u{0009}'
            | '\u{000A}'
            | '\u{000B}'
            | '\u{000C}'
            | '\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}
