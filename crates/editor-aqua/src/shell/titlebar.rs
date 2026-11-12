use crate::shell::layout::{self, TITLEBAR_HEIGHT};
use crate::ui::kit::pane::split;
use crate::ui::kit::{
    Caption, CaptionButton, IconSize, Link, LinkTone, Space, Spaced, Surface, TextRole, Tooltip,
};
use crate::ui::theme::ArtisanTheme;
use aqua::prelude::{CaptionControl, CaptionRegion, DragRegions, ObservedWindowState};
use aqua_text::StyledSpan;
use aqua_ui::{
    AlignItems,
    element::{Box as UiBox, Canvas, Element, Text},
    style::{Length, Style},
    tree::ActivationSource,
};
use editor_assets::AssetId;
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Rc,
};

pub const LEAD: &str = "artisan-titlebar-drag-lead";
pub const WORDMARK: &str = "artisan-sidebar-wordmark";
const DRAG: &str = "artisan-titlebar-drag";
const METER: &str = "artisan-titlebar-meter";
const MIN: &str = "artisan-desktop-titlebar-minimize";
const MAX: &str = "artisan-desktop-titlebar-maximize";
const CLOSE: &str = "artisan-desktop-titlebar-close";
const CARD_WIDTH: f32 = 280.0;

pub struct ContextMeterTooltip {
    hovered: Rc<Cell<bool>>,
    popup: Option<aqua_ui::overlay::OverlayId>,
    reading: Option<crate::shell::context_meter::ContextReading>,
    anchor: Option<aqua_core::NodeId>,
    from: f32,
    target: f32,
    started: f64,
    value: f32,
}

impl Default for ContextMeterTooltip {
    fn default() -> Self {
        Self {
            hovered: Rc::new(Cell::new(false)),
            popup: None,
            reading: None,
            anchor: None,
            from: 0.0,
            target: 0.0,
            started: 0.0,
            value: 0.0,
        }
    }
}

impl ContextMeterTooltip {
    pub fn ring(
        &self,
        reading: &crate::shell::context_meter::ContextReading,
        theme: ArtisanTheme,
    ) -> Element {
        let hovered = Rc::clone(&self.hovered);
        crate::shell::context_meter::ring(reading, theme, move |value| hovered.set(value))
    }

    pub fn sync(
        &mut self,
        app: &mut aqua::UiApp,
        reading: Option<&crate::shell::context_meter::ContextReading>,
        theme: ArtisanTheme,
        now: f64,
        reduced_motion: bool,
    ) -> Result<bool, String> {
        let anchor = app.find_debug_selector("artisan-native-context-meter");
        if self.anchor != anchor || self.reading.as_ref() != reading {
            self.dispose(app);
        }
        let desired = self.hovered.get() && reading.is_some() && anchor.is_some();
        if desired && self.popup.is_none() {
            let owner = anchor.ok_or_else(|| "context meter is not mounted".to_owned())?;
            let reading = reading.ok_or_else(|| "context reading is unavailable".to_owned())?;
            let mut options = aqua_ui::popup::PopupOptions::tooltip();
            options.dismiss = aqua_ui::overlay::DismissPolicy::MANUAL;
            options.placement = aqua_ui::overlay::Placement {
                preferred: aqua_ui::overlay::Side::Below,
                align: aqua_ui::overlay::Align::End,
                gap: Space::S8.px(),
                margin: 0.0,
                flip: false,
                centered: false,
            };
            self.popup = Some(
                app.open_popup_measured(owner, context_card(reading, theme), options)
                    .map_err(|error| error.to_string())?,
            );
            self.anchor = Some(owner);
            self.reading = Some(reading.clone());
            self.value = 0.0;
            self.from = 0.0;
            self.target = 0.0;
        }
        let target = if desired { 1.0 } else { 0.0 };
        if target != self.target {
            self.from = self.value;
            self.target = target;
            self.started = now;
        }
        let duration = if desired { 0.25 } else { 0.15 };
        let progress = if reduced_motion {
            1.0
        } else {
            ((now - self.started) / duration).clamp(0.0, 1.0) as f32
        };
        self.value = (self.target - self.from)
            .mul_add(crate::ui::picker_motion::smooth_out(progress), self.from);
        if let Some(popup) = self.popup {
            if let Some(root) = app.host().popup_root(popup) {
                let mut paint = app
                    .tree()
                    .paint(root)
                    .ok_or_else(|| "context card is not mounted".to_owned())?;
                paint.opacity = Some(self.value);
                let rest = if desired { 0.97 } else { 0.99 };
                let scale = (1.0_f32 - rest).mul_add(self.value, rest);
                let width = app
                    .host()
                    .service()
                    .bounds(popup)
                    .map_or(0.0, |bounds| bounds.w);
                paint.transform = Some(
                    aqua_core::Transform2D::translate(-width, 0.0)
                        .then(aqua_core::Transform2D::scale(scale, scale))
                        .then(aqua_core::Transform2D::translate(width, 0.0)),
                );
                app.tree_mut().set_paint(root, paint);
            }
            if !desired && progress >= 1.0 {
                self.dispose(app);
            }
        }
        Ok(self.popup.is_some() && progress < 1.0)
    }

    pub fn dispose(&mut self, app: &mut aqua::UiApp) {
        if let Some(popup) = self.popup.take() {
            app.close_popup(popup);
        }
        self.reading = None;
        self.anchor = None;
        self.value = 0.0;
        self.target = 0.0;
        self.from = 0.0;
    }

    pub fn reset(&mut self, app: &mut aqua::UiApp) {
        self.hovered.set(false);
        self.dispose(app);
    }
}

fn context_card(
    reading: &crate::shell::context_meter::ContextReading,
    theme: ArtisanTheme,
) -> Element {
    let prose = |sentence: String, facts: Vec<String>| {
        let channels = |color: aqua_core::GpuiTextColor| {
            [
                color.hue_degrees,
                color.saturation,
                color.lightness,
                color.alpha,
            ]
        };
        let span = |words: &str, ink: crate::ui::theme::Oklch| {
            StyledSpan::new(words, TextRole::Caption.style(&theme))
                .gpui_source_color(channels(ink.to_reference_text_color()))
        };
        let mut ranges: Vec<_> = facts
            .into_iter()
            .filter(|fact| !fact.is_empty())
            .filter_map(|fact| sentence.find(&fact).map(|start| start..start + fact.len()))
            .collect();
        ranges.sort_by_key(|range| range.start);
        let mut spans = Vec::new();
        let mut at = 0;
        for range in ranges {
            if range.start < at {
                continue;
            }
            if range.start > at {
                spans.push(span(&sentence[at..range.start], theme.colors.text_muted));
            }
            spans.push(span(&sentence[range.clone()], theme.colors.text));
            at = range.end;
        }
        if at < sentence.len() {
            spans.push(span(&sentence[at..], theme.colors.text_muted));
        }
        Text::new(sentence).spans(spans).build()
    };
    let percent = format!("{}%", reading.percent().round());
    let size = crate::shell::context_meter::compact_tokens(reading.window_tokens);
    let lines = UiBox::new()
        .layout("column")
        .spaced(Space::S4)
        .child(TextRole::Title.text(&theme, "Context Window").build())
        .child(prose(
            reading.percent_sentence(),
            vec![reading.model_name.clone(), percent],
        ))
        .child(prose(
            reading.capacity_sentence(),
            vec![reading.model_name.clone(), size],
        ))
        .build();
    let progress =
        crate::ui::progress::bar(&theme, reading.percent(), Space::S8.px()).aria_label("Progress");
    let body = UiBox::new()
        .layout("column")
        .spaced(Space::S12)
        .child(lines)
        .child(progress)
        .build();
    Surface::Tooltip(Tooltip::Card)
        .build(&theme, vec![body])
        .style(Style::new().width(Length::Px(CARD_WIDTH)))
        .debug_selector("artisan-native-context-meter-card")
}

#[derive(Clone, Default)]
pub struct Titlebar {
    regions: Rc<RefCell<Option<DragRegions>>>,
    actions: Rc<RefCell<VecDeque<CaptionControl>>>,
    hovered: Rc<Cell<Option<CaptionControl>>>,
    changed: Rc<Cell<bool>>,
}

impl Titlebar {
    pub fn thread_header(
        &self,
        core: &crate::core::Core,
        conversation: &crate::conversation::controller::Conversation,
        theme: ArtisanTheme,
        open_url: Rc<dyn Fn(String)>,
        open_agent: Rc<dyn Fn(Option<artisan_api::ItemId>)>,
    ) -> Option<Element> {
        let crate::core::Screen::Thread { host, thread } = core.workspace_screen() else {
            return None;
        };
        let host = core.host(host)?;
        let record = host.threads.get(thread);
        let title = record
            .and_then(|thread| thread.title.clone())
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| "Thread".to_owned());
        let project = record.and_then(|thread| host.projects.get(&thread.project_id));
        let repository = record
            .and_then(|thread| host.repository(&thread.project_id))
            .filter(|repository| repository.web_url.is_some());
        let text = |value: String, ink: crate::ui::theme::Oklch| {
            TextRole::Label
                .text(&theme, value)
                .gpui_source_color(ink.to_reference_text_color())
                .style(Style::new().flex_shrink(0.0))
                .build()
        };
        let separator = || text("/".to_owned(), theme.colors.gray.step(7));
        let mut cluster = UiBox::new()
            .layout("row")
            .spaced(Space::S6)
            .clip(true)
            .style(
                Style::new()
                    .align(AlignItems::CENTER)
                    .min_width(Length::Pct(0.0)),
            );
        if let Some(project) = project {
            if let Some(repository) = repository {
                let url = repository.web_url.as_ref()?.clone();
                let asset = match repository.host.as_deref() {
                    Some("github") => AssetId::SVGL_GITHUB,
                    Some("gitlab") => AssetId::SVGL_GITLAB,
                    Some("azure") => AssetId::SVGL_MICROSOFT_AZURE,
                    Some("bitbucket") => AssetId::SIMPLE_ICONS_BITBUCKET,
                    Some("codeberg") => AssetId::SIMPLE_ICONS_CODEBERG,
                    Some("gitea") => AssetId::SIMPLE_ICONS_GITEA,
                    Some("sourcehut") => AssetId::SIMPLE_ICONS_SOURCEHUT,
                    _ => AssetId::SVGL_GIT,
                };
                let label = crate::sidebar::list::qualified_label(&url);
                let activate = Rc::clone(&open_url);
                cluster = cluster
                    .child(
                        IconSize::Md
                            .glyph(asset, theme.colors.text.to_reference_paint())
                            .style(Style::new().flex_shrink(0.0)),
                    )
                    .child(
                        Link::new(
                            "artisan-desktop-titlebar-repository-link",
                            theme,
                            LinkTone::Link,
                            label.clone(),
                        )
                        .on_activate(move || activate(url.clone()))
                        .build()
                        .style(Style::new().flex_shrink(0.0))
                        .aria_label(format!("Open {label}")),
                    );
            } else {
                cluster = cluster.child(
                    UiBox::new()
                        .layout("row")
                        .spaced(Space::S6)
                        .style(Style::new().align(AlignItems::CENTER).flex_shrink(0.0))
                        .child(IconSize::Md.glyph(
                            AssetId::TABLER_FOLDER,
                            theme.colors.text_muted.to_reference_paint(),
                        ))
                        .child(text(project.name.clone(), theme.colors.text))
                        .build(),
                );
            }
            cluster = cluster.child(separator());
        }
        let trail = conversation.agent_trail();
        let title = if trail.is_empty() {
            TextRole::Label
                .text(&theme, title)
                .gpui_source_color(theme.colors.text_muted.to_reference_text_color())
                .overflow(aqua_text::overflow::OverflowMode::EndEllipsis)
                .style(
                    Style::new()
                        .min_width(Length::Pct(0.0))
                        .flex_grow(1.0)
                        .text_wrap(false),
                )
                .build()
        } else {
            let activate = Rc::clone(&open_agent);
            Link::new(
                "artisan-desktop-titlebar-title",
                theme,
                LinkTone::Crumb,
                title,
            )
            .on_activate(move || activate(None))
            .build()
            .aria_label("Open parent thread")
        };
        cluster = cluster.child(title.debug_selector("artisan-desktop-titlebar-title"));
        let last = trail.len().saturating_sub(1);
        for (index, (agent, name)) in trail.into_iter().enumerate() {
            cluster = cluster.child(separator());
            let key = format!("artisan-desktop-titlebar-agent-{agent}");
            if index == last {
                cluster = cluster.child(text(name, theme.colors.text).with_key(key));
            } else {
                let activate = Rc::clone(&open_agent);
                cluster = cluster.child(
                    Link::new(key, theme, LinkTone::Crumb, name)
                        .current(false)
                        .on_activate(move || activate(Some(agent.clone())))
                        .build()
                        .style(Style::new().flex_shrink(0.0))
                        .aria_label("Open parent agent"),
                );
            }
        }
        Some(
            cluster
                .build()
                .debug_selector("artisan-desktop-titlebar-header"),
        )
    }

    pub fn regions(&self) -> Option<DragRegions> {
        self.regions.borrow().clone()
    }

    pub fn take_actions(&self) -> VecDeque<CaptionControl> {
        std::mem::take(&mut *self.actions.borrow_mut())
    }

    pub fn take_changed(&self) -> bool {
        self.changed.replace(false)
    }

    pub fn entry(&self, theme: ArtisanTheme, window: &ObservedWindowState) -> Element {
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

    pub fn workspace(
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
        let regions = self.regions.clone();
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
        let stroke = (1.0 / window.scale_factor) as f32;
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
        let actions = self.actions.clone();
        let hovered = self.hovered.clone();
        let changed = self.changed.clone();
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
