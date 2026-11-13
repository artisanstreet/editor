use super::{Funnel, Step};
use crate::ui::kit::Space;
use crate::ui::rich::charts::scale::{f32_of_count, percent};
use crate::ui::rich::kit::format_number;
use crate::ui::theme::{ArtisanTheme, Oklch, Opacity, RadiusStep, RadiusTokens};
use aqua_core::{GpuiBackground, InputEvent, Insets, Key, Modifiers, NamedKey, Role};
use aqua_text::FontWeight;
use aqua_ui::collection::List;
use aqua_ui::element::{Element, ui};
use aqua_ui::input::{EventPhase, InputKind, InputResponse, RoutedInput};
use aqua_ui::style::{Length, Style};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TextRole {
    RowLabel,
    Value,
    Summary,
}
pub(crate) type TextFactory = Rc<dyn Fn(TextRole, &str, f32, f32, FontWeight, Oklch) -> Element>;
pub(crate) type EmptyFactory = Rc<dyn Fn(&str) -> Element>;
pub(crate) type NavigationFrame = Rc<dyn Fn(Element, aqua_core::GpuiHsla) -> Element>;

struct Entry {
    model: Arc<[Step]>,
    selected: Option<usize>,
    reveal: Option<usize>,
    reset: u64,
}

#[derive(Clone, Default)]
pub(crate) struct FunnelState(Rc<RefCell<HashMap<String, Entry>>>);
impl FunnelState {
    fn sync(&self, id: &str, model: &Funnel) -> (Option<usize>, u64) {
        let mut entries = self.0.borrow_mut();
        let entry = entries.entry(id.to_owned()).or_insert_with(|| Entry {
            model: model.steps.clone(),
            selected: None,
            reveal: None,
            reset: 0,
        });
        if !Arc::ptr_eq(&entry.model, &model.steps) {
            if *entry.model != *model.steps {
                entry.selected = None;
                entry.reveal = None;
                entry.reset = entry.reset.wrapping_add(1);
            }
            entry.model = model.steps.clone();
        }
        (entry.selected, entry.reset)
    }
    fn hover(&self, id: &str, index: usize, hovered: bool) -> bool {
        let mut entries = self.0.borrow_mut();
        let Some(entry) = entries.get_mut(id) else {
            return false;
        };
        let before = entry.selected;
        if hovered {
            entry.selected = Some(index);
        } else if entry.selected == Some(index) {
            entry.selected = None;
        }
        before != entry.selected
    }
    fn key(&self, id: &str, count: usize, code: NamedKey, alt: bool) -> bool {
        let mut entries = self.0.borrow_mut();
        let Some(entry) = entries.get_mut(id) else {
            return false;
        };
        if count == 0 {
            return false;
        }
        let next = match code {
            NamedKey::Escape | NamedKey::Backspace => None,
            NamedKey::Left if alt => None,
            NamedKey::Home => Some(0),
            NamedKey::End => Some(count - 1),
            NamedKey::Left | NamedKey::Up => {
                Some(entry.selected.map_or(0, |index| index.saturating_sub(1)))
            }
            NamedKey::Right | NamedKey::Down => Some(
                entry
                    .selected
                    .map_or(0, |index| index.saturating_add(1).min(count - 1)),
            ),
            NamedKey::Enter => Some(entry.selected.unwrap_or(0)),
            _ => return false,
        };
        entry.selected = next;
        entry.reveal = next;
        true
    }
    pub(crate) fn pending_reveal(&self, id: &str) -> Option<(String, usize)> {
        let entries = self.0.borrow();
        let entry = entries.get(id)?;
        Some((format!("{id}-rows-{}", entry.reset), entry.reveal?))
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

fn drop_text(drop: f64, share: Option<f64>) -> String {
    let amount = format_number(drop.abs());
    let sign = if drop >= 0.0 { "−" } else { "+" };
    share.map_or_else(
        || format!("{sign}{amount}"),
        |share| format!("{sign}{amount} ({sign}{})", percent(share.abs())),
    )
}

fn summary(funnel: &Funnel, selected: Option<usize>) -> String {
    let last = funnel.steps.len().saturating_sub(1);
    match selected {
        Some(index) => {
            let reading = funnel.reading(index);
            let mut parts = vec![format!(
                "{}: {}",
                funnel.label(index),
                format_number(reading.value)
            )];
            if let Some(share) = reading.of_first {
                parts.push(format!("{} of {}", percent(share), funnel.label(0)));
            }
            if let Some((drop, share)) = reading.drop {
                parts.push(format!(
                    "{} from {}",
                    drop_text(drop, share),
                    funnel.label(index - 1)
                ));
            }
            parts.join(" · ")
        }
        None => match funnel.reading(last).of_first {
            Some(share) if last > 0 => format!(
                "{} of {} reach {}",
                percent(share),
                funnel.label(0),
                funnel.label(last)
            ),
            _ => String::new(),
        },
    }
}

impl Funnel {
    pub(crate) fn view(
        &self,
        id: String,
        theme: &ArtisanTheme,
        states: FunnelState,
        text: TextFactory,
        empty: EmptyFactory,
        navigation_frame: NavigationFrame,
        invalidate: Rc<dyn Fn()>,
    ) -> Element {
        let count = self.steps.len();
        if count == 0 {
            return empty("No steps");
        }
        let (selected, reset) = states.sync(&id, self);
        let theme = *theme;
        let height = theme.density.control_sm;
        let model = self.clone();
        let row_state = states.clone();
        let row_id = id.clone();
        let row_text = text.clone();
        let row_invalidate = invalidate.clone();
        let rows = List::new()
            .items(0..count)
            .key(|index| *index)
            .virtualize()
            .estimated_row_height(Length::Px(height))
            .style(
                Style::new()
                    .width(Length::Pct(100.0))
                    .height(Length::Px(height * f32_of_count(count.min(10)))),
            )
            .row(move |index| {
                let index = *index;
                let reading = model.reading(index);
                let fill = theme.colors.palette[0]
                    .at(if selected.is_some_and(|selected| selected != index) {
                        Opacity::Dimmed
                    } else {
                        Opacity::BarSolo
                    })
                    .to_reference_paint();
                let state = row_state.clone();
                let key = row_id.clone();
                let invalidate = row_invalidate.clone();
                let name = ui::Box::new()
                    .style(
                        Style::new()
                            .width(Length::Pct(24.0))
                            .flex_shrink(0.0)
                            .min_width(Length::Px(0.0)),
                    )
                    .child(row_text(
                        TextRole::RowLabel,
                        &model.label(index),
                        13.0,
                        20.0,
                        FontWeight::NORMAL,
                        theme.colors.text_muted,
                    ))
                    .clip(true)
                    .build();
                let bar = ui::Box::new()
                    .style(
                        Style::new()
                            .width(Length::Pct(model.width(index) * 100.0))
                            .height(Length::Pct(100.0))
                            .gpui_background(GpuiBackground::Solid(fill)),
                    )
                    .radius(RadiusTokens::value(RadiusStep::Xs))
                    .build();
                let center = ui::Box::new()
                    .layout("row")
                    .style(
                        Style::new()
                            .flex_grow(1.0)
                            .min_width(Length::Px(0.0))
                            .height(Length::Px(height - 4.0))
                            .justify(aqua_ui::JustifyContent::CENTER),
                    )
                    .child(bar)
                    .build();
                let value = row_text(
                    TextRole::Value,
                    &format_number(reading.value),
                    13.0,
                    20.0,
                    FontWeight::MEDIUM,
                    theme.colors.text,
                );
                let share = row_text(
                    TextRole::Value,
                    &reading.of_first.map(percent).unwrap_or_default(),
                    13.0,
                    20.0,
                    FontWeight::NORMAL,
                    theme.colors.text_muted,
                );
                let right = ui::Box::new()
                    .layout("row")
                    .style(
                        Style::new()
                            .width(Length::Pct(28.0))
                            .flex_shrink(0.0)
                            .min_width(Length::Px(0.0))
                            .justify(aqua_ui::JustifyContent::END)
                            .gap(Space::S6.px()),
                    )
                    .clip(true)
                    .child(value)
                    .child(share)
                    .build();
                ui::Box::new()
                    .layout("row")
                    .style(
                        Style::new()
                            .width(Length::Pct(100.0))
                            .height(Length::Px(height))
                            .align(aqua_ui::AlignItems::CENTER)
                            .gap(Space::S12.px()),
                    )
                    .child(name)
                    .child(center)
                    .child(right)
                    .build()
                    .with_key(format!("{row_id}-step-{index}"))
                    .on_hover_change(move |hovered| {
                        if state.hover(&key, index, hovered) {
                            invalidate();
                        }
                    })
            })
            .build()
            .with_key(format!("{id}-rows-{reset}"));
        let reading = summary(self, selected);
        let selection_label = selected.map(|index| {
            format!(
                "{}: {}; {}",
                self.label(index),
                format_number(self.value(index)),
                reading
            )
        });
        let summary = ui::Box::new()
            .style(Style::new().padding_insets(Insets {
                top: 4.0,
                ..Insets::default()
            }))
            .child(text(
                TextRole::Summary,
                &reading,
                13.0,
                20.0,
                FontWeight::NORMAL,
                theme.colors.text_muted,
            ))
            .build();
        let content = ui::Box::new()
            .layout("column")
            .style(Style::new().width(Length::Pct(100.0)).gap(Space::S6.px()))
            .child(rows)
            .child(summary)
            .build();
        let mut label = "Funnel. Arrows explore steps; Escape clears the reading.".to_owned();
        if let Some(value) = &selection_label {
            label.push(' ');
            label.push_str(value);
        }
        let keyboard = states.clone();
        let key = id.clone();
        let mut navigation = ui::Box::new()
            .width(Length::Pct(100.0))
            .child(content)
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
                    count,
                    code,
                    key_event.modifiers.contains(Modifiers::ALT),
                ) {
                    return InputResponse::default();
                }
                invalidate();
                InputResponse {
                    stop_propagation: true,
                    ..InputResponse::default()
                }
            });
        if let Some(label) = selection_label {
            navigation = navigation.aria_value(label);
        }
        navigation_frame(navigation, theme.colors.ring.to_reference_paint())
    }
}

#[cfg(feature = "test-support")]
pub struct FunnelFixture {
    model: Funnel,
    state: FunnelState,
}
#[cfg(feature = "test-support")]
impl FunnelFixture {
    pub fn new(json: &str) -> Self {
        let model = serde_json::from_str(json).expect("valid funnel");
        let state = FunnelState::default();
        state.sync("test", &model);
        Self { model, state }
    }
    pub fn replace(&mut self, json: &str) {
        self.model = serde_json::from_str(json).expect("valid funnel");
        self.state.sync("test", &self.model);
    }
    pub fn selected(&self) -> Option<usize> {
        self.state.sync("test", &self.model).0
    }
    pub fn generation(&self) -> u64 {
        self.state.sync("test", &self.model).1
    }
    pub fn hover(&self, index: usize, hovered: bool) -> bool {
        self.state.hover("test", index, hovered)
    }
    pub fn key(&self, code: NamedKey, alt: bool) -> bool {
        self.state.key("test", self.model.steps.len(), code, alt)
    }
    pub fn summary(&self) -> String {
        summary(&self.model, self.selected())
    }
    pub fn pending_reveal(&self) -> Option<(String, usize)> {
        self.state.pending_reveal("test")
    }
    pub fn revealed(&self, key: &str, index: usize) {
        self.state.revealed("test", key, index);
    }
}
