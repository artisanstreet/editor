//! `file-tree`: the files a change touched, under directories derived from
//! their paths.
//!
//! Directories form once, at parse: chains of single-child directories
//! fold into one row (`modules/ui/src`), directories sort before files,
//! and the tree flattens into rows in reading order. Collapsing a
//! directory hides the rows nested under it.

use crate::markdown_renderer::rich::budget;
use crate::theme::{ArtisanTheme, TextStep, TypographyTokens};
use std::collections::HashSet;
use std::sync::Arc;

use editor_assets::AssetId;
use gpui::{
    AnyElement, App, Entity, IntoElement, ParentElement as _, RenderOnce, SharedString,
    Styled as _, Window, div, prelude::*, px, uniform_list,
};
use serde::Deserialize;

use super::super::actions::open_reference;
use super::super::kit::{self, Tone};
use super::super::{BlockContext, RichBlock};
use super::counted;
use crate::tree_row::{TreeFocus, TreeNavigation, TreeRow};

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
pub(super) struct FileTree(Arc<Model>);

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
    indices: std::collections::HashMap<String, usize>,
    count: usize,
    files: Vec<(String, usize)>,
}

impl Dir {
    fn insert(&mut self, parts: &[String], entry: usize) {
        self.count += 1;
        match parts {
            [] => {}
            [file] => self.files.push((file.clone(), entry)),
            [dir, rest @ ..] => {
                let index = *self.indices.entry(dir.clone()).or_insert_with(|| {
                    self.dirs.push(Self {
                        name: dir.clone(),
                        ..Self::default()
                    });
                    self.dirs.len() - 1
                });
                self.dirs[index].insert(rest, entry);
            }
        }
    }

    fn file_count(&self) -> usize {
        self.count
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
        if spec.entries.len() > 4096 {
            return Err("file-tree preview supports at most 4096 entries".to_owned());
        }
        let mut root = Dir::default();
        let mut entries = Vec::new();
        for entry in spec.entries {
            let parts = segments(&entry.path);
            if parts.len() > 32 {
                return Err("file-tree paths exceed the 32-level preview budget".to_owned());
            }
            if parts.is_empty() {
                continue;
            }
            root.insert(&parts, entries.len());
            entries.push(Entry {
                path: SharedString::from(entry.path.trim().to_owned()),
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
        Ok(Self(Arc::new(Model { entries, rows })))
    }
}

impl RichBlock for FileTree {
    fn resident_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + std::mem::size_of::<Model>()
            + budget::vector(&self.0.entries)
            + self
                .0
                .entries
                .iter()
                .map(|entry| entry.path.len() + entry.note.as_ref().map_or(0, |note| note.len()))
                .sum::<usize>()
            + budget::vector(&self.0.rows)
            + self
                .0
                .rows
                .iter()
                .map(|row| {
                    row.name.len()
                        + match &row.kind {
                            RowKind::Dir { key, .. } => key.len(),
                            RowKind::File { .. } => 0,
                        }
                })
                .sum::<usize>()
    }

    fn render(&self, cx: &BlockContext<'_>) -> AnyElement {
        FileTreeView {
            key: cx.key("tree"),
            scope: cx.media.and_then(|provider| provider.resource_scope()),
            theme: *cx.theme,
            model: Arc::clone(&self.0),
        }
        .into_any_element()
    }
}

#[derive(Default)]
struct TreeState {
    collapsed: HashSet<SharedString>,
    model: Option<Arc<Model>>,
    focus: TreeFocus,
    scroll: gpui::UniformListScrollHandle,
}

#[derive(IntoElement)]
struct FileTreeView {
    scope: Option<super::super::actions::ResourceScope>,
    key: SharedString,
    theme: ArtisanTheme,
    model: Arc<Model>,
}

impl FileTreeView {
    fn id(&self, part: impl std::fmt::Display) -> gpui::ElementId {
        kit::part(&self.key, part)
    }
}

impl FileTreeView {
    fn node_id(&self, index: usize, row: &Row) -> SharedString {
        node_id(&self.key, index, row, &self.model)
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "one directory row and the tree state it reads"
    )]
    fn dir(
        &self,
        index: usize,
        row: &Row,
        key: &SharedString,
        files: usize,
        open: bool,
        state: &Entity<TreeState>,
        focus: &TreeFocus,
    ) -> TreeRow {
        let theme = self.theme;
        let key = key.clone();
        let state = state.clone();
        let frame = TreeRow::branch(
            focus.node(self.node_id(index, row)),
            theme,
            row.depth,
            row.name.clone(),
            open,
            move |expanded, _, cx| {
                state.update(cx, |state, cx| {
                    if expanded {
                        state.collapsed.remove(&key);
                    } else {
                        state.collapsed.insert(key.clone());
                    }
                    cx.notify();
                });
            },
        );
        if open {
            frame
        } else {
            frame.trailing(kit::label(&theme, counted(files, "file")))
        }
    }
    fn file(&self, index: usize, row: &Row, entry: &Entry, focus: &TreeFocus) -> TreeRow {
        let theme = self.theme;
        let path = entry.path.clone();
        let scope = self.scope.clone();
        let tone = entry.change.map_or(Tone::Neutral, Change::tone);
        let deleted = entry.change == Some(Change::Deleted);
        let mut frame = TreeRow::leaf(
            focus.node(self.node_id(index, row)),
            theme,
            row.depth,
            row.name.clone(),
            path.clone(),
            move |window, cx| open_reference(&path, None, scope.clone(), window, cx),
        )
        .deleted(deleted)
        .leading(kit::glyph(
            if deleted {
                AssetId::TABLER_FILE_X
            } else {
                AssetId::TABLER_FILE_TEXT
            },
            14.0,
            if tone == Tone::Neutral {
                theme.colors.muted_foreground.to_paint()
            } else {
                tone.color(&theme).to_paint()
            },
        ));
        if let Some(change) = entry.change {
            frame = frame.trailing(
                div()
                    .font_family(theme.typography.mono.family)
                    .tracking(0.0)
                    .text_size(TypographyTokens::size(TextStep::Xs))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(change.tone().color(&theme).to_paint())
                    .child(change.letter()),
            );
        }
        if let Some(note) = &entry.note {
            frame = frame.description(note.clone());
        }
        frame
    }
}
fn node_id(key: &str, index: usize, row: &Row, model: &Model) -> SharedString {
    match &row.kind {
        RowKind::Dir { key: path, .. } => format!("{key}-dir-{path}").into(),
        RowKind::File { entry } => {
            format!("{key}-file-{}-{index}", model.entries[*entry].path).into()
        }
    }
}
fn navigation_target(
    model: &Model,
    visible: &[usize],
    index: usize,
    command: TreeNavigation,
) -> usize {
    match command {
        TreeNavigation::Previous => index.saturating_sub(1),
        TreeNavigation::Next => (index + 1).min(visible.len().saturating_sub(1)),
        TreeNavigation::First => 0,
        TreeNavigation::Last => visible.len().saturating_sub(1),
        TreeNavigation::Parent => (0..index)
            .rev()
            .find(|prior| model.rows[visible[*prior]].depth < model.rows[visible[index]].depth)
            .unwrap_or(index),
    }
}

impl RenderOnce for FileTreeView {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let model = Arc::clone(&self.model);
        let state = kit::keyed(window, cx, self.id("state"), TreeState::default);
        state.update(cx, |state, cx| {
            if !state.model.as_ref().is_some_and(|old| Arc::ptr_eq(old, &model)) {
                state.focus.reset(window, cx);
                state.model = Some(model.clone());
                state.collapsed.retain(|key| model.rows.iter().any(|row| matches!(&row.kind, RowKind::Dir { key: current, .. } if current == key)));
                state.scroll = gpui::UniformListScrollHandle::new();
            }
        });
        let collapsed = state.read(cx).collapsed.clone();
        let focus = state.read(cx).focus.clone();
        let scroll = state.read(cx).scroll.clone();
        let navigation_scroll = scroll.clone();

        let mut visible = Vec::new();
        let mut hidden_below: Option<usize> = None;
        for (index, row) in model.rows.iter().enumerate() {
            if let Some(depth) = hidden_below {
                if row.depth > depth {
                    continue;
                }
                hidden_below = None;
            }
            visible.push(index);
            if let RowKind::Dir { key, .. } = &row.kind
                && collapsed.contains(key)
            {
                hidden_below = Some(row.depth);
            }
        }
        let visible: Arc<[usize]> = visible.into();
        let row_count = visible.len();
        let list_id = self.id("rows");
        let tree_id = self.id("tree");
        let row_model = model.clone();
        let list = uniform_list(list_id, row_count, move |range, window, _| {
            focus.retain_visible(
                range
                    .clone()
                    .filter_map(|position| visible.get(position))
                    .map(|index| node_id(&self.key, *index, &row_model.rows[*index], &row_model)),
                window,
            );
            range
                .filter_map(|visible_index| {
                    let index = *visible.get(visible_index)?;
                    let row = &row_model.rows[index];
                    let frame = match &row.kind {
                        RowKind::Dir { key, files } => self.dir(
                            index,
                            row,
                            key,
                            *files,
                            !collapsed.contains(key),
                            &state,
                            &focus,
                        ),
                        RowKind::File { entry } => {
                            self.file(index, row, &row_model.entries[*entry], &focus)
                        }
                    };
                    let visible = visible.clone();
                    let model = row_model.clone();
                    let key = self.key.clone();
                    let focus = focus.clone();
                    let scroll = navigation_scroll.clone();
                    Some(frame.on_navigate(move |command, window, cx| {
                        let target = navigation_target(&model, &visible, visible_index, command);
                        let index = visible[target];
                        scroll.scroll_to_item(target, gpui::ScrollStrategy::Nearest);
                        focus.focus(node_id(&key, index, &model.rows[index], &model), window, cx);
                        window.refresh();
                    }))
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(&scroll)
        .w_full()
        .h(TreeRow::row_height() * row_count.min(14) as f32);

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
            .id(tree_id)
            .role(gpui::Role::Tree)
            .aria_label("Changed files")
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
            .child(list)
    }
}

#[cfg(test)]
mod resource_target_tests {
    use super::*;
    #[test]
    fn keyboard_navigation_uses_visible_order_across_viewport_edges() {
        let entries: Vec<_> = (0..24)
            .map(|index| serde_json::json!({"path": format!("directory/file-{index:02}.rs")}))
            .collect();
        let tree: FileTree =
            serde_json::from_value(serde_json::json!({"entries": entries})).unwrap();
        let model = &tree.0;
        let visible: Vec<_> = (0..model.rows.len()).collect();
        assert_eq!(
            navigation_target(model, &visible, 13, TreeNavigation::Next),
            14
        );
        assert_eq!(
            navigation_target(model, &visible, 0, TreeNavigation::Last),
            24
        );
        assert_eq!(
            navigation_target(model, &visible, 24, TreeNavigation::Parent),
            0
        );
        assert_eq!(navigation_target(model, &[0], 0, TreeNavigation::Next), 0);
        assert_eq!(
            navigation_target(model, &visible, 0, TreeNavigation::Previous),
            0
        );
    }
    #[test]
    fn display_normalization_preserves_absolute_and_unc_resource_targets() {
        for path in [
            "/home/user/project/main.rs",
            "C:\\project\\main.rs",
            "\\\\server\\share\\main.rs",
        ] {
            let tree: FileTree =
                serde_json::from_value(serde_json::json!({"entries":[{"path":path}]})).unwrap();
            assert_eq!(tree.0.entries[0].path.as_ref(), path);
            assert!(!tree.0.rows.is_empty());
        }
    }
}
