//! Dependency-free compact project-path presentation policy.
//!
//! This is the native counterpart of
//! `modules/frontend/src/lib/root/project-path.ts`. A project path is parsed
//! as presentation data rather than with the host platform's path APIs: a
//! browser may receive a Windows, POSIX, UNC, or WSL path regardless of the
//! machine rendering it.

#![allow(clippy::module_name_repetitions)]
#![forbid(unsafe_code)]

/// A separator that can be requested for a rendered project path.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PathSeparator {
    /// Render path separators as a backslash (`\\`).
    Backslash,
    /// Render path separators as a forward slash (`/`).
    ForwardSlash,
}

impl PathSeparator {
    const fn character(self) -> char {
        match self {
            Self::Backslash => '\\',
            Self::ForwardSlash => '/',
        }
    }
}

/// Selects native rendering or an explicit separator override.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SeparatorPreference {
    /// Use `/` for POSIX paths and `\\` for Windows paths.
    Native,
    /// Render all separators as `/`.
    ForwardSlash,
    /// Render all separators as `\\`.
    Backslash,
}

impl SeparatorPreference {
    const fn override_separator(self) -> Option<PathSeparator> {
        match self {
            Self::Native => None,
            Self::ForwardSlash => Some(PathSeparator::ForwardSlash),
            Self::Backslash => Some(PathSeparator::Backslash),
        }
    }
}

/// Alias retaining the display-format terminology used by frontend callers.
pub type PathSeparatorPreference = SeparatorPreference;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PathDialect {
    Posix,
    Windows,
}

struct ProjectPath<'a> {
    dialect: PathDialect,
    display_name: Option<&'a str>,
    suffix: Option<String>,
    anchor: String,
    segments: Vec<String>,
}

type ProjectPathRule = for<'a> fn(&mut ProjectPath<'a>);

const MAXIMUM_CONTEXT_SEGMENTS: usize = 3;

const PROJECT_PATH_RULES: [ProjectPathRule; 3] = [
    collapse_home,
    remove_repeated_project_name,
    compact_deep_middle,
];

/// Produces a compact, platform-native location for a project picker row.
///
/// `separator` is `None` for native rendering or an explicit separator for a
/// presentation-only override. The input is parsed as data rather than as a
/// host-platform path, so either dialect can be supplied on any operating
/// system. Empty and whitespace-only inputs return `None`.
///
/// The rules are deliberately ordered: conventional home roots are collapsed,
/// a final segment equal to the row's display name is removed, and only then
/// is genuinely deep remaining context shortened.
///
/// # Example
///
/// ```
/// # use artisan_frontend::project_path_policy::{PathSeparator, short_project_path};
/// assert_eq!(
///     short_project_path(
///         r"C:\Users\sander\Desktop\artisan-editor",
///         Some("artisan-editor"),
///         Some(PathSeparator::Backslash),
///     ),
///     Some(String::from(r"~\Desktop")),
/// );
/// ```
#[must_use = "use the compact path when it is present"]
pub fn short_project_path(
    root_path: &str,
    display_name: Option<&str>,
    separator: Option<PathSeparator>,
) -> Option<String> {
    let mut path = parse_path(root_path, display_name)?;
    for rule in PROJECT_PATH_RULES {
        rule(&mut path);
    }

    let rendered = render_path(&path, separator);
    (!rendered.is_empty()).then_some(rendered)
}

/// Produces a compact project path using a typed native/explicit preference.
///
/// [`SeparatorPreference::Native`] preserves the path's detected dialect;
/// the other variants apply a presentation-only separator override.
#[must_use = "use the compact path when it is present"]
pub fn short_project_path_with_preference(
    root_path: &str,
    display_name: Option<&str>,
    preference: SeparatorPreference,
) -> Option<String> {
    short_project_path(root_path, display_name, preference.override_separator())
}

fn parse_path<'a>(root_path: &'a str, display_name: Option<&'a str>) -> Option<ProjectPath<'a>> {
    let source = root_path.trim_matches(is_ecmascript_trim_whitespace);
    if source.is_empty() {
        return None;
    }

    let dialect = if source.contains('\\') || has_drive_separator(source) {
        PathDialect::Windows
    } else {
        PathDialect::Posix
    };
    let normalized = source.replace('\\', "/");
    let normalized = normalized.trim_end_matches('/');

    if let Some((distro, remainder)) = parse_wsl(normalized) {
        return Some(ProjectPath {
            anchor: "/".to_owned(),
            dialect: PathDialect::Windows,
            display_name,
            segments: split_segments(remainder),
            suffix: Some(format!("{distro} (WSL)")),
        });
    }

    if let Some((server, share, remainder)) = parse_unc(normalized) {
        return Some(ProjectPath {
            anchor: format!(r"\\{server}\{share}"),
            dialect: PathDialect::Windows,
            display_name,
            segments: split_segments(remainder),
            suffix: None,
        });
    }

    if let Some((drive, remainder)) = parse_drive(normalized) {
        return Some(ProjectPath {
            anchor: drive.to_owned(),
            dialect,
            display_name,
            segments: split_segments(remainder),
            suffix: None,
        });
    }

    let (anchor, remainder) = match normalized.strip_prefix('/') {
        Some(remainder) => ("/".to_owned(), remainder),
        None => (String::new(), normalized),
    };
    Some(ProjectPath {
        anchor,
        dialect,
        display_name,
        segments: split_segments(remainder),
        suffix: None,
    })
}

fn parse_wsl(normalized: &str) -> Option<(&str, &str)> {
    let prefix = if starts_with_ascii_case_insensitive(normalized, "//wsl$/") {
        "//wsl$/"
    } else if starts_with_ascii_case_insensitive(normalized, "//wsl.localhost/") {
        "//wsl.localhost/"
    } else {
        return None;
    };

    let remainder = &normalized[prefix.len()..];
    let (distro, path) = remainder.split_once('/').unwrap_or((remainder, ""));
    (!distro.is_empty()).then_some((distro, path))
}

fn parse_unc(normalized: &str) -> Option<(&str, &str, &str)> {
    let remainder = normalized.strip_prefix("//")?;
    let mut components = remainder.splitn(3, '/');
    let server = components.next()?;
    let share = components.next()?;
    if server.is_empty() || share.is_empty() {
        return None;
    }
    Some((server, share, components.next().unwrap_or_default()))
}

fn parse_drive(normalized: &str) -> Option<(&str, &str)> {
    let bytes = normalized.as_bytes();
    if bytes.len() < 2
        || !bytes[0].is_ascii_alphabetic()
        || bytes[1] != b':'
        || (bytes.len() > 2 && bytes[2] != b'/')
    {
        return None;
    }

    let remainder = normalized.get(2..)?.strip_prefix('/').unwrap_or_default();
    Some((&normalized[..2], remainder))
}

fn split_segments(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .map(str::to_owned)
        .collect()
}

fn collapse_home(path: &mut ProjectPath<'_>) {
    let Some(first) = path.segments.first().map(String::as_str) else {
        return;
    };
    if path.segments.len() < 2 {
        return;
    }

    let conventional_home = match path.anchor.as_str() {
        "/" => is_home_segment(first),
        anchor if is_drive_anchor(anchor) => first.eq_ignore_ascii_case("users"),
        _ => false,
    };
    if !conventional_home {
        return;
    }

    path.anchor.clear();
    path.anchor.push('~');
    path.segments.drain(..2);
}

fn remove_repeated_project_name(path: &mut ProjectPath<'_>) {
    let Some(display_name) = path.display_name else {
        return;
    };
    let Some(final_segment) = path.segments.last() else {
        return;
    };

    let repeated = match path.dialect {
        PathDialect::Windows => windows_accent_sensitive_equal(final_segment, display_name),
        PathDialect::Posix => final_segment == display_name,
    };
    if repeated {
        path.segments.pop();
    }
}

fn compact_deep_middle(path: &mut ProjectPath<'_>) {
    if path.segments.len() <= MAXIMUM_CONTEXT_SEGMENTS {
        return;
    }

    let first = path.segments[0].clone();
    let last = path.segments[path.segments.len() - 1].clone();
    path.segments = vec![first, "…".to_owned(), last];
}

fn render_path(path: &ProjectPath<'_>, separator: Option<PathSeparator>) -> String {
    let separator_character = separator.map_or_else(
        || match path.dialect {
            PathDialect::Posix => '/',
            PathDialect::Windows => '\\',
        },
        PathSeparator::character,
    );
    let joined = join_segments(&path.segments, separator_character);

    let mut rendered = String::new();
    if path.anchor == "~" {
        rendered.push('~');
        rendered.push(separator_character);
        rendered.push_str(&joined);
    } else if path.anchor == "/" {
        if path.segments.is_empty() {
            rendered.push('/');
        } else {
            rendered.push('/');
            rendered.push_str(&joined);
        }
    } else if !path.anchor.is_empty() {
        rendered.push_str(&path.anchor);
        rendered.push(separator_character);
        rendered.push_str(&joined);
    } else {
        rendered.push_str(&joined);
    }

    let rendered = match separator {
        Some(PathSeparator::Backslash) => rendered.replace('/', "\\"),
        Some(PathSeparator::ForwardSlash) => rendered.replace('\\', "/"),
        None => rendered,
    };
    match &path.suffix {
        Some(suffix) => format!("{rendered} · {suffix}"),
        None => rendered,
    }
}

fn join_segments(segments: &[String], separator: char) -> String {
    let mut joined = String::new();
    for (index, segment) in segments.iter().enumerate() {
        if index > 0 {
            joined.push(separator);
        }
        joined.push_str(segment);
    }
    joined
}

fn windows_accent_sensitive_equal(left: &str, right: &str) -> bool {
    // `localeCompare(..., { sensitivity: "accent" })` ignores case while
    // retaining accent differences. Unicode lowercasing gives the same
    // behavior for the path-segment data accepted at this boundary without
    // making the policy depend on a host locale or external crate.
    left.to_lowercase() == right.to_lowercase()
}

fn is_drive_anchor(anchor: &str) -> bool {
    let bytes = anchor.as_bytes();
    bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn has_drive_separator(source: &str) -> bool {
    let bytes = source.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
}

fn starts_with_ascii_case_insensitive(value: &str, prefix: &str) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(prefix))
}

fn is_home_segment(segment: &str) -> bool {
    segment.eq_ignore_ascii_case("home") || segment.eq_ignore_ascii_case("users")
}

fn is_ecmascript_trim_whitespace(character: char) -> bool {
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
