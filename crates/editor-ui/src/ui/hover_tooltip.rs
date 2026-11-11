use std::rc::Rc;

use aqua_core::{NodeId, Rect, Size};
use aqua_ui::{
    element::Element,
    overlay::{Align, DismissPolicy, Placement, Side},
    popup::PopupOptions,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TooltipPlacement {
    Beside { max_width: f32 },
    Above { max_width: f32 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TooltipPlan {
    pub width: f32,
    pub fixed: bool,
    pub side: Side,
}

pub fn tooltip_plan(anchor: Rect, viewport: Size, placement: TooltipPlacement) -> TooltipPlan {
    let viewport_width = viewport.w.max(1.0);
    match placement {
        TooltipPlacement::Above { max_width } => TooltipPlan {
            width: max_width.min(viewport_width),
            fixed: false,
            side: Side::Above,
        },
        TooltipPlacement::Beside { max_width } => {
            let right_space = viewport_width - anchor.right() - 8.0;
            let left_space = anchor.x - 8.0;
            let right = right_space >= max_width || right_space >= left_space;
            let available = if right { right_space } else { left_space };
            TooltipPlan {
                width: max_width.min(available.max(1.0)),
                fixed: true,
                side: if right { Side::After } else { Side::Before },
            }
        }
    }
}

fn placement(plan: TooltipPlan) -> Placement {
    Placement {
        preferred: plan.side,
        align: Align::Center,
        gap: 8.0,
        margin: 0.0,
        flip: false,
        centered: false,
    }
}

pub struct TooltipHost {
    popup: super::popover::AnimatedPickerHost,
    started: std::time::Instant,
    owner: NodeId,
    placement: TooltipPlacement,
    plan: TooltipPlan,
    surface: Rc<dyn Fn(f32, bool) -> Element>,
}

impl TooltipHost {
    pub fn open(
        app: &mut aqua::UiApp,
        owner: NodeId,
        requested: TooltipPlacement,
        surface: impl Fn(f32, bool) -> Element + 'static,
    ) -> Result<Self, String> {
        let anchor = app
            .tree()
            .visual_rect(owner)
            .ok_or_else(|| "tooltip owner is not laid out".to_owned())?;
        let plan = tooltip_plan(anchor, app.viewport(), requested);
        let surface: Rc<dyn Fn(f32, bool) -> Element> = Rc::new(surface);
        let mut options = PopupOptions::tooltip();
        options.placement = placement(plan);
        options.dismiss = DismissPolicy::MANUAL;
        let popup = super::popover::AnimatedPickerHost::open_options(
            app,
            owner,
            surface(plan.width, plan.fixed),
            options,
            0.0,
        )?;
        let overlay = popup.overlay();
        if plan.fixed && app.set_popup_uncontained(overlay, true).is_none() {
            app.close_popup(overlay);
            return Err("tooltip could not be positioned".to_owned());
        }
        Ok(Self {
            popup,
            started: std::time::Instant::now(),
            owner,
            placement: requested,
            plan,
            surface,
        })
    }

    pub fn sync(&mut self, app: &mut aqua::UiApp) -> Result<bool, String> {
        if !self.popup.is_open(app) {
            return Ok(false);
        }
        self.popup
            .advance(app, self.started.elapsed().as_secs_f64())?;
        if !self.popup.is_open(app) {
            return Ok(false);
        }
        let moving = self.popup.is_animating(app);
        if self.popup.is_closing() {
            return Ok(moving);
        }
        let anchor = app
            .tree()
            .visual_rect(self.owner)
            .ok_or_else(|| "tooltip owner is not laid out".to_owned())?;
        let next = tooltip_plan(anchor, app.viewport(), self.placement);
        if next == self.plan {
            return Ok(moving);
        }
        self.popup
            .refresh_content(app, (self.surface)(next.width, next.fixed))?;
        app.set_popup_placement(self.popup.overlay(), placement(next))
            .ok_or_else(|| "tooltip could not be positioned".to_owned())?;
        if next.fixed != self.plan.fixed {
            app.set_popup_uncontained(self.popup.overlay(), next.fixed)
                .ok_or_else(|| "tooltip could not be positioned".to_owned())?;
        }
        self.plan = next;
        Ok(true)
    }

    pub fn root(&self, app: &aqua::UiApp) -> Option<NodeId> {
        app.host().popup_root(self.popup.overlay())
    }

    pub fn is_open(&self, app: &aqua::UiApp) -> bool {
        app.host().is_open(self.popup.overlay())
    }

    pub fn dispose(self, app: &mut aqua::UiApp) {
        self.popup.dispose(app);
    }

    pub fn begin_close(&mut self, app: &mut aqua::UiApp) -> Result<(), String> {
        self.popup
            .begin_close(app, self.started.elapsed().as_secs_f64())
    }
}
