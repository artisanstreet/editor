//! What rich blocks ask of the host.
//!
//! The UI crate cannot open files or reach the composer, so blocks dispatch
//! these actions and the host decides what they do. A block whose action no
//! handler takes copies the same text to the clipboard instead, so every
//! gesture does something.

use gpui::{Action, App, ClipboardItem, Pixels, Point, SharedString, Window};

/// Opens a file of the project at a line.
#[derive(Clone, Debug, Eq, PartialEq, gpui::Action)]
#[action(no_json, namespace = rich_blocks, name = "OpenReference")]
pub struct OpenReference {
    /// The path as the agent wrote it, relative to the project.
    pub path: SharedString,
    /// The 1-based line, when the reference names one.
    pub line: Option<u32>,
}

/// Dispatches `action`, or copies `fallback` when nothing handles it.
pub(super) fn dispatch_or_copy(
    action: &dyn Action,
    fallback: &str,
    window: &mut Window,
    cx: &mut App,
) {
    if window.is_action_available(action, cx) {
        window.dispatch_action(action.boxed_clone(), cx);
    } else {
        cx.write_to_clipboard(ClipboardItem::new_string(fallback.to_owned()));
    }
}

/// Opens `path` at `line` through the host, or copies `path:line`.
pub(super) fn open_reference(path: &str, line: Option<u32>, window: &mut Window, cx: &mut App) {
    let fallback = match line {
        Some(line) => format!("{path}:{line}"),
        None => path.to_owned(),
    };
    let action = OpenReference {
        path: SharedString::from(path.to_owned()),
        line,
    };
    dispatch_or_copy(&action, &fallback, window, cx);
}

/// Shows a menu at the pointer whose items copy one value each: a color as
/// written and in each notation, an image's path.
#[derive(Clone, Debug, PartialEq, gpui::Action)]
#[action(no_json, namespace = rich_blocks, name = "ShowCopyMenu")]
pub struct ShowCopyMenu {
    /// Where the pointer pressed, in window coordinates.
    pub position: Point<Pixels>,
    /// The items, in menu order; never empty.
    pub entries: Vec<CopyEntry>,
}

/// One item of a [`ShowCopyMenu`]: its label, what it copies, and its
/// place in the menu.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CopyEntry {
    pub label: SharedString,
    pub text: SharedString,
    pub kind: CopyKind,
}

/// Where a [`CopyEntry`] sits in its menu.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CopyKind {
    /// The thing itself, as written: the copy icon beside it.
    Primary,
    /// The same thing in another form: after a separator below the primary
    /// entries, without an icon but indented as if it had one.
    Alternative,
}

impl CopyEntry {
    /// An entry that copies the thing as written.
    pub fn primary(label: impl Into<SharedString>, text: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            text: text.into(),
            kind: CopyKind::Primary,
        }
    }

    /// An entry that copies the thing in another form.
    pub fn alternative(label: impl Into<SharedString>, text: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            text: text.into(),
            kind: CopyKind::Alternative,
        }
    }
}

/// Shows `entries` as a copy menu at `position` through the host, or copies
/// the first entry.
pub(in crate::markdown_renderer) fn show_copy_menu(
    position: Point<Pixels>,
    entries: Vec<CopyEntry>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(first) = entries.first().map(|entry| entry.text.clone()) else {
        return;
    };
    let action = ShowCopyMenu { position, entries };
    dispatch_or_copy(&action, &first, window, cx);
}
