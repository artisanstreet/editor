use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    rc::Rc,
    sync::Arc,
};

use aqua_core::{CursorIcon, InputEvent, Insets, Key as InputKey, NamedKey, PointerButton, Role};
use aqua_text::{FontFamily, FontWeight, TextStyle, overflow::OverflowMode};
use aqua_ui::{
    AlignItems,
    collection::List,
    element::{Box as UiBox, Element, Text},
    input::{CaptureRequest, EventPhase, InputKind, InputResponse, RoutedInput},
    style::{Length, Style},
};
use editor_assets::AssetId;

use super::{Change, FileTree, Model, Row, RowKind};
use crate::ui::{
    asset::source_asset_glyph,
    hover::{HoverColors, HoverFill, HoverTone},
    rich::actions::{OpenReference, ResourceScope},
    theme::{ArtisanTheme, Oklch, RadiusStep, RadiusTokens},
};

const ROW_HEIGHT: f32 = 26.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Navigation {
    Previous,
    Next,
    First,
    Last,
    Left,
    Right,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Activate(usize),
    Navigate(usize, Navigation),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Effect {
    pub(crate) changed: bool,
    pub(crate) focus: Option<(String, String, usize)>,
    pub(crate) open: Option<OpenReference>,
}

#[derive(Default)]
pub(crate) struct FileTreeState {
    collapsed: HashSet<String>,
    model: Option<Arc<Model>>,
}

impl FileTreeState {
    pub(crate) fn deactivate(&mut self) {
        self.model = None;
    }

    pub(crate) fn reconcile(&mut self, tree: &FileTree) -> bool {
        if self
            .model
            .as_ref()
            .is_some_and(|old| Arc::ptr_eq(old, &tree.0))
        {
            return false;
        }
        self.collapsed.retain(|key| {
            tree.0
                .rows
                .iter()
                .any(|row| matches!(&row.kind, RowKind::Dir { key: current, .. } if current == key))
        });
        self.model = Some(tree.0.clone());
        true
    }

    fn visible(&self, tree: &FileTree) -> Vec<usize> {
        let mut visible = Vec::new();
        let mut hidden_below = None;
        for (index, row) in tree.0.rows.iter().enumerate() {
            if let Some(depth) = hidden_below {
                if row.depth > depth {
                    continue;
                }
                hidden_below = None;
            }
            visible.push(index);
            if let RowKind::Dir { key, .. } = &row.kind {
                if self.collapsed.contains(key) {
                    hidden_below = Some(row.depth);
                }
            }
        }
        visible
    }

    pub(crate) fn dispatch(
        &mut self,
        tree: &FileTree,
        id: &str,
        scope: Option<ResourceScope>,
        action: Action,
    ) -> Effect {
        if !self
            .model
            .as_ref()
            .is_some_and(|model| Arc::ptr_eq(model, &tree.0))
        {
            return Effect::default();
        }
        let index = match action {
            Action::Activate(index) | Action::Navigate(index, _) => index,
        };
        let visible = self.visible(tree);
        let Some(position) = visible.iter().position(|current| *current == index) else {
            return Effect::default();
        };
        let row = &tree.0.rows[index];
        match action {
            Action::Activate(_) => match &row.kind {
                RowKind::Dir { key, .. } => {
                    if !self.collapsed.remove(key) {
                        self.collapsed.insert(key.clone());
                    }
                    Effect {
                        changed: true,
                        ..Effect::default()
                    }
                }
                RowKind::File { entry } => Effect {
                    open: Some(OpenReference {
                        scope,
                        path: tree.0.entries[*entry].path.clone(),
                        line: None,
                    }),
                    ..Effect::default()
                },
            },
            Action::Navigate(_, command) => {
                if let RowKind::Dir { key, .. } = &row.kind {
                    let collapsed = self.collapsed.contains(key);
                    if command == Navigation::Left && !collapsed {
                        self.collapsed.insert(key.clone());
                        return Effect {
                            changed: true,
                            ..Effect::default()
                        };
                    }
                    if command == Navigation::Right && collapsed {
                        self.collapsed.remove(key);
                        return Effect {
                            changed: true,
                            ..Effect::default()
                        };
                    }
                }
                let target = match command {
                    Navigation::Previous => position.saturating_sub(1),
                    Navigation::Next | Navigation::Right
                        if matches!(command, Navigation::Next)
                            || matches!(&row.kind, RowKind::Dir { .. }) =>
                    {
                        (position + 1).min(visible.len() - 1)
                    }
                    Navigation::First => 0,
                    Navigation::Last => visible.len() - 1,
                    Navigation::Left => (0..position)
                        .rev()
                        .find(|prior| tree.0.rows[visible[*prior]].depth < row.depth)
                        .unwrap_or(position),
                    Navigation::Right => return Effect::default(),
                    Navigation::Next => (position + 1).min(visible.len() - 1),
                };
                let target_index = visible[target];
                Effect {
                    focus: Some((
                        format!("{id}-rows"),
                        node_id(id, target_index, &tree.0.rows[target_index], &tree.0),
                        target,
                    )),
                    ..Effect::default()
                }
            }
        }
    }
}

fn node_id(id: &str, index: usize, row: &Row, model: &Model) -> String {
    match &row.kind {
        RowKind::Dir { key, .. } => format!("{id}-dir-{key}"),
        RowKind::File { entry } => format!("{id}-file-{}-{index}", model.entries[*entry].path),
    }
}

fn color(theme: &ArtisanTheme, change: Option<Change>) -> Oklch {
    match change {
        Some(Change::Added) => theme.colors.banner_success.text,
        Some(Change::Modified) => theme.colors.banner_warning.text,
        Some(Change::Deleted) => theme.colors.banner_error.text,
        Some(Change::Renamed) => theme.colors.banner_info.text,
        Some(Change::Other) | None => theme.colors.text_muted,
    }
}

fn text(
    theme: &ArtisanTheme,
    value: impl Into<String>,
    size: f32,
    line: f32,
    mono: bool,
    weight: FontWeight,
    ink: Oklch,
    strike: bool,
) -> Element {
    Text::new(value.into())
        .text_style(TextStyle {
            families: vec![FontFamily::Named(if mono {
                theme.typography.mono.family.to_owned()
            } else {
                theme.typography.sans.family.to_owned()
            })],
            size_px: size,
            line_height_px: line,
            weight,
            letter_spacing_em: if mono { Some(0.0) } else { None },
            strikethrough: strike,
            ..TextStyle::default()
        })
        .gpui_source_color(ink.to_reference_text_color())
        .wrap(false)
        .overflow(OverflowMode::EndEllipsis)
        .build()
}

fn count(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

fn row(
    tree: &FileTree,
    id: &str,
    index: usize,
    theme: &ArtisanTheme,
    collapsed: &HashSet<String>,
    emit: Rc<dyn Fn(Action)>,
) -> Element {
    let row = &tree.0.rows[index];
    let (expanded, glyph, tint, label_ink, badge, description, accessible, deleted) =
        match &row.kind {
            RowKind::Dir { key, files } => {
                let open = !collapsed.contains(key);
                (
                    Some(open),
                    if open {
                        AssetId::TABLER_FOLDER_OPEN
                    } else {
                        AssetId::TABLER_FOLDER
                    },
                    theme.colors.text_muted,
                    theme.colors.text,
                    (!open).then(|| count(*files, "file")),
                    None,
                    row.name.clone(),
                    false,
                )
            }
            RowKind::File { entry } => {
                let entry = &tree.0.entries[*entry];
                let deleted = entry.change == Some(Change::Deleted);
                (
                    None,
                    if deleted {
                        AssetId::TABLER_FILE_X
                    } else {
                        AssetId::TABLER_FILE_TEXT
                    },
                    color(theme, entry.change),
                    if deleted {
                        theme.colors.text_muted
                    } else {
                        theme.colors.text
                    },
                    entry.change.map(|change| change.letter().to_owned()),
                    entry.note.clone(),
                    entry.path.clone(),
                    deleted,
                )
            }
        };
    let indent = f32::from(u16::try_from(row.depth.min(32)).unwrap_or(32)).mul_add(16.0, 4.0);
    let mut leading = UiBox::new()
        .width(12.0)
        .style(Style::new().flex_shrink(0.0));
    if let Some(open) = expanded {
        leading = leading.child(source_asset_glyph(
            if open {
                AssetId::TABLER_CHEVRON_DOWN
            } else {
                AssetId::TABLER_CHEVRON_RIGHT
            },
            12.0,
            theme.colors.text_muted.to_reference_paint(),
        ));
    }
    let mut content = UiBox::new()
        .width(Length::Pct(100.0))
        .height(ROW_HEIGHT)
        .gap(6.0)
        .radius(RadiusTokens::value(RadiusStep::Sm))
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .min_width(Length::Px(0.0))
                .padding_insets(Insets {
                    left: indent,
                    right: 8.0,
                    ..Insets::default()
                })
                .border_of(1.0, aqua_core::Oklch::TRANSPARENT),
        )
        .child(leading.build())
        .child(source_asset_glyph(glyph, 14.0, tint.to_reference_paint()))
        .child(
            UiBox::new()
                .style(
                    Style::new()
                        .min_width(Length::Px(0.0))
                        .flex_shrink(0.0)
                        .max_width(Length::Pct(60.0)),
                )
                .child(text(
                    theme,
                    row.name.clone(),
                    13.0,
                    18.0,
                    expanded.is_none(),
                    FontWeight::NORMAL,
                    label_ink,
                    deleted,
                ))
                .build(),
        );
    if let Some(badge) = badge {
        let (mono, weight, ink) = if expanded.is_some() {
            (false, FontWeight::MEDIUM, theme.colors.text_muted)
        } else {
            (true, FontWeight(600), tint)
        };
        content = content.child(
            UiBox::new()
                .style(Style::new().flex_shrink(0.0))
                .child(text(theme, badge, 12.0, 16.0, mono, weight, ink, false))
                .build(),
        );
    }
    if let Some(description) = description {
        content = content.child(
            UiBox::new()
                .style(Style::new().flex_grow(1.0).min_width(Length::Px(0.0)))
                .child(text(
                    theme,
                    description,
                    12.0,
                    16.0,
                    false,
                    FontWeight::NORMAL,
                    theme.colors.text_muted,
                    false,
                ))
                .build(),
        );
    }
    let pressed = Rc::new(Cell::new(None));
    let down = pressed.clone();
    let cancelled = pressed.clone();
    let click = emit.clone();
    let accessibility = emit.clone();
    let key_emit = emit;
    let mut element = content
        .build()
        .with_key(node_id(id, index, row, &tree.0))
        .role(Role::TreeItem)
        .aria_label(accessible)
        .aria_level(row.depth.saturating_add(1))
        .on_accessibility_activate(move || accessibility(Action::Activate(index)))
        .focusable(true)
        .cursor(CursorIcon::Pointer)
        .hover_style(HoverColors::new(theme, HoverTone::Neutral).style(HoverFill::Edged))
        .on_input(InputKind::PointerDown, EventPhase::Bubble, move |event| {
            if let RoutedInput::Native(InputEvent::PointerDown(pointer, PointerButton::Primary)) =
                &event.input
            {
                down.set(Some(pointer.contact));
                InputResponse::capture()
            } else {
                InputResponse::default()
            }
        })
        .on_input(InputKind::PointerUp, EventPhase::Bubble, move |event| {
            let was_pressed = pressed.take();
            if let RoutedInput::Native(InputEvent::PointerUp(pointer, PointerButton::Primary)) =
                &event.input
            {
                if was_pressed == Some(pointer.contact) && event.bounds.contains(pointer.position) {
                    click(Action::Activate(index));
                }
            }
            InputResponse {
                pointer_capture: CaptureRequest::Release,
                ..InputResponse::default()
            }
        })
        .on_input(InputKind::PointerCancel, EventPhase::Bubble, move |_| {
            cancelled.set(None);
            InputResponse {
                pointer_capture: CaptureRequest::Release,
                ..InputResponse::default()
            }
        })
        .focus_visible_style(Style::new().gpui_border(
            Insets {
                left: 1.0,
                right: 1.0,
                top: 1.0,
                bottom: 1.0,
            },
            theme.colors.ring.to_reference_paint(),
            false,
        ))
        .on_input(InputKind::KeyDown, EventPhase::Bubble, move |event| {
            let RoutedInput::Native(InputEvent::KeyDown(key)) = &event.input else {
                return InputResponse::default();
            };
            let command = match key.key {
                InputKey::Named(NamedKey::Up) => Navigation::Previous,
                InputKey::Named(NamedKey::Down) => Navigation::Next,
                InputKey::Named(NamedKey::Home) => Navigation::First,
                InputKey::Named(NamedKey::End) => Navigation::Last,
                InputKey::Named(NamedKey::Left) => Navigation::Left,
                InputKey::Named(NamedKey::Right) if expanded.is_some() => Navigation::Right,
                _ => return InputResponse::default(),
            };
            key_emit(Action::Navigate(index, command));
            InputResponse {
                stop_propagation: true,
                ..InputResponse::handled()
            }
        });
    if let Some(open) = expanded {
        element = element.aria_expanded(open);
    }
    element
}

impl FileTree {
    pub(crate) fn render(
        &self,
        theme: &ArtisanTheme,
        id: &str,
        state: Rc<RefCell<FileTreeState>>,
        emit: Rc<dyn Fn(Action)>,
    ) -> Element {
        state.borrow_mut().reconcile(self);
        let visible = state.borrow().visible(self);
        let collapsed = state.borrow().collapsed.clone();
        let mut summary = UiBox::new()
            .gap(10.0)
            .style(Style::new().align(AlignItems::CENTER).wrap(true))
            .child(text(
                theme,
                count(self.0.entries.len(), "file"),
                12.0,
                16.0,
                false,
                FontWeight::MEDIUM,
                theme.colors.text_muted,
                false,
            ));
        for change in Change::ALL {
            let count = self
                .0
                .entries
                .iter()
                .filter(|entry| entry.change == Some(change))
                .count();
            if count > 0 {
                summary = summary.child(text(
                    theme,
                    format!("{count} {}", change.word()),
                    12.0,
                    16.0,
                    false,
                    FontWeight::MEDIUM,
                    color(theme, Some(change)),
                    false,
                ));
            }
        }
        let height = f32::from(u16::try_from(visible.len().min(14)).unwrap_or(14)) * ROW_HEIGHT;
        let tree = FileTree(self.0.clone());
        let row_id = id.to_owned();
        let theme = *theme;
        let list = List::new()
            .items(visible)
            .key(|index| *index)
            .row(move |index| row(&tree, &row_id, *index, &theme, &collapsed, emit.clone()))
            .style(
                Style::new()
                    .width(Length::Pct(100.0))
                    .height(Length::Px(height)),
            )
            .estimated_row_height(Length::Px(ROW_HEIGHT))
            .measured_uniform_rows()
            .virtualize()
            .build()
            .with_key(format!("{id}-rows"));
        UiBox::new()
            .layout("column")
            .gap(6.0)
            .width(Length::Pct(100.0))
            .style(Style::new().min_width(Length::Px(0.0)))
            .child(summary.build())
            .child(list)
            .build()
            .with_key(format!("{id}-tree"))
            .role(Role::Tree)
            .aria_label("Changed files")
    }
}

#[cfg(test)]
#[path = "../../../../../tests/ui/rich/code/file_tree.rs"]
mod tests;
