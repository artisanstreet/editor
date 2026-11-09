use crate::shell::layout::{CONTROL_GLYPH, CONTROL_WIDTH, TITLEBAR_HEIGHT};
use crate::ui::theme::{ArtisanTheme, Opacity};
use aqua::prelude::{CaptionControl, CaptionRegion, DragRegions, ObservedWindowState};
use aqua_core::{GpuiBackground, Insets};
use aqua_ui::{
    AlignItems, JustifyContent,
    element::{Box as UiBox, Button, Canvas, Element, Text},
    style::{Length, Style},
    tree::ActivationSource,
};
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
                gap: 8.0,
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
    use crate::ui::theme::{Elevation, RadiusStep, RadiusTokens};
    use aqua_text::{FontFamily, FontWeight, StyledSpan, TextStyle};
    let style = |size, line, weight| TextStyle {
        families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
        size_px: size,
        line_height_px: line,
        weight,
        ..TextStyle::default()
    };
    let prose = |sentence: String, facts: Vec<String>| {
        let channels = |color: aqua_core::GpuiTextColor| {
            [
                color.hue_degrees,
                color.saturation,
                color.lightness,
                color.alpha,
            ]
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
                spans.push(
                    StyledSpan::new(
                        &sentence[at..range.start],
                        style(theme.typography.label_text, 19.0, FontWeight::NORMAL),
                    )
                    .gpui_source_color(channels(theme.colors.text_muted.to_reference_text_color())),
                );
            }
            spans.push(
                StyledSpan::new(
                    &sentence[range.clone()],
                    style(theme.typography.label_text, 19.0, FontWeight::NORMAL),
                )
                .gpui_source_color(channels(theme.colors.text.to_reference_text_color())),
            );
            at = range.end;
        }
        if at < sentence.len() {
            spans.push(
                StyledSpan::new(
                    &sentence[at..],
                    style(theme.typography.label_text, 19.0, FontWeight::NORMAL),
                )
                .gpui_source_color(channels(theme.colors.text_muted.to_reference_text_color())),
            );
        }
        Text::new(sentence).spans(spans).build()
    };
    let percent = format!("{}%", reading.percent().round());
    let size = crate::shell::context_meter::compact_tokens(reading.window_tokens);
    let title = Text::new("Context Window")
        .text_style(style(theme.typography.control_text, 18.0, FontWeight(600)))
        .gpui_source_color(theme.colors.text.to_reference_text_color())
        .build();
    let lines = UiBox::new()
        .layout("column")
        .gap(4.0)
        .child(title)
        .child(prose(
            reading.percent_sentence(),
            vec![reading.model_name.clone(), percent],
        ))
        .child(prose(
            reading.capacity_sentence(),
            vec![reading.model_name.clone(), size],
        ))
        .build();
    let fraction = (reading.percent() / 100.0) as f32;
    let fill = match reading.tone() {
        crate::shell::context_meter::ContextTone::Normal => {
            aqua_core::GpuiBackground::LinearGradient {
                angle: 90.0,
                color_space: aqua_core::GpuiGradientColorSpace::Srgb,
                stops: [
                    aqua_core::GpuiGradientStop {
                        percentage: 0.0,
                        color: theme.colors.action_bottom.to_reference_paint(),
                    },
                    aqua_core::GpuiGradientStop {
                        percentage: 1.0,
                        color: theme.colors.action_top.to_reference_paint(),
                    },
                ],
            }
        }
        crate::shell::context_meter::ContextTone::Warning => {
            aqua_core::GpuiBackground::Solid(theme.colors.banner_warning.text.to_reference_paint())
        }
        crate::shell::context_meter::ContextTone::Danger => {
            aqua_core::GpuiBackground::Solid(theme.colors.banner_error.text.to_reference_paint())
        }
    };
    let radius = RadiusTokens::value(RadiusStep::Xs);
    let progress = UiBox::new()
        .width(Length::Pct(100.0))
        .height(8.0)
        .radius(radius)
        .clip(true)
        .style(
            Style::new().gpui_background(aqua_core::GpuiBackground::Solid(
                theme
                    .colors
                    .text
                    .at(Opacity::ProgressTrack)
                    .to_reference_paint(),
            )),
        )
        .child(
            UiBox::new()
                .width(Length::Pct(fraction * 100.0))
                .height(Length::Pct(100.0))
                .radius(radius)
                .style(Style::new().gpui_background(fill))
                .build(),
        )
        .build()
        .role(aqua_core::Role::ProgressIndicator)
        .aria_label("Progress")
        .aria_value(format!("{:.0}%", reading.percent()));
    let body = UiBox::new()
        .layout("column")
        .gap(12.0)
        .padding(16.0)
        .child(lines)
        .child(progress)
        .build();
    let radius = RadiusTokens::value(RadiusStep::X2l);
    UiBox::new()
        .width(280.0)
        .radius(radius)
        .style(
            Style::new()
                .gpui_border(
                    Insets::uniform(1.0),
                    theme
                        .colors
                        .text
                        .at(Opacity::GlassBorder)
                        .to_reference_paint(),
                    false,
                )
                .gpui_shadows(
                    crate::ui::glass::glass_card_shadows(&theme, Elevation::Overlay)
                        .into_iter()
                        .map(aqua_ui::style::GpuiShadowLayer::from)
                        .collect(),
                ),
        )
        .child(crate::ui::glass::glass_layers(
            &theme,
            crate::ui::glass::GlassStrength::Strong,
            radius,
        ))
        .child(body)
        .build()
        .debug_selector("artisan-native-context-meter-card")
}

#[derive(Clone, Default)]
pub struct Titlebar {
    regions: Rc<RefCell<Option<DragRegions>>>,
    actions: Rc<RefCell<VecDeque<CaptionControl>>>,
    hovered: Rc<Cell<Option<CaptionControl>>>,
    header_hovered: Rc<RefCell<Option<(crate::core::HostInstance, artisan_api::ThreadId, String)>>>,
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
        let instance = host.instance();
        let thread_id = thread.clone();
        let record = host.threads.get(thread);
        let title = record
            .and_then(|thread| thread.title.clone())
            .filter(|title| !title.trim().is_empty())
            .unwrap_or_else(|| "Thread".to_owned());
        let project = record.and_then(|thread| host.projects.get(&thread.project_id));
        let repository = record
            .and_then(|thread| host.repository(&thread.project_id))
            .filter(|repository| repository.web_url.is_some());
        let text = |value: String, color: aqua_core::GpuiTextColor| {
            Text::new(value)
                .text_style(aqua_text::TextStyle {
                    families: vec![aqua_text::FontFamily::Named(
                        theme.typography.sans.family.to_owned(),
                    )],
                    size_px: theme.typography.control_text,
                    line_height_px: 14.0 * 1.618_034,
                    letter_spacing_em: Some(theme.typography.tracking_em),
                    ..aqua_text::TextStyle::default()
                })
                .gpui_source_color(color)
                .style(Style::new().flex_shrink(0.0))
                .build()
        };
        let muted = theme.colors.text_muted.to_reference_text_color();
        let mut cluster = UiBox::new().layout("row").gap(6.0).clip(true).style(
            Style::new()
                .align(AlignItems::CENTER)
                .min_width(Length::Px(0.0)),
        );
        if let Some(project) = project {
            if let Some(repository) = repository {
                let url = repository.web_url.as_ref()?.clone();
                let asset = match repository.host.as_deref() {
                    Some("github") => editor_assets::AssetId::SVGL_GITHUB,
                    Some("gitlab") => editor_assets::AssetId::SVGL_GITLAB,
                    Some("azure") => editor_assets::AssetId::SVGL_MICROSOFT_AZURE,
                    Some("bitbucket") => editor_assets::AssetId::SIMPLE_ICONS_BITBUCKET,
                    Some("codeberg") => editor_assets::AssetId::SIMPLE_ICONS_CODEBERG,
                    Some("gitea") => editor_assets::AssetId::SIMPLE_ICONS_GITEA,
                    Some("sourcehut") => editor_assets::AssetId::SIMPLE_ICONS_SOURCEHUT,
                    _ => editor_assets::AssetId::SVGL_GIT,
                };
                let label = crate::sidebar::list::qualified_label(&url);
                let activate = Rc::clone(&open_url);
                cluster = cluster
                    .child(
                        UiBox::new()
                            .style(Style::new().flex_shrink(0.0))
                            .child(crate::ui::asset::source_asset_glyph(
                                asset,
                                14.0,
                                theme.colors.text.to_reference_paint(),
                            ))
                            .build(),
                    )
                    .child(
                        Button::new()
                            .style(Style::new().flex_shrink(0.0))
                            .child(text(
                                label.clone(),
                                theme.colors.link.to_reference_text_color(),
                            ))
                            .on_activate(move || activate(url.clone()))
                            .build()
                            .with_key("artisan-desktop-titlebar-repository-link")
                            .role(aqua_core::Role::Button)
                            .aria_label(format!("Open {label}"))
                            .cursor(aqua_core::CursorIcon::Pointer)
                            .focusable(true),
                    );
            } else {
                cluster = cluster.child(
                    UiBox::new()
                        .layout("row")
                        .gap(6.0)
                        .style(Style::new().align(AlignItems::CENTER).flex_shrink(0.0))
                        .child(crate::ui::asset::source_asset_glyph(
                            editor_assets::AssetId::TABLER_FOLDER,
                            14.0,
                            theme.colors.text_muted.to_reference_paint(),
                        ))
                        .child(text(
                            project.name.clone(),
                            theme.colors.text.to_reference_text_color(),
                        ))
                        .build(),
                );
            }
            cluster = cluster.child(text(
                "/".to_owned(),
                theme.colors.gray.step(7).to_reference_text_color(),
            ));
        }
        let trail = conversation.agent_trail();
        let title_key = "artisan-desktop-titlebar-title".to_owned();
        let title_hover = self.header_hovered.borrow().as_ref()
            == Some(&(instance, thread_id.clone(), title_key.clone()));
        let title_text = Text::new(title)
            .text_style(aqua_text::TextStyle {
                families: vec![aqua_text::FontFamily::Named(
                    theme.typography.sans.family.to_owned(),
                )],
                size_px: theme.typography.control_text,
                line_height_px: 14.0 * 1.618_034,
                letter_spacing_em: Some(theme.typography.tracking_em),
                ..aqua_text::TextStyle::default()
            })
            .gpui_source_color(if !trail.is_empty() && title_hover {
                theme.colors.text.to_reference_text_color()
            } else {
                muted
            })
            .overflow(aqua_text::overflow::OverflowMode::EndEllipsis)
            .style(
                Style::new()
                    .min_width(Length::Px(0.0))
                    .flex_grow(if trail.is_empty() { 1.0 } else { 0.0 }),
            )
            .build()
            .debug_selector("artisan-desktop-titlebar-title");
        if trail.is_empty() {
            cluster = cluster.child(title_text);
        } else {
            let activate = Rc::clone(&open_agent);
            let hover = self.header_hover_handler(instance, thread_id.clone(), title_key);
            cluster = cluster.child(
                Button::new()
                    .style(Style::new().min_width(Length::Px(0.0)).flex_shrink(1.0))
                    .child(title_text)
                    .on_activate(move || activate(None))
                    .build()
                    .with_key("artisan-desktop-titlebar-title")
                    .role(aqua_core::Role::Button)
                    .aria_label("Open parent thread")
                    .cursor(aqua_core::CursorIcon::Pointer)
                    .on_hover_change(hover)
                    .focusable(true),
            );
        }
        let last = trail.len().saturating_sub(1);
        for (index, (agent, name)) in trail.into_iter().enumerate() {
            cluster = cluster.child(text(
                "/".to_owned(),
                theme.colors.gray.step(7).to_reference_text_color(),
            ));
            let key = format!("artisan-desktop-titlebar-agent-{agent}");
            let hovered = self.header_hovered.borrow().as_ref()
                == Some(&(instance, thread_id.clone(), key.clone()));
            let crumb = text(
                name,
                if index == last || hovered {
                    theme.colors.text.to_reference_text_color()
                } else {
                    muted
                },
            );
            if index == last {
                cluster = cluster.child(crumb.with_key(key));
            } else {
                let activate = Rc::clone(&open_agent);
                let hover = self.header_hover_handler(instance, thread_id.clone(), key.clone());
                cluster = cluster.child(
                    Button::new()
                        .style(Style::new().flex_shrink(0.0))
                        .child(crumb)
                        .on_activate(move || activate(Some(agent.clone())))
                        .build()
                        .with_key(key)
                        .role(aqua_core::Role::Button)
                        .aria_label("Open parent agent")
                        .cursor(aqua_core::CursorIcon::Pointer)
                        .on_hover_change(hover)
                        .focusable(true),
                );
            }
        }
        Some(
            cluster
                .build()
                .debug_selector("artisan-desktop-titlebar-header"),
        )
    }

    fn header_hover_handler(
        &self,
        instance: crate::core::HostInstance,
        thread: artisan_api::ThreadId,
        key: String,
    ) -> impl Fn(bool) + 'static {
        let hovered = Rc::clone(&self.header_hovered);
        let changed = Rc::clone(&self.changed);
        move |over| {
            let identity = (instance, thread.clone(), key.clone());
            let mut current = hovered.borrow_mut();
            if over && current.as_ref() != Some(&identity) {
                *current = Some(identity);
                changed.set(true);
            } else if !over && current.as_ref() == Some(&identity) {
                *current = None;
                changed.set(true);
            }
        }
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
        let meter_right = inspector_width.unwrap_or(0.0).max(CONTROL_WIDTH * 3.0) + 12.0;
        let inset = if meter.is_some() {
            (meter_right + 24.0 - CONTROL_WIDTH * 3.0).max(0.0)
        } else {
            0.0
        };
        let mut inner = UiBox::new()
            .layout("row")
            .height(Length::Pct(100.0))
            .clip(true)
            .style(
                Style::new()
                    .flex_grow(1.0)
                    .min_width(Length::Px(0.0))
                    .align(AlignItems::CENTER)
                    .padding_insets(Insets {
                        right: inset,
                        ..Insets::default()
                    }),
            );
        for child in header {
            inner = inner.child(child);
        }
        inner = inner.child(drag(DRAG));
        let content = UiBox::new()
            .layout("row")
            .height(Length::Pct(100.0))
            .clip(true)
            .style(
                Style::new()
                    .flex_grow(1.0)
                    .min_width(Length::Px(0.0))
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
            root = root.child(
                UiBox::new()
                    .width(1.0)
                    .style(
                        Style::new()
                            .absolute_at(None, Some(0.0), Some(width - 1.0), Some(0.0))
                            .gpui_background(GpuiBackground::Solid(
                                theme.colors.line.to_reference_paint(),
                            )),
                    )
                    .build(),
            );
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
        let (selector, label, background) = match control {
            CaptionControl::Minimize => (MIN, "Minimize window", theme.colors.surface),
            CaptionControl::Maximize => (
                MAX,
                if maximized {
                    "Restore window"
                } else {
                    "Maximize window"
                },
                theme.colors.raised,
            ),
            CaptionControl::Close => (CLOSE, "Close window", theme.colors.red.solid),
        };
        let foreground = if self.hovered.get() == Some(control) {
            if control == CaptionControl::Close {
                theme.colors.red.contrast
            } else {
                theme.colors.text
            }
        } else {
            theme.colors.text_muted
        };
        let glyph = match control {
            CaptionControl::Minimize => crate::ui::asset::source_asset_glyph(
                editor_assets::AssetId::TABLER_MINUS,
                CONTROL_GLYPH,
                foreground.to_reference_paint(),
            ),
            CaptionControl::Close => crate::ui::asset::source_asset_glyph(
                editor_assets::AssetId::TABLER_X,
                CONTROL_GLYPH,
                foreground.to_reference_paint(),
            ),
            CaptionControl::Maximize => maximize(theme, maximized, stroke),
        };
        let actions = self.actions.clone();
        let hovered = self.hovered.clone();
        let changed = self.changed.clone();
        Button::new()
            .width(CONTROL_WIDTH)
            .height(Length::Pct(100.0))
            .style(
                Style::new()
                    .flex_shrink(0.0)
                    .align(AlignItems::CENTER)
                    .justify(JustifyContent::CENTER)
                    .gpui_border(
                        Insets::uniform(1.0),
                        aqua_core::GpuiHsla {
                            h: 0.0,
                            s: 0.0,
                            l: 0.0,
                            a: 0.0,
                        },
                        false,
                    ),
            )
            .on_activate_event(move |event| {
                if matches!(event.source, Some(ActivationSource::Keyboard(_))) {
                    actions.borrow_mut().push_back(control);
                }
            })
            .child(glyph)
            .build()
            .with_key(selector)
            .debug_selector(selector)
            .aria_label(label)
            .tab_index(0)
            .gpui_keyboard_activation()
            .hover_style(
                Style::new()
                    .gpui_background(GpuiBackground::Solid(background.to_reference_paint())),
            )
            .focus_visible_style(
                Style::new().gpui_border(
                    Insets::uniform(1.0),
                    theme
                        .colors
                        .ring
                        .at(Opacity::FocusRing)
                        .to_reference_paint(),
                    false,
                ),
            )
            .on_hover_change(move |value| {
                let before = hovered.get();
                if value {
                    hovered.set(Some(control));
                } else if hovered.get() == Some(control) {
                    hovered.set(None);
                }
                changed.set(changed.get() || hovered.get() != before);
            })
    }
}

fn drag(selector: &'static str) -> Element {
    UiBox::new()
        .width(Length::Px(0.0))
        .height(Length::Pct(100.0))
        .style(Style::new().flex_grow(1.0))
        .build()
        .debug_selector(selector)
}

fn maximize(theme: ArtisanTheme, maximized: bool, stroke: f32) -> Element {
    let color = theme.colors.text.to_reference_paint();
    let mut glyph = UiBox::new().width(10.0).height(10.0);
    if maximized {
        glyph = glyph
            .child(
                UiBox::new()
                    .width(7.0)
                    .height(7.0)
                    .style(
                        Style::new()
                            .absolute_at(None, Some(0.0), Some(0.0), None)
                            .gpui_border(
                                Insets {
                                    top: stroke,
                                    right: stroke,
                                    ..Insets::default()
                                },
                                color,
                                false,
                            ),
                    )
                    .build(),
            )
            .child(
                UiBox::new()
                    .width(7.0)
                    .height(7.0)
                    .style(
                        Style::new()
                            .absolute_at(Some(0.0), None, None, Some(0.0))
                            .gpui_border(Insets::uniform(stroke), color, false),
                    )
                    .build(),
            );
    } else {
        glyph = glyph.style(Style::new().gpui_border(Insets::uniform(stroke), color, false));
    }
    glyph.build()
}
