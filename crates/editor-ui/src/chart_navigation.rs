//! One focusable navigation surface over chart geometry. Charts map stable
//! semantic ordinals to their readout/zoom state; points are not widgets.
use crate::theme::ArtisanTheme;
use gpui::{
    AnyElement, App, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    RenderOnce, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
};
use std::ops::Range;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChartCommand {
    Select(usize),
    Activate(usize),
    Parent,
    Reset,
}
type Navigate = Box<dyn Fn(ChartCommand, &mut Window, &mut App)>;
#[derive(IntoElement)]
pub struct ChartNavigation {
    id: ElementId,
    theme: ArtisanTheme,
    label: SharedString,
    range: Range<usize>,
    selected: Option<usize>,
    selection_label: Option<SharedString>,
    content: AnyElement,
    navigate: Navigate,
    content_height: bool,
}
impl ChartNavigation {
    pub fn new(
        id: impl Into<ElementId>,
        theme: ArtisanTheme,
        label: impl Into<SharedString>,
        range: Range<usize>,
        content: impl IntoElement,
        navigate: impl Fn(ChartCommand, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            theme,
            label: label.into(),
            range,
            selected: None,
            selection_label: None,
            content: content.into_any_element(),
            navigate: Box::new(navigate),
            content_height: false,
        }
    }
    #[must_use]
    pub const fn content_height(mut self) -> Self {
        self.content_height = true;
        self
    }
    #[must_use]
    pub const fn selected(mut self, selected: Option<usize>) -> Self {
        self.selected = selected;
        self
    }
    #[must_use]
    pub fn selection_label(mut self, label: impl Into<SharedString>) -> Self {
        self.selection_label = Some(label.into());
        self
    }
}
impl RenderOnce for ChartNavigation {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let ring = self.theme.colors.ring.to_paint();
        let range = self.range;
        let selected = self.selected.filter(|index| range.contains(index));
        let navigate = self.navigate;
        let label = self.selection_label.as_ref().map_or_else(
            || self.label.clone(),
            |selection| SharedString::from(format!("{} {}", self.label, selection)),
        );
        let mut root = div()
            .id(self.id)
            .relative()
            .size_full()
            .role(gpui::Role::Group)
            .aria_label(label)
            .focusable()
            .tab_index(0)
            .focus_visible(move |style| style.border_1().border_color(ring))
            .child(self.content);
        if self.content_height {
            root = root.h_auto();
        }
        if let Some(label) = self.selection_label {
            root = root.aria_value(label);
        }
        root.on_key_down(move |event, window, cx| {
            let command = match event.keystroke.key.as_str() {
                "escape" => Some(ChartCommand::Reset),
                "backspace" => Some(ChartCommand::Parent),
                "left" if event.keystroke.modifiers.alt => Some(ChartCommand::Parent),
                "home" if !range.is_empty() => Some(ChartCommand::Select(range.start)),
                "end" if !range.is_empty() => Some(ChartCommand::Select(range.end - 1)),
                "left" | "up" if !range.is_empty() => Some(ChartCommand::Select(
                    selected.map_or(range.start, |index| {
                        index.saturating_sub(1).max(range.start)
                    }),
                )),
                "right" | "down" if !range.is_empty() => Some(ChartCommand::Select(
                    selected.map_or(range.start, |index| {
                        index.saturating_add(1).min(range.end - 1)
                    }),
                )),
                "enter" if !range.is_empty() => {
                    Some(ChartCommand::Activate(selected.unwrap_or(range.start)))
                }
                _ => None,
            };
            if let Some(command) = command {
                navigate(command, window, cx);
                cx.stop_propagation();
            }
        })
    }
}
