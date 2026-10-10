//! `file-tree`: the files a change touched, under directories derived from
//! their paths.
//!
//! Directories form once, at parse: chains of single-child directories
//! fold into one row (`modules/ui/src`), directories sort before files,
//! and the tree flattens into rows in reading order. Collapsing a
//! directory hides the rows nested under it.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::rc::Rc;

use artisan_assets::AssetId;
use gpui::{
    AnyElement, App, Div, Entity, IntoElement, ParentElement as _, RenderOnce, SharedString,
    Stateful, Styled as _, Window, div, prelude::*, px,
};
use serde::Deserialize;

use super::super::actions::open_reference;
use super::super::kit::{self, Tone};
use super::super::{BlockContext, RichBlock};
use super::{chevron, counted};
use crate::theme::ArtisanTheme;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Change {
    #[serde(alias = "add", alias = "new", alias = "created", alias = "create")]
    Added,
    #[serde(
        alias = "modify",
        alias = "changed",
        alias = "updated",
        alias = "edited",
        alias = "edit"
    )]
    Modified,
    #[serde(alias = "delete", alias = "removed", alias = "remove")]
    Deleted,
    #[serde(alias = "rename", alias = "moved", alias = "move")]
    Renamed,
    #[serde(other)]
    Other,
}

impl Change {
    const ALL: [Self; 4] = [Self::Added, Self::Modified, Self::Deleted, Self::Renamed];

    fn tone(self) -> Tone {
        match self {
            Self::Added => Tone::Success,
            Self::Modified => Tone::Warning,
            Self::Deleted => Tone::Danger,
            Self::Renamed => Tone::Info,
            Self::Other => Tone::Neutral,
        }
    }

    fn word(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
            Self::Renamed => "renamed",
            Self::Other => "changed",
        }
    }

    fn letter(self) -> &'static str {
        match self {
            Self::Added => "A",
            Self::Modified => "M",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::Other => "•",
        }
    }
}

#[derive(Debug, Deserialize)]
struct EntrySpec {
    path: String,
    #[serde(default)]
    change: Option<Change>,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Spec {
    entries: Vec<EntrySpec>,
}

#[derive(Debug)]
struct Entry {
    path: SharedString,
    change: Option<Change>,
    note: Option<SharedString>,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) enum RowKind {
    /// A directory; `key` is its full path, unique in the tree.
    Dir {
        key: SharedString,
        files: usize,
    },
    File {
        entry: usize,
    },
}

#[derive(Debug)]
pub(super) struct Row {
    pub(super) depth: usize,
    pub(super) name: SharedString,
    pub(super) kind: RowKind,
}

#[derive(Debug)]
struct Model {
    entries: Vec<Entry>,
    rows: Vec<Row>,
}

/// The `file-tree` spec, derived once.
#[derive(Debug, Deserialize)]
#[serde(try_from = "Spec")]
pub(super) struct FileTree(Rc<Model>);

impl FileTree {
    #[cfg(test)]
    pub(super) fn rows(&self) -> &[Row] {
        &self.0.rows
    }
}

/// `path` as forward-slash segments without `.` or empty parts.
fn segments(path: &str) -> Vec<String> {
    path.trim()
        .replace('\\', "/")
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .map(str::to_owned)
        .collect()
}

#[derive(Default)]
struct Dir {
    name: String,
    dirs: Vec<Dir>,
    files: Vec<(String, usize)>,
}

impl Dir {
    fn insert(&mut self, parts: &[String], entry: usize) {
        match parts {
            [] => {}
            [file] => self.files.push((file.clone(), entry)),
            [dir, rest @ ..] => {
                let index =
                    if let Some(index) = self.dirs.iter().position(|child| child.name == *dir) {
                        index
                    } else {
                        self.dirs.push(Self {
                            name: dir.clone(),
                            ..Self::default()
                        });
                        self.dirs.len() - 1
                    };
                self.dirs[index].insert(rest, entry);
            }
        }
    }

    fn file_count(&self) -> usize {
        self.files.len() + self.dirs.iter().map(Self::file_count).sum::<usize>()
    }

    /// Folds chains of directories that hold one directory and no files.
    fn fold(&mut self) {
        while self.files.is_empty() && self.dirs.len() == 1 {
            let child = self.dirs.remove(0);
            self.name = format!("{}/{}", self.name, child.name);
            self.dirs = child.dirs;
            self.files = child.files;
        }
        for dir in &mut self.dirs {
            dir.fold();
        }
    }

    fn sort(&mut self) {
        self.dirs.sort_by_key(|dir| dir.name.to_lowercase());
        self.files.sort_by_key(|(name, _)| name.to_lowercase());
        for dir in &mut self.dirs {
            dir.sort();
        }
    }

    fn flatten(&self, depth: usize, prefix: &str, rows: &mut Vec<Row>) {
        for dir in &self.dirs {
            let key = format!("{prefix}{}/", dir.name);
            rows.push(Row {
                depth,
                name: SharedString::from(format!("{}/", dir.name)),
                kind: RowKind::Dir {
                    key: SharedString::from(key.clone()),
                    files: dir.file_count(),
                },
            });
            dir.flatten(depth + 1, &key, rows);
        }
        for (name, entry) in &self.files {
            rows.push(Row {
                depth,
                name: SharedString::from(name.clone()),
                kind: RowKind::File { entry: *entry },
            });
        }
    }
}

impl TryFrom<Spec> for FileTree {
    type Error = String;

    fn try_from(spec: Spec) -> Result<Self, String> {
        let mut root = Dir::default();
        let mut entries = Vec::new();
        for entry in spec.entries {
            let parts = segments(&entry.path);
            if parts.is_empty() {
                continue;
            }
            root.insert(&parts, entries.len());
            entries.push(Entry {
                path: SharedString::from(parts.join("/")),
                change: entry.change,
                note: entry
                    .note
                    .map(|note| note.trim().to_owned())
                    .filter(|note| !note.is_empty())
                    .map(SharedString::from),
            });
        }
        if entries.is_empty() {
            return Err("`entries` names no file".to_owned());
        }
        for dir in &mut root.dirs {
            dir.fold();
        }
        root.sort();
        let mut rows = Vec::new();
        root.flatten(0, "", &mut rows);
        Ok(Self(Rc::new(Model { entries, rows })))
    }
}

impl RichBlock for FileTree {
    fn render(&self, cx: &BlockContext<'_>) -> AnyElement {
        FileTreeView {
            key: cx.key("tree"),
            theme: *cx.theme,
            model: Rc::clone(&self.0),
        }
        .into_any_element()
    }

    fn markdown(&self) -> String {
        let model = &self.0;
        let mut out = String::new();
        for row in &model.rows {
            let indent = "  ".repeat(row.depth);
            match row.kind {
                RowKind::Dir { .. } => {
                    let _ = writeln!(out, "{indent}- **{}**", row.name);
                }
                RowKind::File { entry } => {
                    let entry = &model.entries[entry];
                    let _ = write!(out, "{indent}- `{}`", row.name);
                    if let Some(change) = entry.change {
                        let _ = write!(out, " ({})", change.word());
                    }
                    if let Some(note) = &entry.note {
                        let _ = write!(out, " — {note}");
                    }
                    out.push('\n');
                }
            }
        }
        out
    }
}

#[derive(Default)]
struct TreeState {
    collapsed: HashSet<SharedString>,
}

#[derive(IntoElement)]
struct FileTreeView {
    key: SharedString,
    theme: ArtisanTheme,
    model: Rc<Model>,
}

impl FileTreeView {
    fn id(&self, part: impl std::fmt::Display) -> gpui::ElementId {
        gpui::ElementId::Name(SharedString::from(format!("{}-{part}", self.key)))
    }
}

impl FileTreeView {
    /// A row's clickable frame, indented to its depth.
    fn frame(&self, index: usize, depth: usize) -> Stateful<Div> {
        let theme = self.theme;
        #[expect(clippy::cast_precision_loss, reason = "tree depth stays small")]
        let indent = px(4.0 + depth as f32 * 16.0);
        div()
            .id(self.id(format!("row-{index}")))
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(6.0))
            .h(px(26.0))
            .pl(indent)
            .pr(px(8.0))
            .rounded(px(6.0))
            .cursor_pointer()
            .hover(move |style| crate::hover_card::hover_fill_style(style, &theme, px(6.0)))
    }

    /// A directory row; clicking collapses or expands it.
    fn dir(
        &self,
        frame: Stateful<Div>,
        row: &Row,
        key: &SharedString,
        files: usize,
        open: bool,
        state: &Entity<TreeState>,
    ) -> Stateful<Div> {
        let theme = &self.theme;
        let key = key.clone();
        let state = state.clone();
        frame
            .child(chevron(theme, open, 12.0))
            .child(kit::glyph(
                if open {
                    AssetId::TABLER_FOLDER_OPEN
                } else {
                    AssetId::TABLER_FOLDER
                },
                14.0,
                theme.colors.muted_foreground.to_paint(),
            ))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(13.0))
                    .text_color(theme.colors.foreground.to_paint())
                    .child(row.name.clone()),
            )
            .when(!open, |row| {
                row.child(kit::label(theme, counted(files, "file")))
            })
            .on_click(move |_, _, cx| {
                state.update(cx, |state, cx| {
                    if !state.collapsed.remove(&key) {
                        state.collapsed.insert(key.clone());
                    }
                    cx.notify();
                });
            })
    }

    /// A file row: change-tinted glyph, name, badge, and note; clicking
    /// opens the file.
    fn file(&self, frame: Stateful<Div>, row: &Row, entry: &Entry) -> Stateful<Div> {
        let theme = &self.theme;
        let muted = theme.colors.muted_foreground.to_paint();
        let path = entry.path.clone();
        let tone = entry.change.map_or(Tone::Neutral, Change::tone);
        let deleted = entry.change == Some(Change::Deleted);
        frame
            .child(div().w(px(12.0)).flex_shrink_0())
            .child(kit::glyph(
                if deleted {
                    AssetId::TABLER_FILE_X
                } else {
                    AssetId::TABLER_FILE_TEXT
                },
                14.0,
                if tone == Tone::Neutral {
                    muted
                } else {
                    tone.color(theme).to_paint()
                },
            ))
            .child(
                div()
                    .min_w_0()
                    .flex_shrink_0()
                    .max_w(gpui::relative(0.6))
                    .truncate()
                    .font_family(theme.typography.mono.family)
                    .tracking(0.0)
                    .text_size(px(13.0))
                    .text_color(if deleted {
                        muted
                    } else {
                        theme.colors.foreground.to_paint()
                    })
                    .when(deleted, gpui::Styled::line_through)
                    .child(row.name.clone()),
            )
            .children(entry.change.map(|change| {
                div()
                    .flex_shrink_0()
                    .font_family(theme.typography.mono.family)
                    .tracking(0.0)
                    .text_size(px(12.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(change.tone().color(theme).to_paint())
                    .child(change.letter())
            }))
            .children(entry.note.clone().map(|note| {
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(12.0))
                    .text_color(muted)
                    .child(note)
            }))
            .on_click(move |_, window, cx| open_reference(&path, None, window, cx))
    }
}

impl RenderOnce for FileTreeView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let model = Rc::clone(&self.model);
        let state = kit::keyed(window, cx, self.id("state"), TreeState::default);
        let collapsed = state.read(cx).collapsed.clone();

        let mut rows = Vec::new();
        let mut hidden_below: Option<usize> = None;
        for (index, row) in model.rows.iter().enumerate() {
            if let Some(depth) = hidden_below {
                if row.depth > depth {
                    continue;
                }
                hidden_below = None;
            }
            let frame = self.frame(index, row.depth);
            rows.push(match &row.kind {
                RowKind::Dir { key, files } => {
                    let open = !collapsed.contains(key);
                    if !open {
                        hidden_below = Some(row.depth);
                    }
                    self.dir(frame, row, key, *files, open, &state)
                }
                RowKind::File { entry } => self.file(frame, row, &model.entries[*entry]),
            });
        }

        let summary = Change::ALL.into_iter().filter_map(|change| {
            let count = model
                .entries
                .iter()
                .filter(|entry| entry.change == Some(change))
                .count();
            (count > 0).then(|| {
                kit::label(&theme, format!("{count} {}", change.word()))
                    .text_color(change.tone().color(&theme).to_paint())
            })
        });

        div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(10.0))
                    .child(kit::label(&theme, counted(model.entries.len(), "file")))
                    .children(summary),
            )
            .child(div().w_full().min_w_0().flex().flex_col().children(rows))
    }
}
