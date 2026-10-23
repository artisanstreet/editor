//! What rich blocks ask of the host.
//!
//! The UI crate cannot open files or reach the composer, so blocks dispatch
//! these actions and the application root owns their handlers. Opening and
//! copying are separate actions; missing routing never silently copies a path.

use gpui::{App, Pixels, Point, SharedString, Window};

/// Immutable host and thread owning a resource action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceScope {
    pub host: String,
    pub thread: artisan_api::ThreadId,
}

/// Opens a file of the project at a line.
#[derive(Clone, Debug, Eq, PartialEq, gpui::Action)]
#[action(no_json, namespace = rich_blocks, name = "OpenReference")]
pub struct OpenReference {
    /// Immutable host/thread resource scope captured by the rendered block.
    pub scope: Option<ResourceScope>,
    /// The path as the agent wrote it, relative to the project.
    pub path: SharedString,
    /// The 1-based line, when the reference names one.
    pub line: Option<u32>,
}

/// Opens `path` at `line` through its owning Forge preview.
pub(super) fn open_reference(
    path: &str,
    line: Option<u32>,
    scope: Option<ResourceScope>,
    window: &mut Window,
    cx: &mut App,
) {
    let action = OpenReference {
        scope,
        path: SharedString::from(path.to_owned()),
        line,
    };
    window.dispatch_action(Box::new(action), cx);
}

/// Shows a menu at the pointer whose items copy one value each: a color as
/// written and in each notation, an image's path.
#[derive(Clone, Debug, PartialEq, Eq, gpui::Action)]
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

/// Shows `entries` as the application copy menu at `position`.
pub(in crate::markdown_renderer) fn show_copy_menu(
    position: Point<Pixels>,
    entries: Vec<CopyEntry>,
    window: &mut Window,
    cx: &mut App,
) {
    if entries.is_empty() {
        return;
    }
    let action = ShowCopyMenu { position, entries };
    window.dispatch_action(Box::new(action), cx);
}
