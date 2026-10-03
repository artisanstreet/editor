//! Loaded-turn navigator rendering for [`ConversationSurface`].
//!
//! The navigator is two surfaces. The rail is a column of ticks, one per
//! loaded user message, fixed at the transcript's right edge; it never
//! changes size or position. Hovering the rail, or focusing one of its
//! ticks, opens the menu: a floating glass card beside the rail that lists
//! the same messages by their text.
//!
//! The menu is a real floating layer (deferred and anchored in window
//! coordinates), so opening it moves nothing: the ticks stay where they are
//! and stay visible. Its geometry is computed from the marker count and the
//! transcript viewport instead of measured after paint, so it opens at its
//! final place and size and holds them while the pointer travels its rows.

use super::*;

/// Width of the tick rail (`w-10`).
const NAVIGATOR_RAIL_WIDTH_PX: f32 = 40.0;
/// Inset of the rail from the transcript's right edge (`right-2`).
const NAVIGATOR_RAIL_INSET_PX: f32 = 8.0;
/// Padding inside the rail and inside the menu list (`p-2`).
const NAVIGATOR_PAD_PX: f32 = 8.0;
/// Vertical pitch of one tick: a 1 px line centered in a 5 px hit row, so
/// the rail has no dead gaps between its targets.
const NAVIGATOR_TICK_PITCH_PX: f32 = 5.0;
/// Width of the menu, before the transcript column bounds it.
const NAVIGATOR_MENU_WIDTH_PX: f32 = 320.0;
/// Gap between the menu and the rail. The menu's hover region spans it, so
/// the pointer crosses from the rail into the menu without closing it.
const NAVIGATOR_MENU_GAP_PX: f32 = 4.0;
/// Height of one menu row.
const NAVIGATOR_MENU_ROW_PX: f32 = 32.0;
/// Gap between menu rows (`gap-1`).
const NAVIGATOR_MENU_ROW_GAP_PX: f32 = 4.0;
/// Space kept between the menu and the transcript column's left edge, and
/// between the menu and every window edge.
const NAVIGATOR_MENU_MARGIN_PX: f32 = 8.0;
/// Share of the transcript viewport height the rail and the menu may take.
const NAVIGATOR_HEIGHT_CAP: f32 = 0.7;
/// Entrance of the menu: a short fade and a 4 px settle toward the rail.
const NAVIGATOR_MENU_ENTRANCE_MS: u64 = 150;
const NAVIGATOR_MENU_ENTRANCE_SHIFT_PX: f32 = 4.0;

/// Which navigator surface a pointer-hover change belongs to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NavigatorHoverZone {
    /// The tick rail.
    Rail,
    /// The floating menu, including the gap between it and the rail.
    Menu,
}

/// Caps a navigator surface to its share of the transcript viewport.
fn navigator_capped_height(content_px: f32, viewport_px: f32) -> f32 {
    if viewport_px > 0.0 {
        content_px.min(viewport_px * NAVIGATOR_HEIGHT_CAP)
    } else {
        content_px
    }
}

impl ConversationSurface {
    /// Records a pointer-hover change on one navigator surface, returning
    /// whether anything changed.
    ///
    /// The menu stays open while the pointer is over the rail or the menu.
    /// Once it is over neither, the hover pill hides unless a tick keeps
    /// keyboard focus, which holds the menu open on its own.
    fn set_navigator_hover(
        &mut self,
        zone: NavigatorHoverZone,
        hovered: bool,
        window: &Window,
    ) -> bool {
        let flag = match zone {
            NavigatorHoverZone::Rail => &mut self.navigator_rail_hovered,
            NavigatorHoverZone::Menu => &mut self.navigator_menu_hovered,
        };
        if *flag == hovered {
            return false;
        }
        *flag = hovered;
        if !self.navigator_rail_hovered && !self.navigator_menu_hovered {
            let any_focused = self
                .navigator_focus
                .values()
                .any(|handle| handle.is_focused(window));
            if !any_focused && self.navigator_hover.borrow().visible() {
                self.navigator_hover.borrow_mut().hide();
            }
        }
        true
    }

    /// Paints the shared hover pill for navigator menu rows.
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

    /// Prunes navigator focus handles and paints the loaded-turn navigator:
    /// the tick rail, plus the floating menu while it is open.
    ///
    /// Pruning runs on every render, even when the replacement scene has no
    /// markers at all, so a focused control that disappears returns focus to
    /// the transcript. The navigator paints only for two or more loaded
    /// user-message markers. Emitting a scroll intent never touches the GPUI
    /// handle or scene directly.
    ///
    /// The ticks are the navigator's controls: one per marker, keyboard
    /// focusable, never remounted. The menu rows are pointer targets for the
    /// same markers and carry their labels.
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI builder composes the rail, the menu, and the hover/click wiring that share window-local metrics"
    )]
    #[expect(
        clippy::cast_precision_loss,
        reason = "marker counts are bounded by the scene and far below f32's exact integer range"
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
        // Window-local viewport geometry, measured live: two windows sharing
        // one surface center independently, exactly like end-space height.
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
            self.navigator_rail_hovered = false;
            self.navigator_menu_hovered = false;
            return None;
        }
        // Handles are ensured before any focus observation below, so a
        // keyboard arrival opens the menu exactly like rail hover does in
        // the reference (`group-focus-within`).
        for marker in markers.iter() {
            let key = navigator_focus_key(&marker.target);
            self.navigator_focus
                .entry(key)
                .or_insert_with(|| cx.focus_handle().tab_stop(true));
        }
        let focused = markers.iter().find(|marker| {
            self.navigator_focus
                .get(&navigator_focus_key(&marker.target))
                .is_some_and(|handle| handle.is_focused(window))
        });
        let open = self.navigator_rail_hovered || self.navigator_menu_hovered || focused.is_some();
        // Keyboard arrival selects the pill exactly like pointer hover
        // does (`onfocusin`); the guard selects only on arrival so a later
        // pointer move survives while the tick keeps focus. Render is
        // already painting, so neither path notifies.
        if let Some(focused) = focused {
            let slug = navigator_target_slug(&focused.target).to_owned();
            if self.navigator_focused_key.as_deref() != Some(slug.as_str()) {
                self.navigator_focused_key = Some(slug.clone());
                self.navigator_hover.borrow_mut().set_active(slug);
            }
        } else {
            self.navigator_focused_key = None;
            if !open && self.navigator_hover.borrow().visible() {
                self.navigator_hover.borrow_mut().hide();
            }
        }

        let viewport = metrics.viewport;
        let viewport_height = f32::from(viewport.size.height);
        let count = markers.len() as f32;
        let navigator_surface = entity.downgrade();

        // The rail: one tick per marker, in a column whose size depends only
        // on the marker count and the viewport cap. Opening the menu changes
        // nothing here.
        let rail_height = navigator_capped_height(
            NAVIGATOR_PAD_PX * 2.0 + count * NAVIGATOR_TICK_PITCH_PX,
            viewport_height,
        );
        let mut ticks = div()
            .flex()
            .flex_col()
            .items_end()
            .w(px(NAVIGATOR_RAIL_WIDTH_PX))
            .h(px(rail_height))
            .p(px(NAVIGATOR_PAD_PX))
            .overflow_hidden();
        for marker in markers.iter() {
            let key = navigator_focus_key(&marker.target);
            let Some(handle) = self.navigator_focus.get(&key).cloned() else {
                continue;
            };
            let slug = navigator_target_slug(&marker.target);
            let control_selector = format!("{TURN_NAVIGATOR_CONTROL_PREFIX}-{slug}");
            let active = navigator_active == Some(slug);
            let click_surface = navigator_surface.clone();
            let click_target = marker.target.clone();
            let focus_ring_color = theme.interaction.focus_ring_color.to_paint();
            let focus_ring_width = theme.interaction.focus_ring_width;
            ticks = ticks.child(
                div()
                    .id(control_selector.clone())
                    .track_focus(&handle)
                    .tab_index(0)
                    .role(gpui::Role::Button)
                    .aria_label(marker.label.clone())
                    .cursor_pointer()
                    .w_full()
                    .h(px(NAVIGATOR_TICK_PITCH_PX))
                    .flex_shrink_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_end()
                    .rounded(RadiusTokens::value(RadiusStep::Sm))
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
                    // Pointer and keyboard activation share one path: the
                    // button role synthesizes click from Enter/Space, so no
                    // separate key handler may double the intent.
                    .on_click(move |_, _, app| {
                        let _ = click_surface.update(app, |surface, cx| {
                            surface.request_scroll(click_target.clone(), cx);
                        });
                    })
                    .child(
                        div()
                            .h(px(1.0))
                            .w(px(if active { 24.0 } else { 16.0 }))
                            .rounded_full()
                            .debug_selector(|| {
                                format!("{TURN_NAVIGATOR_CONTROL_PREFIX}-{slug}-tick")
                            })
                            .bg(if active {
                                theme.colors.foreground.to_paint()
                            } else {
                                theme.colors.muted_foreground.with_alpha(0.5).to_paint()
                            }),
                    ),
            );
        }
        // Viewport geometry converges through the same window-local
        // discipline as end space: measured per window, notified only on
        // change. The rail is always mounted, so it carries the listener.
        let metrics_state = navigator_metrics.clone();
        let metrics_surface = navigator_surface.clone();
        let ticks = ticks.on_children_prepainted(move |_children_bounds, window, app| {
            let changed = metrics_surface
                .update(app, |surface, cx| {
                    let next = TurnNavigatorMetrics {
                        viewport: surface.scroll_handle.bounds(),
                    };
                    let changed = metrics_state.update(cx, |metrics, _| {
                        let changed = *metrics != next;
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
                // request a frame: without this the navigator keeps its
                // previous place until some unrelated repaint.
                window.defer(app, |window, _| window.refresh());
            }
        });
        // Middle of the card, not the prose column: the reference anchors
        // the rail at `top-1/2 right-2`. Centering has no translate
        // primitive here, so the top offset comes from the viewport height
        // and the rail's own computed height.
        let rail_hover_surface = navigator_surface.clone();
        let rail = div()
            .id(SharedString::from(TURN_NAVIGATOR_SELECTOR))
            .absolute()
            .right(px(NAVIGATOR_RAIL_INSET_PX))
            .top(px(((viewport_height - rail_height) / 2.0).max(0.0)))
            .debug_selector(|| TURN_NAVIGATOR_SELECTOR.to_owned())
            .on_hover(move |hovered: &bool, window, app| {
                let _ = rail_hover_surface.update(app, |surface, cx| {
                    if surface.set_navigator_hover(NavigatorHoverZone::Rail, *hovered, window) {
                        cx.notify();
                    }
                });
            })
            .child(ticks);

        // The layer spans the surface root so the rail anchors to the
        // card's right edge. It carries no listener, so it takes no input
        // of its own.
        let mut navigator = div().absolute().top_0().left_0().size_full().child(rail);
        if open
            && let Some(menu) = self.render_turn_navigator_menu(
                entity,
                theme,
                viewport,
                navigator_active,
                cx.reduce_motion(),
            )
        {
            navigator = navigator.child(menu);
        }
        Some(navigator.into_any_element())
    }

    /// Builds the floating menu for an open navigator.
    ///
    /// The card sits to the left of the rail with its right edge
    /// [`NAVIGATOR_MENU_GAP_PX`] from the rail, centered on the transcript
    /// viewport like the rail. Its width is bounded by the transcript
    /// column, so it never reaches the rules of the sidebars beside the
    /// column, and the window margin keeps it clear of the window edges.
    /// Returns `None` until the viewport has been measured.
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI builder composes the menu rows, hover pill, probes, and placement in visual order"
    )]
    #[expect(
        clippy::cast_precision_loss,
        reason = "marker counts are bounded by the scene and far below f32's exact integer range"
    )]
    fn render_turn_navigator_menu(
        &self,
        entity: &Entity<Self>,
        theme: &ArtisanTheme,
        viewport: gpui::Bounds<gpui::Pixels>,
        navigator_active: Option<&str>,
        reduce_motion: bool,
    ) -> Option<AnyElement> {
        let viewport_width = f32::from(viewport.size.width);
        let viewport_height = f32::from(viewport.size.height);
        let width = NAVIGATOR_MENU_WIDTH_PX.min(
            viewport_width
                - NAVIGATOR_RAIL_INSET_PX
                - NAVIGATOR_RAIL_WIDTH_PX
                - NAVIGATOR_MENU_GAP_PX
                - NAVIGATOR_MENU_MARGIN_PX,
        );
        if width <= 0.0 || viewport_height <= 0.0 {
            return None;
        }
        let markers = Rc::clone(&self.navigator_markers);
        let count = markers.len() as f32;
        let content_height = NAVIGATOR_PAD_PX * 2.0
            + count * NAVIGATOR_MENU_ROW_PX
            + (count - 1.0).max(0.0) * NAVIGATOR_MENU_ROW_GAP_PX;
        let height = navigator_capped_height(content_height, viewport_height);
        // How far the rows can travel: nothing while they all fit. The
        // handle's own maximum also counts the hover pill laid over the
        // rows, which would let a menu that fits scroll into empty space.
        let scroll_maximum = (content_height - height).max(0.0);
        let scroll_offset = self.navigator_scroll.offset();
        if f32::from(scroll_offset.y) < -scroll_maximum {
            self.navigator_scroll
                .set_offset(point(scroll_offset.x, px(-scroll_maximum)));
        }
        let navigator_surface = entity.downgrade();

        // Plain Div until the scroll wiring below. The list is the pill's
        // positioning context.
        let mut list = div()
            .relative()
            .flex()
            .flex_col()
            .gap(px(NAVIGATOR_MENU_ROW_GAP_PX))
            .p(px(NAVIGATOR_PAD_PX))
            .w_full()
            .h(px(height))
            // Hover is evaluated while painting, and a mouse move alone does
            // not schedule a frame. The list is the hit target for the gaps
            // and padding between rows, so it has to refresh from its own move
            // events; rows already refresh the same way.
            .on_mouse_move(move |_: &MouseMoveEvent, window: &mut Window, _| {
                window.refresh();
            });
        // Shared hover pill behind the rows plus the probe capturing list
        // bounds for pill-relative coordinates, mirroring the model picker.
        let rows: Vec<String> = markers
            .iter()
            .map(|marker| navigator_target_slug(&marker.target).to_owned())
            .collect();
        self.navigator_hover.borrow_mut().clear_if_missing(&rows);
        list = list.child(self.render_navigator_hover_pill(theme, reduce_motion));
        let hover_surface_bounds = Rc::clone(&self.navigator_hover_surface);
        list = list.child(
            canvas(
                |_, _, _| {},
                move |bounds, (), _, _| {
                    *hover_surface_bounds.borrow_mut() = Some(bounds);
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        );
        for marker in markers.iter() {
            let slug = navigator_target_slug(&marker.target);
            let row_selector = format!("{TURN_NAVIGATOR_CONTROL_PREFIX}-{slug}-row");
            let click_surface = navigator_surface.clone();
            let click_target = marker.target.clone();
            let probe_hover = Rc::clone(&self.navigator_hover);
            let probe_surface = Rc::clone(&self.navigator_hover_surface);
            let probe_id = slug.to_owned();
            let move_hover = Rc::clone(&self.navigator_hover);
            let move_id = slug.to_owned();
            let mut label = div()
                .min_w_0()
                .flex_1()
                .truncate()
                .text_size(theme.typography.control_text)
                .text_color(theme.colors.foreground.to_paint())
                .debug_selector(|| format!("{TURN_NAVIGATOR_CONTROL_PREFIX}-{slug}-label"))
                .child(marker.label.clone());
            if navigator_active == Some(slug) {
                label = label.font_weight(FontWeight::MEDIUM);
            }
            list = list.child(
                div()
                    .id(row_selector.clone())
                    .relative()
                    .cursor_pointer()
                    .w_full()
                    .h(px(NAVIGATOR_MENU_ROW_PX))
                    .flex_shrink_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .rounded(RadiusTokens::value(RadiusStep::Lg))
                    .px(px(12.0))
                    .debug_selector(move || row_selector.clone())
                    .on_mouse_move(move |_: &MouseMoveEvent, window, _cx| {
                        move_hover.borrow_mut().set_active(move_id.clone());
                        window.refresh();
                    })
                    .on_click(move |_, _, app| {
                        let _ = click_surface.update(app, |surface, cx| {
                            surface.request_scroll(click_target.clone(), cx);
                        });
                    })
                    .child(label)
                    .child(
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
                    ),
            );
        }
        // The menu owns gestures over its own rows, applying the bounded
        // immediate scroll locally (the reference plain scroller has no
        // smoothing) and containing the event so the transcript does not
        // scroll underneath.
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
                let next = (f32::from(offset.y) + delta).clamp(-scroll_maximum, 0.0);
                navigator_scroll_handle.set_offset(point(offset.x, px(next)));
                cx.stop_propagation();
            });

        // Glass card composed from the shipped picker/composer material
        // primitives: backdrop blur, foreground base, material and highlight
        // layers, and the card shadow. A press on the card never reaches the
        // transcript beneath it, so it cannot start a text selection there.
        let radius = RadiusTokens::value(RadiusStep::Xl);
        let card = div()
            .id(SharedString::from(TURN_NAVIGATOR_MENU_SELECTOR))
            .relative()
            .w(px(width))
            .rounded(radius)
            .backdrop_blur(glass_blur_radius(GlassStrength::Strong))
            .bg(glass_foreground_base(theme))
            .shadow(glass_card_shadows())
            .debug_selector(|| TURN_NAVIGATOR_MENU_SELECTOR.to_owned())
            .on_mouse_down(gpui::MouseButton::Left, |_, _, app| {
                app.stop_propagation();
            })
            .child(glass_material_layer(GlassStrength::Strong, radius))
            .child(glass_highlight_layer(GlassStrength::Strong, radius))
            .child(list);
        let card: AnyElement = if reduce_motion {
            card.into_any_element()
        } else {
            card.opacity(0.0)
                .left(px(-NAVIGATOR_MENU_ENTRANCE_SHIFT_PX))
                .with_animation(
                    ElementId::Name(SharedString::from(format!(
                        "{TURN_NAVIGATOR_MENU_SELECTOR}-entrance"
                    ))),
                    Animation::new(Duration::from_millis(NAVIGATOR_MENU_ENTRANCE_MS))
                        .with_easing(navigator_smooth_out),
                    |card, progress| {
                        card.opacity(progress)
                            .left(px(-NAVIGATOR_MENU_ENTRANCE_SHIFT_PX * (1.0 - progress)))
                    },
                )
                .into_any_element()
        };

        // The hover region is the card plus the gap to the rail: its right
        // edge meets the rail's left edge, so the pointer is over the rail
        // or over this region all the way across.
        let menu_hover_surface = navigator_surface;
        let region = div()
            .id(SharedString::from(format!(
                "{TURN_NAVIGATOR_MENU_SELECTOR}-region"
            )))
            .pr(px(NAVIGATOR_MENU_GAP_PX))
            .on_hover(move |hovered: &bool, window, app| {
                let _ = menu_hover_surface.update(app, |surface, cx| {
                    if surface.set_navigator_hover(NavigatorHoverZone::Menu, *hovered, window) {
                        cx.notify();
                    }
                });
            })
            .child(card);
        let rail_left =
            viewport.right() - px(NAVIGATOR_RAIL_INSET_PX) - px(NAVIGATOR_RAIL_WIDTH_PX);
        let top = viewport.top() + px(((viewport_height - height) / 2.0).max(0.0));
        Some(
            anchored()
                .anchor(gpui::Anchor::TopRight)
                .position(point(rail_left, top))
                .snap_to_window_with_margin(px(NAVIGATOR_MENU_MARGIN_PX))
                .child(region)
                .into_any_element(),
        )
    }
}
