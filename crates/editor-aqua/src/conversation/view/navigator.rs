use super::Dispatch;
use crate::conversation::adapter::Action;
use crate::conversation::navigator::tick_indices;
use crate::ui::motion::{MotionCurve, MotionDuration};
use crate::ui::theme::ArtisanTheme;
use aqua_core::{CursorIcon, Insets};
use aqua_ui::element::{Box as UiBox, Button, Element};
use aqua_ui::style::{Length, Style};
use aqua_ui::{AlignItems, JustifyContent};
use artisan_api::{ItemId, OutlineEntry, Seq};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Instant;

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
    8.0_f32.mul_add(presence, 16.0)
}

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
    pub hover: crate::ui::picker_motion::HoverSurface,
}

impl Default for Navigator {
    fn default() -> Self {
        Self {
            built: None,
            had_active: false,
            ticks: HashMap::new(),
            hover: crate::ui::picker_motion::HoverSurface::new("turn-navigator-hover"),
        }
    }
}

impl Navigator {
    pub fn hovered(&self, id: &ItemId) {
        self.hover.hovered(
            &format!("navigator-row-hit-{id}"),
            true,
            crate::ui::hover::HoverTone::Neutral,
        );
    }

    pub fn clear_hover(&self) {
        self.hover.clear();
    }

    pub fn advance_hover(
        &mut self,
        app: &mut aqua::UiApp,
        surface: aqua_core::NodeId,
        theme: &ArtisanTheme,
        now: f64,
        reduced_motion: bool,
    ) -> Result<bool, String> {
        let frame = self.advance_hover_frame(app, surface, theme, now, reduced_motion)?;
        Ok(frame.changed || frame.animating)
    }

    pub fn advance_hover_frame(
        &mut self,
        app: &mut aqua::UiApp,
        surface: aqua_core::NodeId,
        theme: &ArtisanTheme,
        now: f64,
        reduced_motion: bool,
    ) -> Result<crate::ui::picker_motion::HoverFrame, String> {
        self.hover
            .advance_frame(app, surface, theme, false, now, reduced_motion)
    }

    pub fn dispose(&mut self, app: &mut aqua::UiApp) {
        self.hover.dispose(app);
    }

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
        let above = composer_height.map_or(0.0, |height| height + composer_bottom) + 8.0;
        let room = (viewport_height - above - 8.0).max(32.0);
        let max_height = room.min(viewport_height * 0.7);
        let height = ((markers.len() as f32) * 5.0 + 16.0).min(max_height);
        let indices = tick_indices(
            markers.len(),
            ((height - 16.0) / 5.0).floor() as usize,
            Some(active),
        );
        let slots = ((height - 16.0) / 5.0).floor() as usize;
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
            .width(40.0)
            .height(height)
            .clip(true)
            .style(Style::new().align(AlignItems::START).padding_insets(Insets {
                top: 8.0,
                right: 8.0,
                bottom: 8.0,
                left: 8.0,
            }));
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
            let tick = UiBox::new()
                .width(tick_width(presence))
                .height(1.0)
                .radius(0.5)
                .style(Style::new().gpui_background(tick_color(&theme, presence)))
                .build();
            let position = marker.position;
            let callback = Rc::clone(&dispatch);
            let button = Button::new()
                .width(Length::Pct(100.0))
                .height(5.0)
                .style(
                    Style::new()
                        .flex_shrink(0.0)
                        .align(AlignItems::CENTER)
                        .justify(JustifyContent::START),
                )
                .child(tick)
                .on_activate(move || callback(Action::Jump(position)))
                .build()
                .with_key(format!("navigator-tick-{}", marker.id))
                .cursor(CursorIcon::Pointer);
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
    hover_surface: &crate::ui::picker_motion::HoverSurface,
    label: Rc<dyn Fn(&str, bool, ArtisanTheme) -> Element>,
    hover: Rc<dyn Fn(ItemId)>,
    dispatch: Dispatch,
) -> Element {
    use crate::ui::glass::glass_navigator_card;
    use crate::ui::theme::{RadiusStep, RadiusTokens};
    use aqua_core::Transform2D;
    use aqua_ui::collection::List;
    use aqua_ui::input::{EventPhase, InputKind, InputResponse};

    let pitch = 36.0;
    let height = ((markers.len() as f32) * pitch).min((max_height - 12.0).max(pitch));
    let list = List::new()
        .items(markers.iter().cloned())
        .key(|marker: &OutlineEntry| format!("navigator-row-{}", marker.id))
        .row(move |marker: &OutlineEntry| {
            let selected = active == Some(marker.position);
            let text = label(&marker.label, selected, theme);
            let id = marker.id.clone();
            let on_move = Rc::clone(&hover);
            let position = marker.position;
            let callback = Rc::clone(&dispatch);
            let row = Button::new()
                .width(Length::Pct(100.0))
                .height(32.0)
                .radius(RadiusTokens::value(RadiusStep::Lg))
                .style(
                    Style::new()
                        .align(AlignItems::CENTER)
                        .padding_insets(Insets {
                            left: 12.0,
                            right: 12.0,
                            ..Insets::default()
                        }),
                )
                .child(text)
                .on_activate(move || callback(Action::Jump(position)))
                .build()
                .with_key(format!("navigator-row-hit-{}", marker.id))
                .cursor(CursorIcon::Pointer)
                .on_input(InputKind::PointerMove, EventPhase::Bubble, move |_| {
                    on_move(id.clone());
                    InputResponse::default()
                });
            UiBox::new()
                .width(Length::Pct(100.0))
                .height(pitch)
                .style(Style::new().padding_insets(Insets {
                    top: 2.0,
                    bottom: 2.0,
                    ..Insets::default()
                }))
                .child(row)
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
    let content = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .style(Style::new().min_width(Length::Px(0.0)).padding_insets(Insets {
            left: 8.0,
            right: 8.0,
            top: 6.0,
            bottom: 6.0,
        }))
        .child(hover_surface.pill(&theme, 14.0, false))
        .child(list)
        .build()
        .with_key("turn-navigator-hover-surface");
    let progress = entrance.clamp(0.0, 1.0);
    UiBox::new()
        .width(320.0)
        .layout("column")
        .opacity(progress)
        .style(Style::new().transform(Transform2D::translate(4.0 * (1.0 - progress), 0.0)))
        .child(glass_navigator_card(&theme, content))
        .build()
}

pub fn menu_scroll_offset(active: usize, count: usize, viewport_height: f32) -> f32 {
    let full = count as f32 * 36.0;
    ((active as f32 * 36.0 + 18.0) - viewport_height / 2.0)
        .clamp(0.0, (full - viewport_height).max(0.0))
}
