//! Trigger and menu rendering for the model selector: the compact trigger, the
//! bounded popover, the engine tabs, and the engine-light and surface helpers
//! that paint them.
//!
//! Split from `native_model_selector/render.rs`; the sibling `rows` child
//! paints the model rows, preview pane, and policy axes.

use super::*;

impl NativeModelSelector {
    fn render_trigger(&self, cx: &Context<Self>) -> Stateful<Div> {
        let foreground = self.theme.colors.foreground.to_paint();
        let engine = self.state.policy().map_or_else(
            || self.state.active_engine().to_owned(),
            |policy| policy.engine_id.clone(),
        );
        let label = self.state.trigger_label();
        let mut trigger = div()
            .id("artisan-native-model-selector-trigger")
            .track_focus(&self.trigger_focus)
            .debug_selector(|| NATIVE_MODEL_SELECTOR_TRIGGER_SELECTOR.to_owned())
            .role(gpui::Role::Button)
            .aria_label("Select model")
            .on_click(cx.listener(Self::handle_trigger_click))
            .flex()
            .items_center()
            .gap(px(8.0))
            .h(px(COMPACT_CONTROL_HEIGHT_PX))
            .max_w(px(360.0))
            .px(px(8.0))
            .rounded(px(10.0))
            .hover(move |style| style.bg(hover_fill_gradient(self.theme)))
            .focus_visible(move |style| style.shadow(source_focus_ring(&self.theme)))
            .text_color(foreground);
        trigger = trigger.child(
            icon(IconStyle::resolve(
                self.theme,
                engine_asset(&engine),
                IconSize::Default,
                IconTint::Muted,
            ))
            .size(px(16.0))
            .flex_shrink_0(),
        );
        let mut body = div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .flex_1()
            .min_w(px(0.0))
            .overflow_hidden();
        let muted = self.theme.colors.muted_foreground.to_paint();
        for token in label.tokens() {
            body = body.child(render_trigger_label_token(muted, token));
        }
        trigger = trigger.child(body);
        trigger.child(
            icon(IconStyle::resolve(
                self.theme,
                AssetId::TABLER_SELECTOR,
                IconSize::Compact,
                IconTint::Muted,
            ))
            .size(px(14.0))
            .flex_shrink_0(),
        )
    }

    fn render_menu(&self, viewport: Size<Pixels>, cx: &Context<Self>) -> Option<AnyElement> {
        if self.menu_motion.borrow().phase() == PickerMenuPhase::Hidden {
            self.menu_bounds.borrow_mut().take();
            return None;
        }
        let bounds = Rc::clone(&self.menu_bounds);
        let bounds_probe = canvas(
            |_, _, _| {},
            move |painted, (), _, _| {
                *bounds.borrow_mut() = Some(painted);
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let origin = self.trigger_origin.borrow().as_ref().copied()?;
        let mut panel = div()
            .id("artisan-native-model-selector-menu")
            .on_mouse_down_out(cx.listener(Self::handle_outside_press))
            .occlude()
            .relative()
            .child(bounds_probe)
            .track_focus(&self.menu_focus)
            .debug_selector(|| NATIVE_MODEL_SELECTOR_MENU_SELECTOR.to_owned())
            .on_key_down(cx.listener(Self::handle_menu_key))
            .flex()
            .flex_col()
            .overflow_hidden()
            .w(menu_width_for_viewport(viewport))
            .max_h(menu_max_height_for_viewport(viewport))
            .p(px(8.0))
            .gap(px(8.0))
            .rounded(px(22.0))
            .backdrop_blur(glass_blur_radius(GlassStrength::Strong))
            .bg(glass_foreground_base(&self.theme))
            .text_color(self.theme.colors.foreground.to_paint())
            .shadow(source_menu_shadows(&self.theme));
        panel = panel.child(glass_material_layer(GlassStrength::Strong, px(22.0)));
        panel = panel.child(glass_highlight_layer(GlassStrength::Strong, px(22.0)));
        panel = panel.child(self.render_engine_tabs(cx));
        panel = panel.child(
            div()
                .flex()
                .flex_row()
                .h(px(MODEL_PANEL_HEIGHT_PX))
                .flex_shrink_0()
                .gap(px(8.0))
                .child(self.render_model_list(cx))
                .child(self.render_preview(viewport, cx)),
        );
        let motion = *self.menu_motion.borrow();
        Some(
            anchored()
                .anchor(Anchor::BottomLeft)
                .position(origin)
                .offset(point(px(0.0), px(-MENU_GAP_PX)))
                .child(animate_picker_menu(
                    panel,
                    self.menu_motion.clone(),
                    motion,
                    "main",
                ))
                .into_any_element(),
        )
    }

    #[expect(
        clippy::float_cmp,
        reason = "indicator geometry re-measures only when the measured tab position changes exactly; an epsilon would skip sub-pixel corrections"
    )]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "indicator geometry is measured in f64 from GPUI bounds and painted as f32 pixels"
    )]
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI builder composes the tab surface, measurement probe, and sliding indicator that share reactive state"
    )]
    fn render_engine_tabs(&self, cx: &Context<Self>) -> Stateful<Div> {
        let surface_bounds = Rc::clone(&self.engine_surface_bounds);
        let surface_probe = canvas(
            |_, _, _| {},
            move |bounds, (), window, cx| {
                let changed = {
                    let mut surface = surface_bounds.borrow_mut();
                    if *surface == Some(bounds) {
                        false
                    } else {
                        *surface = Some(bounds);
                        true
                    }
                };
                if changed {
                    window.defer(cx, |window, _| window.refresh());
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        let active_engine = self.state.active_engine().to_owned();
        let transition = *self.engine_indicator_transition.borrow();
        let indicator = {
            let indicator = self.engine_indicator.borrow();
            indicator.indicator_visible().then(|| {
                render_engine_light(
                    &self.theme,
                    px(indicator.indicator_left() as f32),
                    px(indicator.indicator_width() as f32),
                    transition,
                )
            })
        };
        let mut tabs = div()
            .id("artisan-model-engine-tabs")
            .relative()
            .flex()
            .flex_row()
            .w_full()
            .h(px(40.0))
            .gap(px(4.0))
            .p(px(4.0))
            .rounded(px(10.0))
            .overflow_x_scroll()
            .overflow_y_hidden()
            .scrollbar_width(px(0.0))
            .bg(source_control_gradient(&self.theme))
            .shadow(source_card_shadows(&self.theme))
            .child(surface_probe);
        if let Some(indicator) = indicator {
            tabs = tabs.child(indicator);
        }
        for harness in self
            .state
            .snapshot()
            .manifest
            .harnesses
            .iter()
            .filter(|harness| !harness.hidden)
        {
            let engine_id = harness.id.clone();
            let measured_engine_id = engine_id.clone();
            let indicator_policy = Rc::clone(&self.engine_indicator);
            let surface_bounds = Rc::clone(&self.engine_surface_bounds);
            let indicator_transition = Rc::clone(&self.engine_indicator_transition);
            let animation_generation = Rc::clone(&self.engine_indicator_animation_generation);
            let active_engine = active_engine.clone();
            let tab_probe = canvas(
                |_, _, _| {},
                move |bounds, (), window, cx| {
                    if measured_engine_id != active_engine {
                        return;
                    }
                    let Some(surface) = *surface_bounds.borrow() else {
                        return;
                    };
                    let measurement = EngineSectionIndicatorMeasurement::new(
                        f64::from(f32::from(surface.left())),
                        f64::from(f32::from(bounds.left())),
                        f64::from(f32::from(bounds.size.width)),
                    );
                    let changed = {
                        let mut indicator = indicator_policy.borrow_mut();
                        let previous_visible = indicator.indicator_visible();
                        let previous_left = indicator.indicator_left();
                        let previous_width = indicator.indicator_width();
                        let engine_changed =
                            indicator.lit_engine() != Some(measured_engine_id.as_str());
                        let next_left = measurement.tab_left - measurement.surface_left;
                        let geometry_changed =
                            previous_left != next_left || previous_width != measurement.tab_width;
                        if !previous_visible || engine_changed || geometry_changed {
                            indicator.measure(measured_engine_id.clone(), Some(measurement));
                            if previous_visible && engine_changed {
                                let mut generation = animation_generation.borrow_mut();
                                *generation = generation.saturating_add(1);
                                *indicator_transition.borrow_mut() =
                                    Some(EngineIndicatorTransition {
                                        from_left: previous_left,
                                        from_width: previous_width,
                                        to_left: next_left,
                                        to_width: measurement.tab_width,
                                        generation: *generation,
                                    });
                            } else {
                                // Resize remeasurement follows the source's
                                // instant geometry correction rather than
                                // replaying a tab-change animation.
                                *indicator_transition.borrow_mut() = None;
                            }
                            true
                        } else {
                            false
                        }
                    };
                    if changed {
                        window.defer(cx, |window, _| window.refresh());
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full();
            let selector = format!(
                "{NATIVE_MODEL_SELECTOR_ENGINE_SELECTOR_PREFIX}-{}",
                harness.id
            );
            let mut tab = div()
                .id(format!(
                    "{NATIVE_MODEL_SELECTOR_ENGINE_SELECTOR_PREFIX}-{}",
                    harness.id
                ))
                .debug_selector(move || selector.clone())
                .on_click(cx.listener(move |view: &mut Self, _: &ClickEvent, _, cx| {
                    view.switch_engine(engine_id.clone(), cx);
                }))
                .role(gpui::Role::Button)
                .aria_label(harness.label.clone())
                .flex()
                .items_center()
                .justify_center()
                .size(px(32.0))
                .flex_shrink_0()
                .rounded(px(14.0))
                .text_color(self.theme.colors.foreground.to_paint())
                .child(tab_probe);
            tab = tab.child(
                icon(IconStyle::resolve(
                    self.theme,
                    engine_asset(&harness.id),
                    IconSize::Compact,
                    IconTint::Inherit,
                ))
                .size(px(16.0))
                .flex_shrink_0(),
            );
            tabs = tabs.child(tab);
        }
        tabs
    }
}

impl Render for NativeModelSelector {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let viewport = window.viewport_size();
        let menu = self.render_menu(viewport, cx);
        let option_tooltip = self.render_option_tooltip(viewport);
        let trigger = self.render_trigger(cx);
        let origin = Rc::clone(&self.trigger_origin);
        let trigger_bounds = Rc::clone(&self.trigger_bounds);
        let scroll = self.menu_scroll.clone();
        let probe = canvas(
            move |_, _, _| {},
            move |bounds, (), window, cx| {
                *trigger_bounds.borrow_mut() = Some(bounds);
                let moved = *origin.borrow_mut() != Some(bounds.origin);
                *origin.borrow_mut() = Some(bounds.origin);
                if moved {
                    let scroll = scroll.clone();
                    window.defer(cx, move |window, _| {
                        scroll.scroll_to_item(0);
                        window.refresh();
                    });
                }
            },
        )
        .absolute()
        .size_full();
        div()
            .id("artisan-native-model-selector-root")
            .tab_group()
            .flex()
            .flex_col()
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .child(probe)
                    .children(menu.map(deferred))
                    .child(trigger),
            )
            .children(option_tooltip)
    }
}

pub(crate) fn animate_picker_menu(
    panel: Stateful<Div>,
    motion: Rc<RefCell<PickerMenuMotion>>,
    snapshot: PickerMenuMotion,
    surface: &'static str,
) -> AnyElement {
    let Some((from_opacity, from_offset, to_opacity, to_offset, generation)) =
        snapshot.transition()
    else {
        return panel.into_any_element();
    };
    let phase = snapshot.phase();
    let animation_id = ElementId::Name(
        format!(
            "artisan-native-model-selector-menu-{surface}-{}-{generation}",
            match phase {
                PickerMenuPhase::Opening => "opening",
                PickerMenuPhase::Closing => "closing",
                PickerMenuPhase::Hidden | PickerMenuPhase::Open => "settled",
            }
        )
        .into(),
    );
    panel
        .top(px(from_offset))
        .opacity(from_opacity)
        .with_animation(
            animation_id,
            Animation::new(Duration::from_millis(PICKER_MENU_MOTION_DURATION_MS))
                .with_easing(engine_light_smooth_out),
            move |panel, progress| {
                motion.borrow_mut().apply_progress(generation, progress);
                panel
                    .top(px(from_offset + (to_offset - from_offset) * progress))
                    .opacity(from_opacity + (to_opacity - from_opacity) * progress)
            },
        )
        .into_any_element()
}

fn menu_width_for_viewport(viewport: Size<Pixels>) -> Pixels {
    let available = f32::from(viewport.width) - MENU_VIEWPORT_INSET_X_PX;
    px(MENU_WIDTH_PX.min(available.max(0.0)))
}

fn menu_max_height_for_viewport(viewport: Size<Pixels>) -> Pixels {
    let available = f32::from(viewport.height) - MENU_VIEWPORT_INSET_Y_PX;
    px(MENU_MAX_HEIGHT_PX.min(available.max(0.0)))
}

pub(super) fn source_control_gradient(theme: &ArtisanTheme) -> gpui::Background {
    let (top, bottom) = match theme.mode {
        ThemeMode::Light => (SurfaceStep::S225, SurfaceStep::S200),
        ThemeMode::Dark => (SurfaceStep::S800, SurfaceStep::S925),
    };
    vertical_gradient(theme.surfaces.value(top), theme.surfaces.value(bottom))
}

pub(super) fn source_card_shadows(theme: &ArtisanTheme) -> Vec<gpui::BoxShadow> {
    card_shadows(theme)
}

fn source_menu_shadows(_theme: &ArtisanTheme) -> Vec<gpui::BoxShadow> {
    glass_card_shadows()
}

pub(super) fn source_focus_ring(theme: &ArtisanTheme) -> Vec<gpui::BoxShadow> {
    vec![gpui::BoxShadow {
        color: theme.interaction.focus_ring_color.to_paint(),
        offset: point(px(0.0), px(0.0)),
        blur_radius: px(0.0),
        spread_radius: theme.interaction.focus_ring_width,
        inset: false,
    }]
}

fn render_engine_light(
    theme: &ArtisanTheme,
    tab_left: Pixels,
    tab_width: Pixels,
    transition: Option<EngineIndicatorTransition>,
) -> AnyElement {
    let target_left = transition.map_or_else(
        || {
            centered_engine_light_left(
                f64::from(f32::from(tab_left)),
                f64::from(f32::from(tab_width)),
            )
        },
        |transition| centered_engine_light_left(transition.to_left, transition.to_width),
    );
    let image = engine_light_image(theme);
    let image = img(ImageSource::Render(image))
        .absolute()
        .top(px(-2.0))
        .left(px(target_left))
        .w(px(ENGINE_LIGHT_WIDTH_PX))
        .h(px(ENGINE_LIGHT_HEIGHT_PX));
    let Some(transition) = transition else {
        return image.into_any_element();
    };
    let from_left = centered_engine_light_left(transition.from_left, transition.from_width);
    let animation_id = ElementId::Name(
        format!(
            "artisan-native-model-engine-light-{}",
            transition.generation
        )
        .into(),
    );
    image
        .left(px(from_left))
        .with_animation(
            animation_id,
            Animation::new(std::time::Duration::from_millis(250))
                .with_easing(engine_light_smooth_out),
            move |image, progress| {
                image.left(px(from_left + ((target_left - from_left) * progress)))
            },
        )
        .into_any_element()
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "engine-light geometry is computed in f64 from GPUI bounds and painted as f32 pixels"
)]
fn centered_engine_light_left(tab_left: f64, tab_width: f64) -> f32 {
    let left = tab_left + ((tab_width - f64::from(ENGINE_LIGHT_WIDTH_PX)) / 2.0).max(0.0);
    left as f32
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
pub(super) fn engine_light_smooth_out(progress: f32) -> f32 {
    MotionCurve::SmoothOut.sample(f64::from(progress)) as f32
}

const ENGINE_LIGHT_WIDTH_PX: f32 = 32.0;
const ENGINE_LIGHT_HEIGHT_PX: f32 = 24.0;
const ENGINE_LIGHT_RASTER_SCALE: u32 = 4;
const ENGINE_LIGHT_RASTER_WIDTH: u32 = 128;
const ENGINE_LIGHT_RASTER_HEIGHT: u32 = 96;

thread_local! {
    static ENGINE_LIGHT_IMAGES: RefCell<[Option<Arc<RenderImage>>; 2]> = const { RefCell::new([None, None]) };
}

fn engine_light_image(theme: &ArtisanTheme) -> Arc<RenderImage> {
    ENGINE_LIGHT_IMAGES.with(|images| {
        let mut images = images.borrow_mut();
        let slot = match theme.mode {
            ThemeMode::Light => 0,
            ThemeMode::Dark => 1,
        };
        images[slot]
            .get_or_insert_with(|| Arc::new(RenderImage::new(vec![engine_light_frame(theme)])))
            .clone()
    })
}

#[allow(clippy::cast_precision_loss)]
fn engine_light_frame(theme: &ArtisanTheme) -> image::Frame {
    let foreground = theme.colors.foreground.to_srgb();
    let red = color_channel_to_byte(foreground.r);
    let green = color_channel_to_byte(foreground.g);
    let blue = color_channel_to_byte(foreground.b);
    // CSS filters run before the mask. Bake the 2px Gaussian into the cached
    // alpha image, with transparent padding for the filter's edge samples.
    let padding = 6 * ENGINE_LIGHT_RASTER_SCALE;
    let gradient = image::GrayImage::from_fn(
        ENGINE_LIGHT_RASTER_WIDTH + 2 * padding,
        ENGINE_LIGHT_RASTER_HEIGHT + 2 * padding,
        |x, y| {
            let inside = x >= padding
                && x < padding + ENGINE_LIGHT_RASTER_WIDTH
                && y >= padding
                && y < padding + ENGINE_LIGHT_RASTER_HEIGHT;
            let alpha = if inside {
                interpolate_profile(
                    (y - padding) as f32 / ENGINE_LIGHT_RASTER_HEIGHT as f32,
                    &[(0.0, 0.32), (0.26, 0.10), (0.52, 0.02), (0.74, 0.0)],
                )
            } else {
                0.0
            };
            image::Luma([color_channel_to_byte(alpha)])
        },
    );
    let blurred = image::imageops::blur(&gradient, 2.0 * ENGINE_LIGHT_RASTER_SCALE as f32);
    let buffer = image::ImageBuffer::from_fn(
        ENGINE_LIGHT_RASTER_WIDTH,
        ENGINE_LIGHT_RASTER_HEIGHT,
        |x, y| {
            let nx = (x as f32 + 0.5) / ENGINE_LIGHT_RASTER_WIDTH as f32;
            let ny = (y as f32 + 0.5) / ENGINE_LIGHT_RASTER_HEIGHT as f32;
            let distance = (((nx - 0.5) / 0.48).powi(2) + ((ny - 0.35) / 0.70).powi(2)).sqrt();
            let mask = interpolate_profile(
                distance,
                &[(0.0, 1.0), (0.42, 0.5), (0.68, 0.1), (0.88, 0.0)],
            );
            let alpha = f32::from(blurred.get_pixel(x + padding, y + padding)[0]) / 255.0;
            // RenderImage consumes BGRA, matching Image::to_image_data.
            image::Rgba([blue, green, red, color_channel_to_byte(alpha * mask)])
        },
    );
    image::Frame::new(buffer)
}

fn interpolate_profile(value: f32, stops: &[(f32, f32)]) -> f32 {
    let Some(&(first_position, first_alpha)) = stops.first() else {
        return 0.0;
    };
    if value <= first_position {
        return first_alpha;
    }
    for window in stops.windows(2) {
        let [(start_position, start_alpha), (end_position, end_alpha)] = window else {
            unreachable!("a two-item profile window is guaranteed by windows(2)");
        };
        if value <= *end_position {
            let span = *end_position - *start_position;
            if span <= f32::EPSILON {
                return *end_alpha;
            }
            let progress = (value - *start_position) / span;
            return start_alpha + ((*end_alpha - *start_alpha) * progress);
        }
    }
    stops.last().map_or(0.0, |(_, alpha)| *alpha)
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn color_channel_to_byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Paints one trigger-label token; gradient tokens recolor their own glyphs.
fn render_trigger_label_token(muted: Hsla, token: &NativeModelLabelToken) -> AnyElement {
    let text = div().truncate().text_size(px(14.0)).line_height(px(20.0));
    match token.role {
        NativeModelLabelRole::Name => text.child(token.text.clone()).into_any_element(),
        NativeModelLabelRole::Detail => text
            .text_color(muted)
            .child(token.text.clone())
            .into_any_element(),
        NativeModelLabelRole::Gradient(gradient) => text
            .child(gradient_label_text(token.text.clone(), gradient))
            .into_any_element(),
    }
}

/// Builds one static styled text whose glyphs interpolate `gradient`.
fn gradient_label_text(text: String, gradient: SpeedGradient) -> StyledText {
    let highlights = gradient_highlights(&text, gradient);
    StyledText::new(text).with_highlights(highlights)
}

/// Builds one contiguous character-sized highlight range per glyph.
///
/// The first character receives the gradient start and the last receives its
/// end; a single-character label keeps the start colour. Ranges are exact
/// UTF-8 byte ranges and cover the whole text without gaps.
#[allow(clippy::cast_precision_loss)]
pub(in crate::native_model_selector) fn gradient_highlights(
    text: &str,
    gradient: SpeedGradient,
) -> Vec<(Range<usize>, HighlightStyle)> {
    let count = text.chars().count();
    if count == 0 {
        return Vec::new();
    }
    let mut highlights = Vec::with_capacity(count);
    let mut start = 0;
    for (index, character) in text.chars().enumerate() {
        let end = start + character.len_utf8();
        let progress = if count > 1 {
            index as f32 / (count - 1) as f32
        } else {
            0.0
        };
        highlights.push((
            start..end,
            HighlightStyle {
                color: Some(rgb_to_hsla(rgb(gradient.color_at(progress)))),
                ..HighlightStyle::default()
            },
        ));
        start = end;
    }
    highlights
}

pub(crate) fn engine_asset(engine_id: &str) -> AssetId {
    match engine_id {
        "codex" => AssetId::SVGL_OPENAI,
        "claude" => AssetId::SVGL_CLAUDE_AI,
        "cursor" => AssetId::SVGL_CURSOR,
        "grok" => AssetId::SVGL_GROK,
        "opencode2" => AssetId::BRANDS_OPENCODE,
        _ => AssetId::TABLER_QUESTION_MARK,
    }
}

/// Provider accent for one engine's usage meter, mirroring the accent table
/// in `lib/engine/presentation.ts`. Unknown engines have no accent and fall
/// back to the muted surface paint at the call site.
#[must_use]
pub(crate) fn engine_accent(engine_id: &str) -> Option<u32> {
    match engine_id {
        "claude" => Some(0x00d9_7757),
        "codex" => Some(0x0010_a37f),
        "cursor" | "grok" | "opencode2" => Some(0x006b_7280),
        _ => None,
    }
}
