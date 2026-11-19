use crate::shell::layout::{self, TITLEBAR_HEIGHT};
use crate::ui::kit::pane::split;
use crate::ui::kit::{Caption, CaptionButton};
use crate::ui::theme::ArtisanTheme;
use aqua::prelude::{CaptionControl, CaptionRegion, DragRegions, ObservedWindowState};
use aqua_ui::{
    AlignItems,
    element::{Box as UiBox, Canvas, Element},
    style::{Length, Style},
    tree::ActivationSource,
};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Rc,
};

pub(crate) const LEAD: &str = "artisan-titlebar-drag-lead";
pub(crate) const WORDMARK: &str = "artisan-sidebar-wordmark";
const DRAG: &str = "artisan-titlebar-drag";
const METER: &str = "artisan-titlebar-meter";
const MIN: &str = "artisan-desktop-titlebar-minimize";
const MAX: &str = "artisan-desktop-titlebar-maximize";
const CLOSE: &str = "artisan-desktop-titlebar-close";

#[derive(Debug, Clone, Default)]
pub struct Titlebar {
    regions: Rc<RefCell<Option<DragRegions>>>,
    actions: Rc<RefCell<VecDeque<CaptionControl>>>,
    hovered: Rc<Cell<Option<CaptionControl>>>,
    changed: Rc<Cell<bool>>,
}

impl Titlebar {
    pub(crate) fn regions(&self) -> Option<DragRegions> {
        self.regions.borrow().clone()
    }

    pub(crate) fn take_actions(&self) -> VecDeque<CaptionControl> {
        std::mem::take(&mut *self.actions.borrow_mut())
    }

    pub(crate) fn take_changed(&self) -> bool {
        self.changed.replace(false)
    }

    pub(crate) fn entry(&self, theme: ArtisanTheme, window: &ObservedWindowState) -> Element {
        let content = UiBox::new()
            .layout("row")
            .width(Length::Pct(100.0))
            .height(TITLEBAR_HEIGHT)
            .style(Style::new().flex_shrink(0.0).align(AlignItems::CENTER))
            .child(drag(DRAG))
            .child(self.controls(theme, window))
            .build();
        self.measured(content, false, false, TITLEBAR_HEIGHT)
    }

    pub(crate) fn workspace(
        &self,
        theme: ArtisanTheme,
        window: &ObservedWindowState,
        header: Vec<Element>,
        meter: Option<Element>,
        inspector_width: Option<f32>,
    ) -> Element {
        let meter_right = layout::meter_right(inspector_width);
        let inset = layout::header_right_inset(meter_right, meter.is_some());
        let mut inner = UiBox::new()
            .layout("row")
            .height(Length::Pct(100.0))
            .clip(true)
            .style(
                Style::new()
                    .flex_grow(1.0)
                    .min_width(Length::Pct(0.0))
                    .align(AlignItems::CENTER),
            );
        for child in header {
            inner = inner.child(child);
        }
        inner = inner.child(drag(DRAG)).child(
            UiBox::new()
                .width(inset)
                .style(Style::new().flex_shrink(0.0))
                .build(),
        );
        let content = UiBox::new()
            .layout("row")
            .height(Length::Pct(100.0))
            .clip(true)
            .style(
                Style::new()
                    .flex_grow(1.0)
                    .min_width(Length::Pct(0.0))
                    .align(AlignItems::CENTER),
            )
            .child(inner.build())
            .build();
        let mut root = UiBox::new()
            .layout("row")
            .width(Length::Pct(100.0))
            .height(crate::frame::strip::BAR_HEIGHT)
            .style(Style::new().flex_shrink(0.0).align(AlignItems::CENTER))
            .child(content)
            .child(self.controls(theme, window));
        let has_meter = meter.is_some();
        if let Some(meter) = meter {
            root = root.child(
                UiBox::new()
                    .height(crate::frame::strip::BAR_HEIGHT)
                    .style(
                        Style::new()
                            .absolute_at(None, Some(0.0), Some(meter_right), None)
                            .align(AlignItems::CENTER),
                    )
                    .child(meter)
                    .build()
                    .debug_selector(METER),
            );
        }
        if let Some(width) = inspector_width {
            root = root.child(split(&theme).style(Style::new().absolute_at(
                None,
                Some(0.0),
                Some(width - 1.0),
                Some(0.0),
            )));
        }
        self.measured(
            root.build().debug_selector("artisan-desktop-titlebar"),
            true,
            has_meter,
            crate::frame::strip::BAR_HEIGHT,
        )
    }

    fn measured(&self, content: Element, lead: bool, meter: bool, height: f32) -> Element {
        let regions = Rc::clone(&self.regions);
        Canvas::new()
            .width(Length::Pct(100.0))
            .height(height)
            .style(Style::new().flex_shrink(0.0))
            .on_prepaint(move |tree, _| {
                let bounds = |selector: &str| {
                    let mut nodes = tree.traverse().into_iter().filter(|node| {
                        tree.get(*node).is_some_and(|record| {
                            record.debug_selector.as_deref() == Some(selector)
                        })
                    });
                    let node = nodes.next()?;
                    if nodes.next().is_some() {
                        return None;
                    }
                    let rect = tree.visual_rect(node)?;
                    Some((
                        f64::from(rect.x),
                        f64::from(rect.y),
                        f64::from(rect.w),
                        f64::from(rect.h),
                    ))
                };
                let measured = (|| {
                    let mut result = DragRegions::default();
                    result.draggable.push(bounds(DRAG)?);
                    if lead {
                        result.draggable.push(bounds(LEAD)?);
                        result.no_drag.push(bounds(WORDMARK)?);
                    }
                    if meter {
                        result.no_drag.push(bounds(METER)?);
                    }
                    for (control, selector) in [
                        (CaptionControl::Minimize, MIN),
                        (CaptionControl::Maximize, MAX),
                        (CaptionControl::Close, CLOSE),
                    ] {
                        result.caption_controls.push(CaptionRegion {
                            control,
                            bounds: bounds(selector)?,
                        });
                    }
                    Some(result)
                })();
                *regions.borrow_mut() = measured;
            })
            .child(content)
            .build()
    }

    fn controls(&self, theme: ArtisanTheme, window: &ObservedWindowState) -> Element {
        let stroke = crate::measure::narrow(1.0 / window.scale_factor);
        UiBox::new()
            .layout("row")
            .height(Length::Pct(100.0))
            .style(Style::new().flex_shrink(0.0))
            .child(self.button(theme, CaptionControl::Minimize, window.maximized, stroke))
            .child(self.button(theme, CaptionControl::Maximize, window.maximized, stroke))
            .child(self.button(theme, CaptionControl::Close, window.maximized, stroke))
            .build()
    }

    fn button(
        &self,
        theme: ArtisanTheme,
        control: CaptionControl,
        maximized: bool,
        stroke: f32,
    ) -> Element {
        let (selector, label, caption) = match (control, maximized) {
            (CaptionControl::Minimize, _) => (MIN, "Minimize window", Caption::Minimize),
            (CaptionControl::Maximize, true) => (MAX, "Restore window", Caption::Restore),
            (CaptionControl::Maximize, false) => (MAX, "Maximize window", Caption::Maximize),
            (CaptionControl::Close, _) => (CLOSE, "Close window", Caption::Close),
        };
        let actions = Rc::clone(&self.actions);
        let hovered = Rc::clone(&self.hovered);
        let changed = Rc::clone(&self.changed);
        CaptionButton::new(selector, theme, caption, label)
            .stroke(stroke)
            .lit(self.hovered.get() == Some(control))
            .on_activate(move |event| {
                if matches!(event.source, Some(ActivationSource::Keyboard(_))) {
                    actions.borrow_mut().push_back(control);
                }
            })
            .on_hover(move |value| {
                let before = hovered.get();
                if value {
                    hovered.set(Some(control));
                } else if hovered.get() == Some(control) {
                    hovered.set(None);
                }
                changed.set(changed.get() || hovered.get() != before);
            })
            .build()
            .debug_selector(selector)
    }
}

fn drag(selector: &'static str) -> Element {
    UiBox::new()
        .width(Length::Pct(0.0))
        .height(Length::Pct(100.0))
        .style(Style::new().flex_grow(1.0))
        .build()
        .debug_selector(selector)
}
