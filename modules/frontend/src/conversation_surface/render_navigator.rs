//! Loaded-turn navigator rail rendering for [`ConversationSurface`].
//!
//! Extracted verbatim from `conversation_surface.rs` during the phase-2 module
//! split; visibility was widened to `pub(super)` for parent-owned methods.
//! Paints the capped per-turn marker list and its shared hover pill.

use super::*;

impl ConversationSurface {
    /// Prunes navigator focus handles and paints the loaded-turn rail.
    ///
    /// Pruning runs on every render, even when the replacement scene has no
    /// markers at all, so a focused control that disappears returns focus to
    /// the transcript. The rail itself paints only for two or more loaded
    /// user-message markers. Emitting a scroll intent never touches the GPUI
    /// handle or scene directly.
    /// Paints the shared hover pill for navigator rows.
    ///
    /// Row probes measure into the picker's [`SlidingHoverState`]; the pill
    /// paints the retained flight behind the rows and settles interrupted
    /// flights from the currently displayed rectangle, exactly like the
    /// model picker surface. Reduced motion jumps to the target.
    pub(super) fn render_navigator_hover_pill(
        &self,
        theme: &ArtisanTheme,
        reduce_motion: bool,
    ) -> AnyElement {
        let (mut rect, visible, transition) = {
            let hover = self.navigator_hover.borrow();
            (hover.visual_rect(), hover.visible(), hover.transition())
        };
        if reduce_motion && let Some(transition) = transition {
            rect = transition.to;
            self.navigator_hover
                .borrow_mut()
                .apply_progress(transition.generation, 1.0);
        }
        let pill = div()
            .id(SharedString::from(TURN_NAVIGATOR_HOVER_SELECTOR))
            .absolute()
            .left(px(rect.left))
            .top(px(rect.top))
            .w(px(rect.width))
            .h(px(rect.height))
            .rounded(RadiusTokens::value(RadiusStep::Lg))
            .bg(hover_fill_gradient(*theme))
            .opacity(if visible { 1.0 } else { 0.0 });
        if !reduce_motion && let Some(transition) = transition {
            let motion = Rc::clone(&self.navigator_hover);
            let from = transition.from;
            let to = transition.to;
            let generation = transition.generation;
            let animation_id = ElementId::Name(SharedString::from(format!(
                "{TURN_NAVIGATOR_HOVER_SELECTOR}-{generation}"
            )));
            return pill
                .with_animation(
                    animation_id,
                    Animation::new(Duration::from_millis(250)).with_easing(navigator_smooth_out),
                    move |pill, progress| {
                        let rect = from.lerp(to, progress);
                        motion.borrow_mut().apply_progress(generation, progress);
                        pill.left(px(rect.left))
                            .top(px(rect.top))
                            .w(px(rect.width))
                            .h(px(rect.height))
                    },
                )
                .into_any_element();
        }
        pill.into_any_element()
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI builder composes the rail markers, active pill, and wheel/click wiring that share window-local metrics"
    )]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "navigator geometry is measured in f64 from GPUI bounds and painted as f32 pixels"
    )]
    pub(super) fn render_turn_navigator(
        &mut self,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        window: &mut Window,
        cx: &mut Context<Self>,
        navigator_active: Option<&str>,
    ) -> Option<AnyElement> {
        // Reused scene-derived cache: markers only change when the accepted
        // scene is replaced, so a render never re-walks the transcript to
        // rebuild navigator labels.
        let markers = Rc::clone(&self.navigator_markers);
        // Window-local rail geometry, measured live: two windows sharing one
        // surface center independently, exactly like end-space height.
        let navigator_metrics = window.use_state(cx, |_, _| TurnNavigatorMetrics::default());
        let metrics = *navigator_metrics.read(cx);
        // Prune focus handles whose targets left the scene on every render,
        // even when the replacement scene has no markers at all. A focused
        // control that disappears returns focus to the transcript.
        let live: Vec<String> = markers
            .iter()
            .map(|marker| navigator_focus_key(&marker.target))
            .collect();
        let stale: Vec<String> = self
            .navigator_focus
            .keys()
            .filter(|key| !live.contains(*key))
            .cloned()
            .collect();
        for key in stale {
            if let Some(handle) = self.navigator_focus.remove(&key)
                && handle.is_focused(window)
            {
                self.transcript_focus.focus(window, cx);
            }
        }
        if markers.is_empty() {
            self.navigator_expanded = false;
            return None;
        }
        // Handles are ensured before any focus observation below, so a
        // keyboard arrival can expand the rail into labels exactly like rail
        // hover does in the reference (`group-focus-within`).
        for marker in markers.iter() {
            let key = navigator_focus_key(&marker.target);
            self.navigator_focus
                .entry(key)
                .or_insert_with(|| cx.focus_handle().tab_stop(true));
        }
        let focus_expanded = markers.iter().any(|marker| {
            self.navigator_focus
                .get(&navigator_focus_key(&marker.target))
                .is_some_and(|handle| handle.is_focused(window))
        });
        let expanded = self.navigator_expanded || focus_expanded;
        // --inspector-width: clamp(16rem, 25vw, 350px) (theme.css:176): the
        // expanded panel matches the inspector facing it across the
        // transcript, capped to the card actually available.
        let card_width = f64::from(self.scroll_handle.bounds().size.width);
        let mut inspector_width =
            (f64::from(window.bounds().size.width) * 0.25).clamp(256.0, 350.0);
        if card_width > 0.0 {
            inspector_width = inspector_width.min(card_width);
        }
        let inspector_width = inspector_width as f32;
        // Keyboard arrival selects the pill exactly like pointer hover
        // does (`onfocusin`); the guard selects only on arrival so a later
        // trigger-zone clear survives while the row keeps focus. Render is
        // already painting, so neither path notifies.
        if let Some(focused) = markers.iter().find(|marker| {
            self.navigator_focus
                .get(&navigator_focus_key(&marker.target))
                .is_some_and(|handle| handle.is_focused(window))
        }) {
            let slug = navigator_target_slug(&focused.target).to_owned();
            if self.navigator_focused_key.as_deref() != Some(slug.as_str()) {
                self.navigator_focused_key = Some(slug.clone());
                self.navigator_hover.borrow_mut().set_active(slug);
            }
        } else {
            self.navigator_focused_key = None;
            if !expanded && self.navigator_hover.borrow().visible() {
                self.navigator_hover.borrow_mut().hide();
            }
        }
        // The lit marker is the turn the reader is at, tracked from painted
        // geometry against the reference 96 px threshold — never the tail.
        let navigator_surface = entity.downgrade();
        // One control per marker in every state, exactly like the reference:
        // hovering reveals the labels of controls that were already there
        // rather than mounting new ones, so pointer and keyboard activation
        // can never race a remount.
        // Plain Div until the metrics listener below: `on_children_prepainted`
        // is a Div inherent, so identity, scrolling, and the width clock
        // attach at the tail. The list is the pill's positioning context.
        let mut list = div()
            .relative()
            .flex()
            .flex_col()
            .items_end()
            .gap(theme.spacing.steps(1.0))
            .p(px(8.0))
            // Hover is evaluated while painting, and a mouse move alone does
            // not schedule a frame. The list is the hit target for the gaps
            // and padding between rows, so it has to refresh from its own move
            // events; rows already refresh the same way.
            .on_mouse_move(move |_: &MouseMoveEvent, window: &mut Window, _| {
                window.refresh();
            });
        if metrics.viewport_px > 0.0 {
            list = list.max_h(px(metrics.viewport_px * 0.7));
        }
        // Shared hover pill behind the rows plus the probe capturing list
        // bounds for pill-relative coordinates, mirroring the model picker.
        list = list.child(self.render_navigator_hover_pill(theme, cx.reduce_motion()));
        let hover_surface_bounds = Rc::clone(&self.navigator_hover_surface);
        let list_bounds_surface = entity.downgrade();
        // Height-change detection is window-local: two windows share one
        // surface, and a shared last-measured value would make them notify
        // each other about each other's layout.
        let last_probe_height = window.use_state(cx, |_, _| None::<gpui::Pixels>);
        list = list.child(
            canvas(
                |_, _, _| {},
                move |bounds, (), window, app| {
                    // The metrics listener reads this probe on the next frame,
                    // so a height change here has to schedule that frame too:
                    // otherwise an expanded rail keeps its collapsed top
                    // offset until some unrelated repaint.
                    let changed = last_probe_height.update(app, |height, _| {
                        let changed = *height != Some(bounds.size.height);
                        *height = Some(bounds.size.height);
                        changed
                    });
                    *hover_surface_bounds.borrow_mut() = Some(bounds);
                    if changed {
                        let _ = list_bounds_surface.update(app, |_, cx| cx.notify());
                        window.defer(app, |window, _| window.refresh());
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        );
        for marker in markers.iter() {
            let key = navigator_focus_key(&marker.target);
            let Some(handle) = self.navigator_focus.get(&key).cloned() else {
                continue;
            };
            let slug = navigator_target_slug(&marker.target);
            let control_selector = format!("{TURN_NAVIGATOR_CONTROL_PREFIX}-{slug}");
            let label_selector = format!("{control_selector}-label");
            let tick_selector = format!("{control_selector}-tick");
            let active = navigator_active == Some(slug);
            let click_surface = navigator_surface.clone();
            let click_target = marker.target.clone();
            let probe_hover = Rc::clone(&self.navigator_hover);
            let probe_surface = Rc::clone(&self.navigator_hover_surface);
            let probe_id = slug.to_owned();
            let move_hover = Rc::clone(&self.navigator_hover);
            let move_surface = Rc::clone(&self.navigator_hover_surface);
            let move_id = slug.to_owned();
            let focus_ring_color = theme.interaction.focus_ring_color.to_paint();
            let focus_ring_width = theme.interaction.focus_ring_width;
            // Labels show expanded, ticks at rest: the spans swap while the
            // row itself never remounts, so pointer and keyboard activation
            // keep one stable target.
            let mut row_content_label = None;
            if expanded {
                let mut label = div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_size(theme.typography.control_text)
                    .text_color(theme.colors.foreground.to_paint())
                    .debug_selector({
                        let selector = label_selector.clone();
                        move || selector.clone()
                    })
                    .child(marker.label.clone());
                if active {
                    label = label.font_weight(FontWeight::MEDIUM);
                }
                row_content_label = Some(label);
            }
            let mut row_content_tick = None;
            if !expanded {
                row_content_tick = Some(
                    div()
                        .h(px(1.0))
                        .w(px(if active { 24.0 } else { 16.0 }))
                        .rounded_full()
                        .debug_selector({
                            let selector = tick_selector.clone();
                            move || selector.clone()
                        })
                        .bg(if active {
                            theme.colors.foreground.to_paint()
                        } else {
                            theme.colors.muted_foreground.with_alpha(0.5).to_paint()
                        }),
                );
            }
            let mut row = div()
                .id(control_selector.clone())
                .relative()
                .track_focus(&handle)
                .tab_index(0)
                .role(gpui::Role::Button)
                .aria_label(marker.label.clone())
                .cursor_pointer()
                .w_full()
                .flex()
                .flex_row()
                .items_center()
                .justify_end()
                .gap(theme.spacing.steps(3.0))
                .rounded(RadiusTokens::value(RadiusStep::Lg))
                .px(px(if expanded { 12.0 } else { 0.0 }))
                .py(px(if expanded { 6.0 } else { 0.0 }))
                .debug_selector(move || control_selector.clone())
                .focus_visible(move |focused| {
                    focused.shadow(vec![BoxShadow {
                        color: focus_ring_color,
                        offset: point(px(0.0), px(0.0)),
                        blur_radius: px(0.0),
                        spread_radius: focus_ring_width,
                        inset: false,
                    }])
                })
                // Trigger zone: the rightmost 40 px strip reveals the card,
                // so the pill belongs to the label area — a pointer over the
                // strip clears it instead of highlighting a tick row of its
                // own. Keyboard focus selects through render instead.
                .on_mouse_move(move |event: &MouseMoveEvent, window, _cx| {
                    let over_strip = move_surface.borrow().as_ref().is_some_and(|surface| {
                        f64::from(event.position.x) >= f64::from(surface.right()) - 40.0
                    });
                    if over_strip {
                        move_hover.borrow_mut().hide();
                    } else {
                        move_hover.borrow_mut().set_active(move_id.clone());
                    }
                    window.refresh();
                })
                // Pointer and keyboard activation share one path: the button
                // role synthesizes click from Enter/Space, so no separate key
                // handler may double the intent.
                .on_click(move |_, _, app| {
                    let _ = click_surface.update(app, |surface, cx| {
                        surface.request_scroll(click_target.clone(), cx);
                    });
                });
            if let Some(label) = row_content_label {
                row = row.child(label);
            }
            if let Some(tick) = row_content_tick {
                row = row.child(tick);
            }
            row = row.child(
                canvas(
                    |_, _, _| {},
                    move |bounds, (), window, cx| {
                        let Some(surface) = *probe_surface.borrow() else {
                            return;
                        };
                        let rect = HoverRect {
                            left: f32::from(bounds.left() - surface.left()),
                            top: f32::from(bounds.top() - surface.top()),
                            width: f32::from(bounds.size.width),
                            height: f32::from(bounds.size.height),
                        };
                        if probe_hover.borrow_mut().measure(&probe_id, rect) {
                            window.defer(cx, |window, _| window.refresh());
                        }
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            );
            list = list.child(row);
        }
        // Rail metrics converge through the same window-local discipline as
        // end space: measured per window, notified only on change.
        let metrics_state = navigator_metrics.clone();
        let metrics_hover_surface = Rc::clone(&self.navigator_hover_surface);
        list = list.on_children_prepainted(move |_children_bounds, window, app| {
            let changed = navigator_surface
                .update(app, |surface, cx| {
                    let viewport = f64::from(surface.scroll_handle.bounds().size.height);
                    // Center the actually painted list box, read from its probe:
                    // row spans miscount once absolute overlays (pill, probe,
                    // glass) join the children.
                    let guard = metrics_hover_surface.borrow();
                    let Some(list_bounds) = guard.as_ref() else {
                        return false;
                    };
                    let height = f64::from(list_bounds.size.height);
                    // The capped list is what actually paints: clamp the measured
                    // height to the 70 % cap before centering, or a long list
                    // pins its top at zero instead of centering the cap.
                    let capped = if viewport > 0.0 {
                        height.min(viewport * 0.7)
                    } else {
                        height
                    };
                    let next = TurnNavigatorMetrics {
                        top_px: (((viewport - capped) / 2.0).max(0.0)) as f32,
                        viewport_px: viewport as f32,
                    };
                    let changed = metrics_state.update(cx, |metrics, _| {
                        #[expect(
                            clippy::float_cmp,
                            reason = "the loop guard treats any geometry change as a reason to notify; an epsilon would skip sub-pixel navigator re-layout"
                        )]
                        let changed = metrics.top_px != next.top_px
                            || metrics.viewport_px != next.viewport_px;
                        *metrics = next;
                        changed
                    });
                    if changed {
                        cx.notify();
                    }
                    changed
                })
                .unwrap_or(false);
            if changed {
                // A notification raised inside prepaint does not itself
                // request a frame: without this the rail keeps its previous
                // top until some unrelated repaint, which is exactly what a
                // reduced-motion reveal would otherwise show.
                window.defer(app, |window, _| window.refresh());
            }
        });
        // Identity, scrolling, and width attach after the Div-phase
        // listener above: the rail owns gestures over its own rows, applying
        // the bounded immediate scroll locally (the reference plain scroller
        // has no smoothing) and containing the event so the transcript does
        // not scroll underneath. Width motion replays per hover generation,
        // never on mount: generation zero paints the static width outright
        // (open 250 ms, close 150 ms on the dropdown curve).
        let navigator_scroll_handle = self.navigator_scroll.clone();
        let list = list
            .id(SharedString::from(TURN_NAVIGATOR_LIST_SELECTOR))
            .overflow_y_scroll()
            .track_scroll(&self.navigator_scroll)
            .on_scroll_wheel(move |event, window, cx| {
                let delta = f32::from(event.delta.pixel_delta(window.line_height()).y);
                if delta.abs() <= f32::EPSILON {
                    return;
                }
                let offset = navigator_scroll_handle.offset();
                let maximum = f32::from(navigator_scroll_handle.max_offset().y).max(0.0);
                let next = (f32::from(offset.y) + delta).clamp(-maximum, 0.0);
                navigator_scroll_handle.set_offset(point(offset.x, px(next)));
                cx.stop_propagation();
            });
        // Width motion replays per hover generation, never on mount, and
        // never under reduced motion: generation zero paints the static
        // width outright. Reversals start from the retained painted width,
        // so an interrupted flight never jumps to a fixed endpoint. Open
        // runs the reference 250 ms, close 150 ms, both on the dropdown
        // curve.
        let width_target = if expanded { inspector_width } else { 40.0 };
        let list: AnyElement = if self.navigator_width_generation == 0 || cx.reduce_motion() {
            *self.navigator_width_px.borrow_mut() = width_target;
            list.w(px(width_target)).into_any_element()
        } else {
            let from_w = self.navigator_width_from;
            let width_state = Rc::clone(&self.navigator_width_px);
            let generation = self.navigator_width_generation;
            let duration_ms = if expanded { 250 } else { 150 };
            list.w(px(from_w))
                .with_animation(
                    ElementId::Name(SharedString::from(format!(
                        "{TURN_NAVIGATOR_SELECTOR}-width-{generation}"
                    ))),
                    Animation::new(Duration::from_millis(duration_ms))
                        .with_easing(navigator_smooth_out),
                    move |list, progress| {
                        let width = from_w + (width_target - from_w) * progress;
                        *width_state.borrow_mut() = width;
                        list.w(px(width))
                    },
                )
                .into_any_element()
        };
        let hover_surface = entity.downgrade();
        // Middle of the card, not the prose column: the reference anchors
        // the rail at `top-1/2 right-2` with a 70 % height cap. Centering
        // has no translate primitive here, so the top offset converges from
        // the measured list height in window-local state.
        let mut rail = div()
            .id(SharedString::from(TURN_NAVIGATOR_SELECTOR))
            .absolute()
            .right(px(8.0))
            .top(px(metrics.top_px))
            .debug_selector(|| TURN_NAVIGATOR_SELECTOR.to_owned())
            .on_hover(move |hovered: &bool, window, app| {
                let _ = hover_surface.update(app, |surface, cx| {
                    let mut changed = false;
                    if surface.navigator_expanded != *hovered {
                        surface.navigator_expanded = *hovered;
                        surface.navigator_width_from = *surface.navigator_width_px.borrow();
                        surface.navigator_width_generation =
                            surface.navigator_width_generation.wrapping_add(1);
                        changed = true;
                    }
                    // The pill belongs to the expanded area: leaving the rail
                    // hides it unless a row keeps keyboard focus, which holds
                    // the labels open on its own.
                    if !*hovered {
                        let any_focused = surface
                            .navigator_focus
                            .values()
                            .any(|handle| handle.is_focused(window));
                        if !any_focused && surface.navigator_hover.borrow().visible() {
                            surface.navigator_hover.borrow_mut().hide();
                            changed = true;
                        }
                    }
                    if changed {
                        cx.notify();
                    }
                });
            });
        // Expanded glass card behind the labels, composed from the shipped
        // picker/composer material primitives: backdrop blur, foreground
        // base, material and highlight layers, and the card shadow. The
        // collapsed tick strip stays unpainted and click-through.
        if expanded {
            let radius = RadiusTokens::value(RadiusStep::Xl);
            rail = rail
                .rounded(radius)
                .backdrop_blur(glass_blur_radius(GlassStrength::Strong))
                .bg(glass_foreground_base(theme))
                .shadow(glass_card_shadows())
                .child(glass_material_layer(GlassStrength::Strong, radius))
                .child(glass_highlight_layer(GlassStrength::Strong, radius));
        }
        rail = rail.child(list);
        Some(rail.into_any_element())
    }
}
