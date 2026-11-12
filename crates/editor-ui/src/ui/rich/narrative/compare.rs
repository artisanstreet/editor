use crate::ui::kit::TextRole;
use serde::Deserialize;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use super::callout::text;
use crate::ui::theme::{ArtisanTheme, Opacity};
use aqua_core::geometry::CornerRadii;
use aqua_core::{DisplayItem, GpuiBackground, InputEvent, Insets, Key, Modifiers, NamedKey, Role};
use aqua_ui::collection::List;
use aqua_ui::element::{Element, ui};
use aqua_ui::input::{EventPhase, InputKind, InputResponse, RoutedInput};
use aqua_ui::style::{Length, Style};

#[derive(Debug, Deserialize)]
struct CompareSpec {
    options: Vec<String>,
    rows: Vec<RowSpec>,
    #[serde(default)]
    recommended: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct RowSpec {
    criterion: String,
    #[serde(default)]
    values: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(try_from = "CompareSpec")]
pub(crate) struct Compare(Arc<CompareData>);
#[derive(Debug)]
struct CompareData {
    options: Vec<String>,
    rows: Vec<Row>,
    recommended: Option<usize>,
}
#[derive(Debug)]
struct Row {
    criterion: String,
    values: Vec<String>,
}
impl Compare {
    pub(crate) fn resident_bytes(&self) -> usize {
        use crate::ui::rich::kit::{retained_strings, retained_vec};
        std::mem::size_of::<Self>()
            + std::mem::size_of::<CompareData>()
            + retained_strings(&self.0.options)
            + retained_vec(&self.0.rows)
            + self
                .0
                .rows
                .iter()
                .map(|row| row.criterion.capacity() + retained_strings(&row.values))
                .sum::<usize>()
    }
}

#[derive(Clone, Default)]
pub(crate) struct CompareState(
    Rc<RefCell<HashMap<String, (std::sync::Weak<CompareData>, Option<usize>)>>>,
);

impl CompareState {
    pub(crate) fn remove(&self, id: &str) {
        self.0.borrow_mut().remove(id);
    }
}

pub(crate) type NavigationFrame = Rc<dyn Fn(Element, aqua_core::GpuiHsla) -> Element>;

fn cell(
    content: Element,
    id: String,
    column: usize,
    count: usize,
    selected: Option<usize>,
    theme: ArtisanTheme,
    states: CompareState,
    block: String,
    invalidate: Rc<dyn Fn()>,
) -> Element {
    #[expect(
        clippy::cast_precision_loss,
        reason = "comparison is limited to sixteen options"
    )]
    let width = 76.0 / count as f32;
    let mut style = Style::new()
        .width(Length::Pct(width))
        .min_width(Length::Px(0.0))
        .padding_insets(Insets {
            left: 10.0,
            right: 10.0,
            top: 7.0,
            bottom: 7.0,
        });
    if selected == Some(column) {
        style = style.gpui_background(GpuiBackground::Solid(
            theme
                .colors
                .text
                .at(Opacity::HoverBottom)
                .to_reference_paint(),
        ));
    }
    ui::Box::new()
        .style(style)
        .child(content)
        .build()
        .with_key(id)
        .on_hover_change(move |hovered| {
            let mut map = states.0.borrow_mut();
            if let Some((_, selection)) = map.get_mut(&block) {
                if hovered {
                    *selection = Some(column);
                } else if *selection == Some(column) {
                    *selection = None;
                }
            }
            drop(map);
            invalidate();
        })
}

fn criterion(theme: &ArtisanTheme, label: &str) -> Element {
    ui::Box::new()
        .style(
            Style::new()
                .width(Length::Pct(24.0))
                .flex_shrink(0.0)
                .padding_insets(Insets {
                    left: 10.0,
                    right: 10.0,
                    top: 7.0,
                    bottom: 7.0,
                }),
        )
        .child(text(theme, label, TextRole::Label, theme.colors.text_muted))
        .build()
}

impl Compare {
    pub(crate) fn view(
        &self,
        id: String,
        theme: &ArtisanTheme,
        states: CompareState,
        navigation_frame: NavigationFrame,
        invalidate: Rc<dyn Fn()>,
    ) -> Element {
        let selected = {
            let mut state = states.0.borrow_mut();
            let entry = state
                .entry(id.clone())
                .or_insert_with(|| (Arc::downgrade(&self.0), None));
            if !std::sync::Weak::ptr_eq(&entry.0, &Arc::downgrade(&self.0)) {
                *entry = (Arc::downgrade(&self.0), None);
            }
            entry.1
        };
        let theme = *theme;
        let count = self.0.options.len();
        #[expect(
            clippy::cast_precision_loss,
            reason = "comparison is limited to sixteen options"
        )]
        let minimum = count as f32 * 96.0 + 128.0;
        let mut header = ui::Box::new()
            .layout("row")
            .style(Style::new().align(aqua_ui::AlignItems::END))
            .child(criterion(&theme, ""));
        for (column, option) in self.0.options.iter().enumerate() {
            let recommended = self.0.recommended == Some(column);
            let color = if recommended {
                theme.colors.link
            } else {
                theme.colors.text
            };
            let mut content = ui::Box::new().layout("column");
            if recommended {
                content = content.child(text(&theme, "Recommended", TextRole::Caption, color));
            }
            content = content.child(text(&theme, option, TextRole::Title, color));
            header = header.child(cell(
                content.build(),
                format!("{id}-option-{column}"),
                column,
                count,
                selected,
                theme,
                states.clone(),
                id.clone(),
                invalidate.clone(),
            ));
        }
        let data = self.0.clone();
        let row_states = states.clone();
        let row_id = id.clone();
        let row_invalidate = invalidate.clone();
        let rows = List::new()
            .items(0..data.rows.len())
            .key(|index| *index)
            .virtualize()
            .estimated_row_height(Length::Px(96.0))
            .style(
                Style::new()
                    .width(Length::Pct(100.0))
                    .min_width(Length::Px(minimum))
                    .height(Length::Px(
                        crate::ui::rich::charts::scale::f32_of_count(data.rows.len().min(6)) * 40.0,
                    )),
            )
            .row(move |index| {
                let row = &data.rows[*index];
                let border = theme.colors.text.at(Opacity::Rule).to_reference_paint();
                let mut view = ui::Canvas::new()
                    .layout("row")
                    .style(
                        Style::new()
                            .width(Length::Pct(100.0))
                            .min_width(Length::Px(minimum)),
                    )
                    .on_paint(move |bounds| {
                        vec![DisplayItem::GpuiBorder {
                            bounds,
                            corners: CornerRadii::uniform(0.0),
                            widths: Insets {
                                top: 1.0,
                                left: 0.0,
                                right: 0.0,
                                bottom: 0.0,
                            },
                            color: border,
                            dashed: false,
                        }]
                    })
                    .child(criterion(&theme, &row.criterion));
                for column in 0..count {
                    let value = row.values.get(column).map(String::as_str).unwrap_or("");
                    let (value, color) = if value.is_empty() {
                        ("–", theme.colors.text_muted)
                    } else {
                        (value, theme.colors.text)
                    };
                    view = view.child(cell(
                        text(&theme, value, TextRole::Label, color),
                        format!("{row_id}-row{index}-{column}"),
                        column,
                        count,
                        selected,
                        theme,
                        row_states.clone(),
                        row_id.clone(),
                        row_invalidate.clone(),
                    ));
                }
                view.build()
            })
            .build()
            .with_key(format!("{id}-rows"));
        let table = ui::Scroll::new()
            .width(Length::Pct(100.0))
            .child(
                ui::Box::new()
                    .layout("column")
                    .style(
                        Style::new()
                            .width(Length::Pct(100.0))
                            .min_width(Length::Px(minimum)),
                    )
                    .child(header.build())
                    .child(rows)
                    .build(),
            )
            .build();
        let mut label =
            "Comparison. Arrows select options; Escape clears the highlight.".to_owned();
        if let Some(option) = selected.and_then(|index| self.0.options.get(index)) {
            label.push(' ');
            label.push_str(option);
        }
        let state = states.clone();
        let key = id.clone();
        let mut root = ui::Box::new()
            .width(Length::Pct(100.0))
            .child(table)
            .build()
            .with_key(format!("{id}-navigation"))
            .role(Role::Group)
            .focusable(true)
            .tab_index(0)
            .aria_label(label)
            .on_input(InputKind::KeyDown, EventPhase::Bubble, move |event| {
                let RoutedInput::Native(InputEvent::KeyDown(event_key)) = &event.input else {
                    return InputResponse::default();
                };
                let Key::Named(keycode) = event_key.key else {
                    return InputResponse::default();
                };
                let mut values = state.0.borrow_mut();
                let Some((_, selected)) = values.get_mut(&key) else {
                    return InputResponse::default();
                };
                let next = match keycode {
                    NamedKey::Escape | NamedKey::Backspace => None,
                    NamedKey::Left if event_key.modifiers.contains(Modifiers::ALT) => None,
                    NamedKey::Home => Some(0),
                    NamedKey::End => Some(count - 1),
                    NamedKey::Left | NamedKey::Up => {
                        Some(selected.map_or(0, |index| index.saturating_sub(1)))
                    }
                    NamedKey::Right | NamedKey::Down => {
                        Some(selected.map_or(0, |index| (index + 1).min(count - 1)))
                    }
                    NamedKey::Enter => Some(selected.unwrap_or(0)),
                    _ => return InputResponse::default(),
                };
                *selected = next;
                drop(values);
                invalidate();
                InputResponse {
                    stop_propagation: true,
                    ..InputResponse::default()
                }
            });
        if let Some(option) = selected.and_then(|index| self.0.options.get(index)) {
            root = root.aria_value(option.clone());
        }
        navigation_frame(root, theme.colors.ring.to_reference_paint())
    }
}
impl TryFrom<CompareSpec> for Compare {
    type Error = String;
    fn try_from(spec: CompareSpec) -> Result<Self, String> {
        if spec.options.is_empty() || spec.options.len() > 16 || spec.rows.len() > 4096 {
            return Err("comparison preview needs 1–16 options and at most 4096 criteria".into());
        }
        let recommended = spec.recommended.as_ref().and_then(|wanted| {
            spec.options
                .iter()
                .position(|option| option.trim().eq_ignore_ascii_case(wanted.trim()))
        });
        let text = |text: String| String::from(text.trim().to_owned());
        Ok(Self(Arc::new(CompareData {
            options: spec.options.into_iter().map(text).collect(),
            rows: spec
                .rows
                .into_iter()
                .map(|row| Row {
                    criterion: text(row.criterion),
                    values: row.values.into_iter().map(text).collect(),
                })
                .collect(),
            recommended,
        })))
    }
}
