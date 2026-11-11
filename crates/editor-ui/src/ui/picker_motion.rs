use std::time::Duration;

use super::hover::HoverTone;

pub const DROPDOWN_OPEN: Duration = Duration::from_millis(250);
pub const DROPDOWN_CLOSE: Duration = Duration::from_millis(150);
pub const HOVER_FLIGHT: Duration = Duration::from_millis(250);
pub const DROPDOWN_PRE_SCALE: f32 = 0.97;
pub const DROPDOWN_CLOSING_SCALE: f32 = 0.99;
const HIDDEN_OFFSET: f32 = 8.0;
#[must_use]
pub fn smooth_out(progress: f32) -> f32 {
    let value = super::motion::MotionCurve::SmoothOut.sample(f64::from(progress)) as f32;
    value
}
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HoverRect {
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

impl HoverRect {
    #[must_use]
    pub fn lerp(self, target: Self, progress: f32) -> Self {
        let progress = progress.clamp(0.0, 1.0);
        Self {
            left: (target.left - self.left).mul_add(progress, self.left),
            top: (target.top - self.top).mul_add(progress, self.top),
            width: (target.width - self.width).mul_add(progress, self.width),
            height: (target.height - self.height).mul_add(progress, self.height),
        }
    }
    #[must_use]
    pub fn within(bounds: aqua_core::Rect, surface: aqua_core::Rect) -> Self {
        Self {
            left: bounds.x - surface.x,
            top: bounds.y - surface.y,
            width: bounds.w,
            height: bounds.h,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HoverTransition {
    pub from: HoverRect,
    pub to: HoverRect,
    pub from_tone: HoverTone,
    pub to_tone: HoverTone,
    pub generation: u64,
}
#[derive(Clone, Debug, Default)]
pub struct SlidingHoverState {
    active_id: Option<String>,
    active_tone: HoverTone,
    target_id: Option<String>,
    from_tone: HoverTone,
    tone: HoverTone,
    current: HoverRect,
    from: HoverRect,
    target: HoverRect,
    visible: bool,
    has_geometry: bool,
    animating: bool,
    generation: u64,
}

impl SlidingHoverState {
    #[must_use]
    pub fn active_id(&self) -> Option<&str> {
        self.active_id.as_deref()
    }

    #[must_use]
    pub const fn visual_rect(&self) -> HoverRect {
        self.current
    }

    #[must_use]
    pub const fn visible(&self) -> bool {
        self.visible
    }
    #[must_use]
    pub const fn tone(&self) -> HoverTone {
        self.tone
    }
    pub fn set_active(&mut self, id: String) {
        self.set_active_toned(id, HoverTone::Neutral);
    }
    pub fn set_active_toned(&mut self, id: String, tone: HoverTone) {
        if self.active_id.as_deref() != Some(id.as_str()) {
            self.active_id = Some(id);
        }
        self.active_tone = tone;
        self.visible = true;
    }
    pub fn clear(&mut self) {
        self.active_id = None;
        self.target_id = None;
        self.visible = false;
        self.has_geometry = false;
        self.animating = false;
        self.generation = self.generation.wrapping_add(1);
    }
    pub fn hide(&mut self) {
        self.active_id = None;
        self.visible = false;
    }
    pub fn clear_if_missing(&mut self, visible_ids: &[String]) {
        if self
            .active_id
            .as_ref()
            .is_some_and(|active| !visible_ids.iter().any(|id| id == active))
        {
            self.clear();
        }
    }
    pub fn measure(&mut self, id: &str, rect: HoverRect) -> bool {
        if self.active_id.as_deref() != Some(id) {
            return false;
        }
        if self.target_id.as_deref() == Some(id) && self.target == rect && self.visible {
            return false;
        }
        let switch_target = self.target_id.as_deref() != Some(id);
        let animate = self.visible && self.has_geometry && switch_target;
        self.target_id = Some(id.to_owned());
        self.visible = true;
        self.has_geometry = true;
        self.from = self.current;
        self.target = rect;
        self.from_tone = self.tone;
        self.tone = self.active_tone;
        self.generation = self.generation.wrapping_add(1);
        if animate {
            self.animating = true;
        } else {
            self.current = rect;
            self.from_tone = self.tone;
            self.animating = false;
        }
        true
    }

    #[must_use]
    pub fn transition(&self) -> Option<HoverTransition> {
        self.animating.then_some(HoverTransition {
            from: self.from,
            to: self.target,
            from_tone: self.from_tone,
            to_tone: self.tone,
            generation: self.generation,
        })
    }
    pub fn apply_progress(&mut self, generation: u64, progress: f32) {
        if !self.animating || generation != self.generation {
            return;
        }
        self.current = self.from.lerp(self.target, progress);
        if progress >= 1.0 {
            self.current = self.target;
            self.from_tone = self.tone;
            self.animating = false;
        }
    }
}

pub struct HoverSurface {
    state: std::rc::Rc<std::cell::RefCell<SlidingHoverState>>,
    pill_key: String,
    flight: Option<(u64, aqua_ui::motion::MotionHandle)>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HoverFrame {
    pub changed: bool,
    pub animating: bool,
}

impl HoverSurface {
    pub fn new(pill_key: impl Into<String>) -> Self {
        Self {
            state: std::rc::Rc::default(),
            pill_key: pill_key.into(),
            flight: None,
        }
    }

    pub fn hovered(&self, id: &str, hovered: bool, tone: HoverTone) {
        let mut state = self.state.borrow_mut();
        if hovered {
            state.set_active_toned(id.to_owned(), tone);
        } else if state.active_id() == Some(id) {
            state.hide();
        }
    }

    pub fn clear(&self) {
        self.state.borrow_mut().clear();
    }

    pub fn hide(&self) {
        self.state.borrow_mut().hide();
    }

    pub fn dispose(&mut self, app: &mut aqua::UiApp) {
        if let Some((_, handle)) = self.flight.take() {
            app.host_mut().motion_mut().remove(handle);
        }
        self.state.borrow_mut().clear();
    }

    pub fn is_animating(&self) -> bool {
        self.flight.is_some()
    }

    pub fn pill(
        &self,
        theme: &super::theme::ArtisanTheme,
        radius: f32,
        edged: bool,
    ) -> aqua_ui::element::Element {
        let fill = if edged {
            super::hover::HoverFill::Edged
        } else {
            super::hover::HoverFill::Plain
        };
        aqua_ui::element::Box::new()
            .style(super::hover::HoverColors::new(theme, HoverTone::Neutral).style(fill))
            .style(
                aqua_ui::style::Style::new()
                    .absolute_at(Some(0.0), Some(0.0), None, None)
                    .width(aqua_ui::style::Length::Px(0.0))
                    .height(aqua_ui::style::Length::Px(0.0))
                    .radius(aqua_ui::style::Length::Px(radius))
                    .opacity(0.0),
            )
            .build()
            .with_key(self.pill_key.clone())
    }

    pub fn advance(
        &mut self,
        app: &mut aqua::UiApp,
        surface: aqua_core::NodeId,
        theme: &super::theme::ArtisanTheme,
        edged: bool,
        now: f64,
        reduced_motion: bool,
    ) -> Result<bool, String> {
        let frame = self.advance_frame(app, surface, theme, edged, now, reduced_motion)?;
        Ok(frame.changed || frame.animating)
    }

    pub fn advance_frame(
        &mut self,
        app: &mut aqua::UiApp,
        surface: aqua_core::NodeId,
        theme: &super::theme::ArtisanTheme,
        edged: bool,
        now: f64,
        reduced_motion: bool,
    ) -> Result<HoverFrame, String> {
        use aqua_ui::{
            element::Key,
            motion::{Easing, MotionKind, MotionValue},
            style::{Absolute, Length},
            tree::hash_key,
        };
        let active = self.state.borrow().active_id().map(str::to_owned);
        let row_key = active.as_ref().map(|id| hash_key(Key::from(id.clone())));
        let pill_key = hash_key(Key::from(self.pill_key.clone()));
        let mut row = None;
        let mut pill = None;
        let mut stack = vec![surface];
        while let Some(node) = stack.pop() {
            if let Some(record) = app.tree().get(node) {
                stack.extend(record.children.iter().copied());
                if record.key == row_key && row_key.is_some() {
                    row = Some(node);
                }
                if record.key == Some(pill_key) {
                    pill = Some(node);
                }
            }
        }
        let Some(pill) = pill else {
            return Ok(HoverFrame::default());
        };
        if let (Some(id), Some(row), Some(surface_rect)) =
            (active, row, app.tree().visual_rect(surface))
        {
            if let Some(row_rect) = app.tree().visual_rect(row) {
                let transform = app
                    .tree()
                    .visual_transform(surface)
                    .unwrap_or(aqua_core::Transform2D::IDENTITY);
                let sx = transform.a.hypot(transform.b);
                let sy = transform.c.hypot(transform.d);
                if sx > 0.0 && sy > 0.0 {
                    self.state.borrow_mut().measure(
                        &id,
                        HoverRect {
                            left: (row_rect.x - surface_rect.x) / sx,
                            top: (row_rect.y - surface_rect.y) / sy,
                            width: row_rect.w / sx,
                            height: row_rect.h / sy,
                        },
                    );
                }
            }
        }
        let transition = self.state.borrow().transition();
        let mut progress = 1.0;
        if let Some(transition) = transition {
            if reduced_motion {
                if let Some((_, old)) = self.flight.take() {
                    app.host_mut().motion_mut().remove(old);
                }
                self.state
                    .borrow_mut()
                    .apply_progress(transition.generation, 1.0);
            } else {
                if self
                    .flight
                    .as_ref()
                    .is_none_or(|(generation, _)| *generation != transition.generation)
                {
                    if let Some((_, old)) = self.flight.take() {
                        app.host_mut().motion_mut().remove(old);
                    }
                    let controller = app.host_mut().motion_mut();
                    let handle = controller.spawn(
                        Some(surface),
                        MotionValue::Scalar(0.0),
                        MotionKind::Tween {
                            duration_secs: HOVER_FLIGHT.as_secs_f64(),
                            easing: Easing::Linear,
                        },
                        now,
                    );
                    controller
                        .set_target(handle, MotionValue::Scalar(1.0), now)
                        .map_err(|error| format!("{error:?}"))?;
                    self.flight = Some((transition.generation, handle));
                }
                if let Some((_, handle)) = self.flight {
                    if let Some(MotionValue::Scalar(value)) =
                        app.host_mut().motion_mut().sample(handle, now)
                    {
                        progress = smooth_out(value);
                        self.state
                            .borrow_mut()
                            .apply_progress(transition.generation, progress);
                    }
                    if app.host().motion().is_settled(handle) {
                        app.host_mut().motion_mut().remove(handle);
                        self.flight = None;
                    }
                }
            }
        } else if let Some((_, old)) = self.flight.take() {
            app.host_mut().motion_mut().remove(old);
        }
        let state = self.state.borrow();
        let rect = state.visual_rect();
        let mut colors = super::hover::HoverColors::new(theme, state.tone());
        if let Some(transition) = transition {
            colors = super::hover::HoverColors::new(theme, transition.from_tone).animate_to(
                super::hover::HoverColors::new(theme, transition.to_tone),
                progress,
            );
        }
        let mut layout = app
            .tree()
            .get(pill)
            .ok_or_else(|| "hover pill is missing".to_owned())?
            .style
            .clone();
        let mut paint = app
            .tree()
            .paint(pill)
            .ok_or_else(|| "hover pill is missing".to_owned())?;
        layout.absolute = Some(Absolute {
            left: Some(0.0),
            top: Some(0.0),
            right: None,
            bottom: None,
        });
        layout.width = Length::Px(rect.width);
        layout.height = Length::Px(rect.height);
        let fill = if edged {
            super::hover::HoverFill::Edged
        } else {
            super::hover::HoverFill::Plain
        };
        colors
            .style(fill)
            .opacity(if state.visible() { 1.0 } else { 0.0 })
            .apply_to(&mut layout, &mut paint);
        paint.transform = Some(aqua_core::Transform2D::translate(rect.left, rect.top));
        drop(state);
        let layout_changed = app
            .tree()
            .get(pill)
            .is_some_and(|record| record.style != layout);
        if layout_changed {
            app.tree_mut().set_style(pill, layout);
        }
        let paint_changed = app.tree_mut().set_paint(pill, paint);
        Ok(HoverFrame {
            changed: layout_changed || paint_changed,
            animating: self.flight.is_some(),
        })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MenuPhase {
    #[default]
    Hidden,
    Opening,
    Open,
    Closing,
}
#[derive(Clone, Copy, Debug)]
pub struct MenuMotion {
    phase: MenuPhase,
    current_opacity: f32,
    current_offset: f32,
    from_opacity: f32,
    from_offset: f32,
    to_opacity: f32,
    to_offset: f32,
    generation: u64,
}

impl Default for MenuMotion {
    fn default() -> Self {
        Self {
            phase: MenuPhase::Hidden,
            current_opacity: 0.0,
            current_offset: HIDDEN_OFFSET,
            from_opacity: 0.0,
            from_offset: HIDDEN_OFFSET,
            to_opacity: 0.0,
            to_offset: HIDDEN_OFFSET,
            generation: 0,
        }
    }
}

impl MenuMotion {
    pub const fn frame(self) -> (f32, f32) {
        (self.current_opacity, self.current_offset)
    }
    #[must_use]
    pub const fn phase(self) -> MenuPhase {
        self.phase
    }
    #[must_use]
    pub fn painted(self) -> bool {
        self.phase != MenuPhase::Hidden
    }
    #[must_use]
    pub const fn transition(self) -> Option<(f32, f32, f32, f32, u64)> {
        match self.phase {
            MenuPhase::Opening | MenuPhase::Closing => Some((
                self.from_opacity,
                self.from_offset,
                self.to_opacity,
                self.to_offset,
                self.generation,
            )),
            MenuPhase::Hidden | MenuPhase::Open => None,
        }
    }
    pub const fn begin_open(&mut self) -> u64 {
        self.from_opacity = self.current_opacity;
        self.from_offset = self.current_offset;
        self.to_opacity = 1.0;
        self.to_offset = 0.0;
        self.phase = MenuPhase::Opening;
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }
    pub const fn begin_close(&mut self) -> u64 {
        self.from_opacity = self.current_opacity;
        self.from_offset = self.current_offset;
        self.to_opacity = 0.0;
        self.to_offset = HIDDEN_OFFSET;
        self.phase = MenuPhase::Closing;
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }

    pub fn apply_progress(&mut self, generation: u64, progress: f32) {
        if generation != self.generation {
            return;
        }
        let progress = progress.clamp(0.0, 1.0);
        self.current_opacity =
            (self.to_opacity - self.from_opacity).mul_add(progress, self.from_opacity);
        self.current_offset =
            (self.to_offset - self.from_offset).mul_add(progress, self.from_offset);
    }
    pub fn finish_open(&mut self, generation: u64) -> bool {
        if self.phase != MenuPhase::Opening || generation != self.generation {
            return false;
        }
        self.current_opacity = 1.0;
        self.current_offset = 0.0;
        self.phase = MenuPhase::Open;
        true
    }
    pub fn finish_close(&mut self, generation: u64) -> bool {
        if self.phase != MenuPhase::Closing || generation != self.generation {
            return false;
        }
        self.current_opacity = 0.0;
        self.current_offset = HIDDEN_OFFSET;
        self.phase = MenuPhase::Hidden;
        true
    }
    pub fn hide(&mut self) {
        *self = Self {
            generation: self.generation.wrapping_add(1),
            ..Self::default()
        };
    }
}
