use super::Dispatch;
use crate::conversation::adapter::Action;
use crate::conversation::navigator::tick_indices;
use aqua_core::Role;
use aqua_ui::element::{Box as UiBox, Element};
use aqua_ui::style::{Length, Style};
use aqua_ui::{AlignItems, JustifyContent};
use artisan_api::{ItemId, OutlineEntry, Seq};
use editor_ui::kit::{HitArea, Row, RowItem, Space, Spaced, Surface};
use editor_ui::motion::{MotionCurve, MotionDuration};
use editor_ui::theme::ArtisanTheme;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Instant;

const MENU: Surface = Surface::Menu;
const MENU_ROW: Row = Row::MenuItem;
const MENU_WIDTH: f32 = 320.0;
const ENTRANCE_SHIFT: Space = Space::S4;
const RAIL_PAD: Space = Space::S8;
const RAIL_WIDTH: f32 = 40.0;
const TICK_PITCH: f32 = 5.0;
const TICK_REST: f32 = 16.0;
const TICK_GROWTH: f32 = 8.0;
const TICK_LINE: f32 = 1.0;
const RAIL_MIN: f32 = 32.0;
const RAIL_SHARE: f32 = 0.7;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TickFrame {
    pub changed: bool,
    pub moving: bool,
}

pub fn tick_presence(current: bool, changed: Option<Instant>, now: Instant) -> (f32, bool) {
    let toward = changed.map_or(1.0, |changed| {
        let elapsed = now.saturating_duration_since(changed).as_secs_f32();
        MotionCurve::SmoothOut.sample(f64::from(
            (elapsed / MotionDuration::Fast.as_duration().as_secs_f32()).clamp(0.0, 1.0),
        )) as f32
    });
    let presence = if current { toward } else { 1.0 - toward };
    (presence, toward < 1.0)
}

fn tick_width(presence: f32) -> f32 {
    TICK_GROWTH.mul_add(presence, TICK_REST)
}

#[expect(
    clippy::disallowed_types,
    reason = "the rail tick is a painted data mark whose colour advance_ticks animates by writing its paint directly"
)]
fn tick_color(theme: &ArtisanTheme, presence: f32) -> aqua_core::GpuiBackground {
    aqua_core::GpuiBackground::Solid(
        theme
            .colors
            .gray
            .step(7)
            .animate_to(theme.colors.text, presence)
            .to_reference_paint(),
    )
}

#[expect(
    clippy::disallowed_methods,
    reason = "the rail tick is a painted data mark; advance_ticks animates this paint in place"
)]
fn tick(theme: &ArtisanTheme, presence: f32) -> Element {
    UiBox::new()
        .width(tick_width(presence))
        .height(TICK_LINE)
        .style(Style::new().gpui_background(tick_color(theme, presence)))
        .build()
}

struct BuiltRail {
    count: usize,
    slots: usize,
    indices: Vec<usize>,
    ids: Vec<ItemId>,
}

pub struct Navigator {
    built: Option<BuiltRail>,
    had_active: bool,
    ticks: HashMap<ItemId, (bool, Option<Instant>)>,
}

impl Default for Navigator {
    fn default() -> Self {
        Self {
            built: None,
            had_active: false,
            ticks: HashMap::new(),
        }
    }
}

impl Navigator {
    pub fn advance_ticks(
        &mut self,
        app: &mut aqua::UiApp,
        theme: &ArtisanTheme,
        now: Instant,
    ) -> TickFrame {
        let mut frame = TickFrame::default();
        for (id, (current, changed_at)) in &mut self.ticks {
            let Some(started) = *changed_at else {
                continue;
            };
            let (presence, in_flight) = tick_presence(*current, Some(started), now);
            frame.moving |= in_flight;
            if !in_flight {
                *changed_at = None;
            }
            let key = aqua_ui::tree::hash_key(aqua_ui::element::Key::from(format!(
                "navigator-tick-{id}"
            )));
            let Some(tick) = app
                .tree()
                .sole_node_with_key(key)
                .and_then(|button| app.tree().get(button)?.children.first().copied())
            else {
                continue;
            };
            let Some(mut layout) = app.tree().get(tick).map(|record| record.style.clone()) else {
                continue;
            };
            layout.width = Length::Px(tick_width(presence));
            if app.tree().get(tick).is_some_and(|record| record.style != layout) {
                frame.changed |= app.tree_mut().set_style(tick, layout);
            }
            if let Some(mut paint) = app.tree().paint(tick) {
                paint.gpui_background = Some(tick_color(theme, presence));
                frame.changed |= app.tree_mut().set_paint(tick, paint);
            }
        }
        frame
    }

    pub fn shift_active(&mut self, active: usize, now: Instant, reduced_motion: bool) -> bool {
        let Some(built) = &self.built else {
            return false;
        };
        if active >= built.count
            || tick_indices(built.count, built.slots, Some(active)) != built.indices
        {
            return false;
        }
        let started = if reduced_motion {
            now.checked_sub(MotionDuration::Fast.as_duration() * 2)
                .unwrap_or(now)
        } else {
            now
        };
        for (index, id) in built.indices.iter().zip(&built.ids) {
            let current = *index == active;
            if let Some(entry) = self.ticks.get_mut(id)
                && entry.0 != current
            {
                *entry = (current, Some(started));
            }
        }
        true
    }

    pub fn rail(
        &mut self,
        markers: &[OutlineEntry],
        reading: Option<Seq>,
        viewport_height: f32,
        composer_height: Option<f32>,
        composer_bottom: f32,
        reduced_motion: bool,
        now: Instant,
        theme: ArtisanTheme,
        dispatch: Dispatch,
    ) -> Option<(Element, f32, bool)> {
        if markers.len() < 2 {
            return None;
        }
        let active = markers
            .iter()
            .rposition(|marker| reading.is_some_and(|reading| marker.position <= reading))
            .unwrap_or(0);
        let pad = RAIL_PAD.px();
        let above = composer_height.map_or(0.0, |height| height + composer_bottom) + pad;
        let room = (viewport_height - above - pad).max(RAIL_MIN);
        let max_height = room.min(viewport_height * RAIL_SHARE);
        let height = (markers.len() as f32).mul_add(TICK_PITCH, 2.0 * pad).min(max_height);
        let slots = ((height - 2.0 * pad) / TICK_PITCH).floor() as usize;
        let indices = tick_indices(markers.len(), slots, Some(active));
        self.built = Some(BuiltRail {
            count: markers.len(),
            slots,
            indices: indices.clone(),
            ids: indices.iter().map(|index| markers[*index].id.clone()).collect(),
        });
        let retained: HashSet<_> = indices
            .iter()
            .map(|index| markers[*index].id.clone())
            .collect();
        self.ticks.retain(|id, _| retained.contains(id));
        let animate = !reduced_motion && self.had_active;
        self.had_active = true;
        let mut moving = false;
        let mut ticks = UiBox::new()
            .layout("column")
            .width(RAIL_WIDTH)
            .height(height)
            .clip(true)
            .padded(RAIL_PAD)
            .style(Style::new().align(AlignItems::START));
        for index in indices {
            let marker = &markers[index];
            let current = index == active;
            let entry = self
                .ticks
                .entry(marker.id.clone())
                .or_insert((current, None));
            if entry.0 != current {
                *entry = (current, animate.then_some(now));
            }
            let (presence, in_flight) = tick_presence(current, entry.1, now);
            moving |= in_flight;
            let position = marker.position;
            let callback = Rc::clone(&dispatch);
            let button = HitArea::new(
                format!("navigator-tick-{}", marker.id),
                Role::Button,
                marker.label.clone(),
            )
            .child(tick(&theme, presence))
            .on_activate(move || callback(Action::Jump(position)))
            .build()
            .style(
                Style::new()
                    .height(Length::Px(TICK_PITCH))
                    .flex_shrink(0.0)
                    .align(AlignItems::CENTER)
                    .justify(JustifyContent::START),
            );
            ticks = ticks.child(button);
        }
        Some((ticks.build(), max_height, moving))
    }
}

pub fn menu(
    markers: &[OutlineEntry],
    active: Option<Seq>,
    max_height: f32,
    theme: ArtisanTheme,
    entrance: f32,
    label: Rc<dyn Fn(&str, bool, ArtisanTheme) -> Element>,
    dispatch: Dispatch,
) -> Element {
    use aqua_core::Transform2D;
    use aqua_ui::collection::List;

    let pitch = MENU_ROW.height().unwrap_or_default();
    let height = ((markers.len() as f32) * pitch)
        .min((max_height - 2.0 * MENU.padding_px()).max(pitch));
    let list = List::new()
        .items(markers.iter().cloned())
        .key(|marker: &OutlineEntry| format!("navigator-row-{}", marker.id))
        .row(move |marker: &OutlineEntry| {
            let selected = active == Some(marker.position);
            let position = marker.position;
            let callback = Rc::clone(&dispatch);
            RowItem::new(
                format!("navigator-row-hit-{}", marker.id),
                theme,
                MENU_ROW,
                MENU,
                marker.label.clone(),
            )
            .body(label(&marker.label, selected, theme))
            .on_activate(move || callback(Action::Jump(position)))
            .build()
        })
        .virtualize()
        .estimated_row_height(Length::Px(pitch))
        .style(
            Style::new()
                .height(Length::Px(height))
                .width(Length::Pct(100.0)),
        )
        .build()
        .with_key("turn-navigator-list");
    let progress = entrance.clamp(0.0, 1.0);
    MENU.build(&theme, vec![list])
        .style(
            Style::new()
                .width(Length::Px(MENU_WIDTH))
                .min_width(Length::Pct(0.0))
                .opacity(progress)
                .transform(Transform2D::translate(
                    ENTRANCE_SHIFT.px() * (1.0 - progress),
                    0.0,
                )),
        )
        .with_key("turn-navigator-hover-surface")
}

pub fn menu_scroll_offset(active: usize, count: usize, viewport_height: f32) -> f32 {
    let pitch = MENU_ROW.height().unwrap_or_default();
    let full = count as f32 * pitch;
    ((active as f32 + 0.5) * pitch - viewport_height / 2.0)
        .clamp(0.0, (full - viewport_height).max(0.0))
}
