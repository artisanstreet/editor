use super::{Heatmap, HeatmapData, Scale};
use crate::ui::rich::charts::scale::{f32_of, f32_of_count, f64_of, reading};
use crate::ui::rich::charts::scene::{self, Probe, Shape};
use crate::ui::rich::kit::{diverging_color, sequential_color};
use crate::ui::theme::{ArtisanTheme, Oklch, RadiusStep, RadiusTokens};
use aqua_core::{GpuiBackground, InputEvent, Insets, Key, Modifiers, NamedKey, Role};
use aqua_text::FontWeight;
use aqua_ui::collection::List;
use aqua_ui::element::{Element, ui};
use aqua_ui::input::{EventPhase, InputKind, InputResponse, RoutedInput};
use aqua_ui::style::{Length, Style};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TextRole {
    RowLabel,
    Tick,
    Ramp,
}
pub(crate) type TextFactory = Rc<dyn Fn(TextRole, &str, f32, f32, FontWeight, Oklch) -> Element>;
pub(crate) type EmptyFactory = Rc<dyn Fn(&str) -> Element>;
pub(crate) type ReadoutFactory = Rc<dyn Fn(f32, &str, &str) -> Element>;
pub(crate) type NavigationFrame = Rc<dyn Fn(Element, aqua_core::GpuiHsla) -> Element>;
pub(crate) type EnvelopeTextFactory = Rc<dyn Fn(&str, crate::ui::kit::TextRole, Oklch) -> Element>;

pub(crate) fn envelope(
    theme: &ArtisanTheme,
    title: Option<&str>,
    subtitle: Option<&str>,
    caption: Option<&str>,
    content: Element,
    text: EnvelopeTextFactory,
) -> Element {
    use crate::ui::kit::{Space, Spaced, TextRole as Type};
    let mut body = ui::Box::new().layout("column").spaced(Space::S12).style(
        Style::new()
            .width(Length::Pct(100.0))
            .min_width(Length::Px(0.0)),
    );
    if title.is_some() || subtitle.is_some() {
        let mut heading = ui::Box::new().layout("column").spaced(Space::S2);
        if let Some(title) = title {
            heading = heading.child(text(title, Type::Title, theme.colors.text));
        }
        if let Some(subtitle) = subtitle {
            heading = heading.child(text(subtitle, Type::Label, theme.colors.text_muted));
        }
        body = body.child(heading.build());
    }
    body = body.child(content);
    if let Some(caption) = caption {
        body = body.child(text(caption, Type::Caption, theme.colors.text_muted));
    }
    body.build()
}

struct Entry {
    model: Arc<HeatmapData>,
    selected: Option<(usize, usize)>,
    plot: Probe,
    reveal: Option<usize>,
    reset: u64,
}

#[derive(Clone, Default)]
pub(crate) struct HeatmapState(Rc<RefCell<HashMap<String, Entry>>>);

impl HeatmapState {
    fn sync(&self, id: &str, model: &Heatmap) -> (Option<(usize, usize)>, Probe, u64) {
        let mut entries = self.0.borrow_mut();
        let entry = entries.entry(id.to_owned()).or_insert_with(|| Entry {
            model: model.0.clone(),
            selected: None,
            plot: Rc::new(Cell::new(None)),
            reveal: None,
            reset: 0,
        });
        if !Arc::ptr_eq(&entry.model, &model.0) {
            if *entry.model != *model.0 {
                entry.selected = None;
                entry.reveal = None;
                entry.reset = entry.reset.wrapping_add(1);
            }
            entry.model = model.0.clone();
        }
        (entry.selected, entry.plot.clone(), entry.reset)
    }

    fn hover(&self, id: &str, row: usize, columns: usize, x: f32, y: f32) -> bool {
        let selected = if columns > 0 && (0.0..1.0).contains(&x) && (0.0..1.0).contains(&y) {
            Some((
                row,
                super::super::scale::index_of(f64::from(x) * f64_of(columns)).min(columns - 1),
            ))
        } else {
            None
        };
        let mut entries = self.0.borrow_mut();
        let Some(entry) = entries.get_mut(id) else {
            return false;
        };
        let changed = entry.selected != selected;
        entry.selected = selected;
        changed
    }

    fn key(&self, id: &str, rows: usize, columns: usize, code: NamedKey, alt: bool) -> bool {
        let mut entries = self.0.borrow_mut();
        let Some(entry) = entries.get_mut(id) else {
            return false;
        };
        let count = rows.saturating_mul(columns);
        if count == 0 {
            return false;
        }
        let current = entry.selected.map(|(row, column)| row * columns + column);
        let next = match code {
            NamedKey::Escape | NamedKey::Backspace => None,
            NamedKey::Left if alt => None,
            NamedKey::Home => Some(0),
            NamedKey::End => Some(count - 1),
            NamedKey::Left | NamedKey::Up => {
                Some(current.map_or(0, |index| index.saturating_sub(1)))
            }
            NamedKey::Right | NamedKey::Down => {
                Some(current.map_or(0, |index| index.saturating_add(1).min(count - 1)))
            }
            NamedKey::Enter => Some(current.unwrap_or(0)),
            _ => return false,
        };
        entry.selected = next.map(|index| (index / columns, index % columns));
        entry.reveal = next.map(|index| index / columns);
        true
    }
    pub(crate) fn pending_reveal(&self, id: &str) -> Option<(String, usize)> {
        let states = self.0.borrow();
        let entry = states.get(id)?;
        let index = entry.reveal?;
        Some((format!("{id}-rows-{}", entry.reset), index))
    }

    pub(crate) fn revealed(&self, id: &str, key: &str, index: usize) {
        if let Some(entry) = self.0.borrow_mut().get_mut(id)
            && key == format!("{id}-rows-{}", entry.reset)
            && entry.reveal == Some(index)
        {
            entry.reveal = None;
        }
    }

    pub(crate) fn remove(&self, id: &str) {
        self.0.borrow_mut().remove(id);
    }
}

impl Heatmap {
    pub(crate) fn view(
        &self,
        id: String,
        theme: &ArtisanTheme,
        states: HeatmapState,
        text: TextFactory,
        empty: EmptyFactory,
        readout: ReadoutFactory,
        navigation_frame: NavigationFrame,
        invalidate: Rc<dyn Fn()>,
    ) -> Element {
        let (rows, columns) = (self.rows(), self.columns());
        if rows == 0 || columns == 0 || !self.has_data {
            return empty("No data to map");
        }
        let (selected, plot, reset) = states.sync(&id, self);
        let theme = *theme;
        let model = self.clone();
        let row_states = states.clone();
        let row_id = id.clone();
        let row_text = text.clone();
        let row_invalidate = invalidate.clone();
        let height = theme.density.control_sm;
        let list = List::new()
            .items(0..rows)
            .key(|row| *row)
            .virtualize()
            .estimated_row_height(Length::Px(height))
            .style(
                Style::new()
                    .width(Length::Pct(100.0))
                    .height(Length::Px(height * f32_of_count(rows.min(10)))),
            )
            .row(move |row| {
                let row = *row;
                let probe: Probe = Rc::new(Cell::new(None));
                let moving = probe.clone();
                let state = row_states.clone();
                let key = row_id.clone();
                let invalidate = row_invalidate.clone();
                let content = row_canvas(&model, &theme, row, selected, probe);
                let plot = ui::Box::new()
                    .style(
                        Style::new()
                            .flex_grow(1.0)
                            .min_width(Length::Px(0.0))
                            .height(Length::Pct(100.0)),
                    )
                    .child(content)
                    .build()
                    .on_input(InputKind::PointerMove, EventPhase::Bubble, move |event| {
                        let RoutedInput::Native(InputEvent::PointerMove(pointer)) = &event.input
                        else {
                            return InputResponse::default();
                        };
                        let (x, y) = scene::fractions(&moving, pointer.position)
                            .unwrap_or((f32::NAN, f32::NAN));
                        let changed = state.hover(&key, row, columns, x, y);
                        if changed {
                            invalidate();
                        }
                        InputResponse::default()
                    });
                ui::Box::new()
                    .layout("row")
                    .style(
                        Style::new()
                            .width(Length::Pct(100.0))
                            .height(Length::Px(height)),
                    )
                    .child(
                        ui::Box::new()
                            .style(
                                Style::new()
                                    .width(Length::Px(96.0))
                                    .min_width(Length::Px(0.0))
                                    .padding_insets(Insets {
                                        right: 8.0,
                                        ..Insets::default()
                                    }),
                            )
                            .child(row_text(
                                TextRole::RowLabel,
                                &model.y_label(row),
                                theme.typography.control_text,
                                height,
                                FontWeight::NORMAL,
                                theme.colors.text_muted,
                            ))
                            .build(),
                    )
                    .child(plot)
                    .build()
                    .with_key(format!("{row_id}-row-{row}"))
            })
            .build()
            .with_key(format!("{id}-rows-{reset}"));
        let width = plot.get().map_or(480.0, |rect| rect.w);
        let budget =
            super::super::scale::index_of(f64::from((width / 64.0).floor().clamp(2.0, 24.0)));
        let stride = columns.div_ceil(budget.max(1)).max(1);
        let mut ticks = ui::Canvas::new().style(
            Style::new()
                .height(Length::Px(16.0))
                .width(Length::Pct(100.0)),
        );
        for column in (0..columns).step_by(stride) {
            let at = f32_of((f64_of(column) + 0.5) / f64_of(columns));
            let left = width * at - 44.0;
            ticks = ticks.child(
                ui::Box::new()
                    .style(
                        Style::new()
                            .absolute_at(Some(left), Some(0.0), None, None)
                            .width(Length::Px(88.0))
                            .justify(aqua_ui::JustifyContent::CENTER),
                    )
                    .child(text(
                        TextRole::Tick,
                        &self.x_label(column),
                        11.0,
                        16.0,
                        FontWeight::NORMAL,
                        theme.colors.text_muted,
                    ))
                    .build(),
            );
        }
        let plot_probe = plot.clone();
        let mut content = ui::Box::new()
            .layout("column")
            .width(Length::Pct(100.0))
            .child(list)
            .child(
                ui::Box::new()
                    .style(Style::new().padding_insets(Insets {
                        left: 96.0,
                        ..Insets::default()
                    }))
                    .child(ticks.build())
                    .build(),
            )
            .child(
                ui::Canvas::new()
                    .style(
                        Style::new()
                            .absolute_at(Some(96.0), Some(0.0), Some(0.0), None)
                            .height(Length::Px(1.0)),
                    )
                    .on_paint(move |bounds| {
                        plot_probe.set(Some(bounds));
                        Vec::new()
                    })
                    .build(),
            );
        let mut selection_label = None;
        if let Some((row, column)) = selected {
            let value = self.cell(row, column).map_or_else(
                || "–".to_owned(),
                |value| reading(value, self.unit.as_deref()),
            );
            let heading = format!("{} · {}", self.y_label(row), self.x_label(column));
            selection_label = Some(format!("{heading}: {value}"));
            content = content.child(readout(
                f32_of((f64_of(column) + 0.5) / f64_of(columns)),
                &heading,
                &value,
            ));
        }
        let mut label = "Heatmap. Arrows explore cells; Home and End select endpoints; Escape clears the reading.".to_owned();
        if let Some(selection) = &selection_label {
            label.push(' ');
            label.push_str(selection);
        }
        let keyboard = states.clone();
        let key = id.clone();
        let keyboard_invalidate = invalidate.clone();
        let mut navigation = content
            .build()
            .with_key(format!("{id}-navigation"))
            .role(Role::Group)
            .focusable(true)
            .tab_index(0)
            .aria_label(label)
            .on_input(InputKind::KeyDown, EventPhase::Bubble, move |event| {
                let RoutedInput::Native(InputEvent::KeyDown(key_event)) = &event.input else {
                    return InputResponse::default();
                };
                let Key::Named(code) = key_event.key else {
                    return InputResponse::default();
                };
                if !keyboard.key(
                    &key,
                    rows,
                    columns,
                    code,
                    key_event.modifiers.contains(Modifiers::ALT),
                ) {
                    return InputResponse::default();
                }
                keyboard_invalidate();
                InputResponse {
                    stop_propagation: true,
                    ..InputResponse::default()
                }
            });
        if let Some(label) = selection_label {
            navigation = navigation.aria_value(label);
        }
        let navigation = navigation_frame(navigation, theme.colors.ring.to_reference_paint());
        let (count, low, high) = match self.scale {
            Scale::Sequential => (7, "fewer".to_owned(), "more".to_owned()),
            Scale::Diverging => (
                9,
                reading(-self.reach, self.unit.as_deref()),
                reading(self.reach, self.unit.as_deref()),
            ),
        };
        let mut stops = ui::Box::new().layout("row").style(Style::new().gap(2.0));
        for step in 0..count {
            let t = f32_of(f64_of(step) / f64_of(count - 1));
            let color = match self.scale {
                Scale::Sequential => sequential_color(&theme, t),
                Scale::Diverging => diverging_color(&theme, t * 2.0 - 1.0),
            };
            stops = stops.child(
                ui::Box::new()
                    .style(
                        Style::new()
                            .width(Length::Px(10.0))
                            .height(Length::Px(10.0))
                            .gpui_background(GpuiBackground::Solid(color.to_reference_paint())),
                    )
                    .radius(RadiusTokens::value(RadiusStep::X2s))
                    .build(),
            );
        }
        let ramp = ui::Box::new()
            .layout("row")
            .style(
                Style::new()
                    .gap(8.0)
                    .align(aqua_ui::AlignItems::CENTER)
                    .padding_insets(Insets {
                        left: 96.0,
                        ..Insets::default()
                    }),
            )
            .child(text(
                TextRole::Ramp,
                &low,
                12.0,
                16.0,
                FontWeight::MEDIUM,
                theme.colors.text_muted,
            ))
            .child(stops.build())
            .child(text(
                TextRole::Ramp,
                &high,
                12.0,
                16.0,
                FontWeight::MEDIUM,
                theme.colors.text_muted,
            ))
            .build();
        ui::Box::new()
            .layout("column")
            .style(Style::new().width(Length::Pct(100.0)).gap(10.0))
            .child(navigation)
            .child(ramp)
            .build()
    }
}

fn row_canvas(
    model: &Heatmap,
    theme: &ArtisanTheme,
    row: usize,
    selected: Option<(usize, usize)>,
    probe: Probe,
) -> Element {
    let columns = f64_of(model.columns());
    let mut shapes = Vec::new();
    if let Some(cells) = model.values.get(row) {
        for column in 0..cells.len() {
            let Some(value) = model.cell(row, column) else {
                continue;
            };
            let x0 = f32_of(f64_of(column) / columns);
            let x1 = f32_of(f64_of(column + 1) / columns);
            let inset = (x1 - x0) * 0.05;
            let t = model.position(value);
            let color = match model.scale {
                Scale::Sequential => sequential_color(theme, t),
                Scale::Diverging => diverging_color(theme, t),
            };
            shapes.push(Shape::Rect {
                x0: x0 + inset,
                x1: x1 - inset,
                y0: 0.05,
                y1: 0.95,
                color: color.to_reference_paint(),
            });
        }
    }
    if let Some((_, column)) = selected.filter(|(selected_row, _)| *selected_row == row) {
        let x0 = f32_of(f64_of(column) / columns);
        let x1 = f32_of(f64_of(column + 1) / columns);
        shapes.push(Shape::Line {
            points: vec![(x0, 0.0), (x1, 0.0), (x1, 1.0), (x0, 1.0), (x0, 0.0)],
            width: 2.0,
            color: theme.colors.text.to_reference_paint(),
            dash: false,
        });
    }
    let shapes: Rc<[Shape]> = shapes.into();
    ui::Canvas::new()
        .style(
            Style::new()
                .absolute_at(Some(0.0), Some(0.0), None, None)
                .width(Length::Pct(100.0))
                .height(Length::Pct(100.0)),
        )
        .on_paint(move |bounds| {
            probe.set(Some(bounds));
            scene::source_scene(bounds, &shapes)
        })
        .build()
}

#[cfg(feature = "test-support")]
pub struct HeatmapFixture {
    model: Heatmap,
    state: HeatmapState,
}

#[cfg(feature = "test-support")]
impl HeatmapFixture {
    pub fn new(json: &str) -> Self {
        let model = serde_json::from_str(json).expect("valid heatmap");
        let state = HeatmapState::default();
        state.sync("test", &model);
        Self { model, state }
    }
    pub fn replace(&mut self, json: &str) {
        self.model = serde_json::from_str(json).expect("valid heatmap");
        self.state.sync("test", &self.model);
    }
    pub fn selected(&self) -> Option<(usize, usize)> {
        self.state.sync("test", &self.model).0
    }
    pub fn hover(&self, row: usize, x: f32, y: f32) -> bool {
        self.state.hover("test", row, self.model.columns(), x, y)
    }
    pub fn key(&self, code: NamedKey, alt: bool) -> bool {
        self.state
            .key("test", self.model.rows(), self.model.columns(), code, alt)
    }
    pub fn pending_reveal(&self) -> Option<(String, usize)> {
        self.state.pending_reveal("test")
    }
    pub fn revealed(&self, key: &str, index: usize) {
        self.state.revealed("test", key, index);
    }
    pub fn generation(&self) -> u64 {
        self.state.sync("test", &self.model).2
    }
}
