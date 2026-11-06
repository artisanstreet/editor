use super::Concentration;
use crate::ui::rich::charts::scale::{f32_of, f64_of, index_of, percent};
use crate::ui::rich::charts::scene::{self, Probe, Shape};
use crate::ui::theme::{ArtisanTheme, Opacity};
use aqua_core::{CursorIcon, GpuiHsla, InputEvent, Key, Modifiers, NamedKey, Role};
use aqua_ui::element::{Element, ui};
use aqua_ui::input::{EventPhase, InputKind, InputResponse, RoutedInput};
use aqua_ui::style::{Length, Style};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

pub(crate) type FrameFactory = Rc<dyn Fn(Element, &str, &str) -> Element>;
pub(crate) type PlotLabelFactory = Rc<dyn Fn(f32, f32, &str, GpuiHsla) -> Element>;
pub(crate) type ReadoutFactory = Rc<dyn Fn(f32, &str, &str, &str, GpuiHsla) -> Element>;
pub(crate) type EmptyFactory = Rc<dyn Fn(&str) -> Element>;
pub(crate) type NavigationFrame = Rc<dyn Fn(Element, GpuiHsla) -> Element>;

struct Entry {
    hover: Option<f64>,
    probe: Probe,
}
#[derive(Clone, Default)]
pub(crate) struct ConcentrationState(Rc<RefCell<HashMap<String, Entry>>>);
impl ConcentrationState {
    fn ensure(&self, id: &str) -> (Option<f64>, Probe) {
        let mut entries = self.0.borrow_mut();
        let entry = entries.entry(id.to_owned()).or_insert_with(|| Entry {
            hover: None,
            probe: Rc::new(Cell::new(None)),
        });
        (entry.hover, entry.probe.clone())
    }
    fn hover(&self, id: &str, x: f32, y: f32) -> bool {
        let selected =
            ((0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y)).then_some(f64::from(x));
        self.set(id, selected)
    }
    fn set(&self, id: &str, selected: Option<f64>) -> bool {
        let mut entries = self.0.borrow_mut();
        let Some(entry) = entries.get_mut(id) else {
            return false;
        };
        let changed = entry.hover != selected;
        entry.hover = selected;
        changed
    }
    fn key(&self, id: &str, code: NamedKey, alt: bool) -> bool {
        let mut entries = self.0.borrow_mut();
        let Some(entry) = entries.get_mut(id) else {
            return false;
        };
        let selected = entry.hover.map(|share| index_of((share * 100.0).round()));
        let next = match code {
            NamedKey::Escape | NamedKey::Backspace => None,
            NamedKey::Left if alt => None,
            NamedKey::Home => Some(0),
            NamedKey::End => Some(100),
            NamedKey::Left | NamedKey::Up => {
                Some(selected.map_or(0, |index| index.saturating_sub(1)))
            }
            NamedKey::Right | NamedKey::Down => {
                Some(selected.map_or(0, |index| index.saturating_add(1).min(100)))
            }
            NamedKey::Enter => Some(selected.unwrap_or(0)),
            _ => return false,
        };
        entry.hover = next.map(|index| f64_of(index) / 100.0);
        true
    }
    pub(crate) fn remove(&self, id: &str) {
        self.0.borrow_mut().remove(id);
    }
}

fn sample(curve: &[(f64, f64)], limit: usize) -> Vec<(f64, f64)> {
    if curve.len() <= limit || limit < 2 {
        return curve.to_vec();
    }
    let last = curve.len() - 1;
    (0..limit)
        .map(|step| curve[index_of(f64_of(step) * f64_of(last) / f64_of(limit - 1)).min(last)])
        .collect()
}
fn shapes(
    theme: &ArtisanTheme,
    curve: &[(f64, f64)],
    cutoff: f64,
    hover: Option<f64>,
) -> Vec<Shape> {
    let at = |(x, y): (f64, f64)| (f32_of(x), 1.0 - f32_of(y));
    let series = theme.colors.palette[0];
    let mut shapes = Vec::new();
    for y in [0.0, 0.25, 0.5, 0.75, 1.0] {
        shapes.push(Shape::Line {
            points: vec![(0.0, y), (1.0, y)],
            width: 1.0,
            color: theme.colors.text.at(Opacity::Rule).to_reference_paint(),
            dash: false,
        });
    }
    shapes.push(Shape::Line {
        points: vec![(0.0, 1.0), (1.0, 0.0)],
        width: 1.0,
        color: theme
            .colors
            .text_muted
            .at(Opacity::ReferenceLine)
            .to_reference_paint(),
        dash: true,
    });
    let points: Vec<_> = sample(curve, 400).into_iter().map(at).collect();
    let mut area = points.clone();
    area.extend([(1.0, 1.0), (0.0, 1.0)]);
    shapes.push(Shape::Area {
        points: area,
        color: series.at(Opacity::AreaQuiet).to_reference_paint(),
    });
    shapes.push(Shape::Line {
        points,
        width: 2.0,
        color: series.to_reference_paint(),
        dash: false,
    });
    let (x, y) = at((cutoff, Concentration::share_of_total(curve, cutoff)));
    let marker = theme
        .colors
        .text
        .at(Opacity::EventRule)
        .to_reference_paint();
    shapes.push(Shape::Line {
        points: vec![(x, 0.0), (x, 1.0)],
        width: 1.0,
        color: marker,
        dash: true,
    });
    shapes.push(Shape::Line {
        points: vec![(0.0, y), (1.0, y)],
        width: 1.0,
        color: marker,
        dash: true,
    });
    shapes.push(Shape::Dot {
        x,
        y,
        radius: 4.0,
        color: theme.colors.text.to_reference_paint(),
    });
    if let Some(share) = hover {
        let (x, y) = at((share, Concentration::share_of_total(curve, share)));
        shapes.push(Shape::Line {
            points: vec![(x, 0.0), (x, 1.0)],
            width: 1.0,
            color: theme
                .colors
                .text
                .at(Opacity::ChartRule)
                .to_reference_paint(),
            dash: false,
        });
        shapes.push(Shape::Dot {
            x,
            y,
            radius: 4.0,
            color: series.to_reference_paint(),
        });
    }
    shapes
}

impl Concentration {
    pub(crate) fn view(
        &self,
        id: String,
        theme: &ArtisanTheme,
        states: ConcentrationState,
        frame: FrameFactory,
        plot_label: PlotLabelFactory,
        readout: ReadoutFactory,
        empty: EmptyFactory,
        navigation_frame: NavigationFrame,
        invalidate: Rc<dyn Fn()>,
    ) -> Element {
        let Some(curve) = self.curve() else {
            return empty("No values to rank");
        };
        let (hover, probe) = states.ensure(&id);
        let cutoff = self.highlight();
        let held = Concentration::share_of_total(&curve, cutoff);
        let shape_list: Rc<[Shape]> = shapes(theme, &curve, cutoff, hover).into();
        let paint_probe = probe.clone();
        let canvas = ui::Canvas::new()
            .style(
                Style::new()
                    .absolute_at(Some(0.0), Some(0.0), None, None)
                    .width(Length::Pct(100.0))
                    .height(Length::Pct(100.0)),
            )
            .on_paint(move |bounds| {
                paint_probe.set(Some(bounds));
                scene::source_scene(bounds, &shape_list)
            })
            .build();
        let marker = plot_label(
            f32_of(cutoff),
            1.0 - f32_of(held),
            &format!(
                "Top {} of {} → {} of {}",
                percent(cutoff),
                self.items(),
                percent(held),
                self.total()
            ),
            theme.colors.text.to_reference_paint(),
        );
        let mut plot = ui::Box::new()
            .style(
                Style::new()
                    .absolute_at(Some(0.0), Some(0.0), None, None)
                    .width(Length::Pct(100.0))
                    .height(Length::Pct(100.0)),
            )
            .child(canvas)
            .child(marker);
        let mut selection_label = None;
        if let Some(share) = hover {
            let held = Concentration::share_of_total(&curve, share);
            let heading = format!("Top {} of {}", percent(share), self.items());
            selection_label = Some(format!("{heading}: {} of {}", percent(held), self.total()));
            plot = plot.child(readout(
                f32_of(share),
                &heading,
                &format!("Share of {}", self.total()),
                &percent(held),
                theme.colors.palette[0].to_reference_paint(),
            ));
        }
        let moving = states.clone();
        let move_id = id.clone();
        let move_probe = probe.clone();
        let move_invalidate = invalidate.clone();
        let leaving = states.clone();
        let leave_id = id.clone();
        let leave_invalidate = invalidate.clone();
        let plot = plot
            .build()
            .with_key(format!("{id}-plot"))
            .cursor(CursorIcon::Crosshair)
            .on_input(InputKind::PointerMove, EventPhase::Bubble, move |event| {
                let RoutedInput::Native(InputEvent::PointerMove(pointer)) = &event.input else {
                    return InputResponse::default();
                };
                let (x, y) =
                    scene::fractions(&move_probe, pointer.position).unwrap_or((f32::NAN, f32::NAN));
                if moving.hover(&move_id, x, y) {
                    move_invalidate();
                }
                InputResponse::default()
            })
            .on_hover_change(move |hovered| {
                if !hovered && leaving.set(&leave_id, None) {
                    leave_invalidate();
                }
            });
        let mut label =
            "Concentration chart. Arrows explore percentage shares; Escape clears the reading."
                .to_owned();
        if let Some(selection) = &selection_label {
            label.push(' ');
            label.push_str(selection);
        }
        let keyboard = states.clone();
        let key = id.clone();
        let mut nav = ui::Box::new()
            .style(
                Style::new()
                    .width(Length::Pct(100.0))
                    .height(Length::Pct(100.0)),
            )
            .child(plot)
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
                if !keyboard.key(&key, code, key_event.modifiers.contains(Modifiers::ALT)) {
                    return InputResponse::default();
                }
                invalidate();
                InputResponse {
                    stop_propagation: true,
                    ..InputResponse::default()
                }
            });
        if let Some(selection) = selection_label {
            nav = nav.aria_value(selection);
        }
        let nav = navigation_frame(nav, theme.colors.ring.to_reference_paint());
        frame(
            nav,
            &format!("Share of {}", self.total()),
            &format!("Share of {}, largest first", self.items()),
        )
    }
}

#[cfg(feature = "test-support")]
pub struct ConcentrationFixture {
    model: Concentration,
    state: ConcentrationState,
}
#[cfg(feature = "test-support")]
impl ConcentrationFixture {
    pub fn new(json: &str) -> Self {
        let model = serde_json::from_str(json).expect("valid concentration");
        let state = ConcentrationState::default();
        state.ensure("test");
        Self { model, state }
    }
    pub fn replace(&mut self, json: &str) {
        self.model = serde_json::from_str(json).expect("valid concentration");
        self.state.ensure("test");
    }
    pub fn hover(&self, x: f32, y: f32) -> bool {
        self.state.hover("test", x, y)
    }
    pub fn selected(&self) -> Option<f64> {
        self.state.ensure("test").0
    }
    pub fn key(&self, key: NamedKey, alt: bool) -> bool {
        self.state.key("test", key, alt)
    }
    pub fn held(&self) -> Option<f64> {
        Some(Concentration::share_of_total(
            &self.model.curve()?,
            self.selected()?,
        ))
    }
    pub fn sample_points(&self, limit: usize) -> Vec<(f64, f64)> {
        self.model
            .curve()
            .map_or_else(Vec::new, |curve| sample(&curve, limit))
    }
}
