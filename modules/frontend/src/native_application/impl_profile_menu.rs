//! Profile menu hover, motion, usage rendering, and route-title helpers for [`NativeApplication`].
//!
//! Extracted verbatim from `native_application.rs` during the phase-2 module
//! split; visibility was widened to `pub(super)` for parent-owned methods.

use super::*;

impl NativeApplication {
    pub(super) fn profile_hover_id_for_index(index: usize) -> Option<String> {
        match index {
            0 => Some(PROFILE_SETTINGS_HOVER_ID.to_owned()),
            1 => Some(PROFILE_USAGE_HOVER_ID.to_owned()),
            _ => None,
        }
    }

    pub(super) fn set_profile_highlight(&mut self, index: usize) {
        if index == 0 {
            let _ = self.profile_menu.move_first();
        } else {
            let _ = self.profile_menu.move_last();
        }
    }

    pub(super) fn sync_profile_hover_to_highlight(&self) {
        if let Some(index) = self.profile_menu.highlighted_index()
            && let Some(id) = Self::profile_hover_id_for_index(index)
        {
            self.profile_hover.borrow_mut().set_active(id);
            self.profile_hover_keyboard.set(true);
        }
    }

    /// Returns the stable keyboard focus for one refresh control,
    /// creating it on first paint. Handles are pruned with the visible
    /// provider list in [`Self::desktop_profile_usage`].
    pub(super) fn profile_refresh_focus_handle(
        &self,
        engine_id: &str,
        cx: &Context<Self>,
    ) -> FocusHandle {
        if let Some(handle) = self
            .profile_refresh_focus
            .borrow()
            .iter()
            .find(|(id, _)| id == engine_id)
            .map(|(_, handle)| handle.clone())
        {
            return handle;
        }
        let handle = cx.focus_handle();
        self.profile_refresh_focus
            .borrow_mut()
            .push((engine_id.to_owned(), handle.clone()));
        handle
    }

    /// Source `bg-border/50` for dropdown separators: the shared border
    /// token with its original alpha multiplied by one half rather than
    /// overridden, so an already-translucent border stays proportional.
    pub(super) fn profile_separator_paint(&self) -> gpui::Hsla {
        let border = self.theme.colors.border;
        border.with_alpha(border.a * 0.5).to_paint()
    }

    pub(super) fn clear_profile_hover(&self) {
        if self.profile_hover.borrow().visible() {
            self.profile_hover.borrow_mut().clear();
        }
        self.profile_hover_surface_bounds.borrow_mut().take();
        self.profile_hover_keyboard.set(false);
        self.profile_meter_hover.borrow_mut().take();
        self.profile_tip_surface_bounds.borrow_mut().take();
        self.profile_tip_anchor.borrow_mut().take();
        *self.profile_tip_tween.borrow_mut() = ProfileTipTween::default();
        self.profile_refresh_swap.borrow_mut().clear();
        self.profile_swap_frame_scheduled.set(false);
    }

    /// Maximum height for the scrollable usage area: the natural content
    /// height wins until the panel would outgrow the space above the
    /// trigger, keeping a small viewport margin. The fixed header, divider,
    /// action rows, and panel padding are subtracted so only the usage
    /// area scrolls. Short windows clamp at zero instead of overflowing.
    pub(super) fn profile_usage_max_height(&self, window: &Window) -> gpui::Pixels {
        let viewport_height = f32::from(window.bounds().size.height);
        let trigger_top = f32::from(self.profile_origin.get().top()).min(viewport_height);
        let available = trigger_top - PROFILE_MENU_ANCHOR_GAP_PX - PROFILE_MENU_VIEWPORT_MARGIN_PX;
        px((available - PROFILE_MENU_FIXED_CHROME_PX).max(0.0))
    }

    pub(super) fn handle_profile_usage_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Same contract as the model picker: this wrapper is the first
        // bubble listener inside the scroll container, so consuming the
        // wheel here keeps GPUI from applying its default offset update a
        // second time.
        cx.stop_propagation();
        if !self.profile_menu_is_interactive() {
            return;
        }
        // A scrolling list must not keep a meter tooltip pinned to a stale
        // row geometry.
        self.profile_meter_hover.borrow_mut().take();
        self.profile_tip_anchor.borrow_mut().take();
        let delta = event.delta.pixel_delta(window.line_height()).y;
        let delta = f32::from(delta);
        if delta.abs() <= f32::EPSILON {
            return;
        }
        let handle = self.profile_usage_scroll.clone();
        let offset = handle.offset();
        let current = f32::from(offset.y);
        let maximum = f32::from(handle.max_offset().y).max(0.0);
        if event.delta.precise() || cx.reduce_motion() {
            let next = (current + delta).clamp(-maximum, 0.0);
            handle.set_offset(gpui::point(offset.x, px(next)));
            self.profile_usage_scroll_state.cancel_to(next, maximum);
            cx.notify();
            return;
        }
        self.profile_usage_scroll_state
            .push(current, delta, maximum);
        self.schedule_profile_usage_scroll_frame(window, cx);
    }

    pub(super) fn schedule_profile_usage_scroll_frame(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.profile_usage_scroll_state.active() || self.profile_usage_scroll_frame_scheduled {
            return;
        }
        self.profile_usage_scroll_frame_scheduled = true;
        cx.on_next_frame(window, |application, window, cx| {
            application.advance_profile_usage_scroll(window, cx);
        });
    }

    pub(super) fn advance_profile_usage_scroll(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.profile_usage_scroll_frame_scheduled = false;
        let offset = self.profile_usage_scroll.offset();
        let maximum = f32::from(self.profile_usage_scroll.max_offset().y).max(0.0);
        if let Some(next) = self
            .profile_usage_scroll_state
            .step(f32::from(offset.y), maximum)
        {
            self.profile_usage_scroll
                .set_offset(gpui::point(offset.x, px(next)));
            cx.notify();
        }
        self.schedule_profile_usage_scroll_frame(window, cx);
    }

    /// Settles a queued scroll target at the current offset so dismissing
    /// the menu cannot leave inertia pending for the next open.
    pub(super) fn cancel_profile_usage_scroll(&mut self) {
        let offset = self.profile_usage_scroll.offset();
        let maximum = f32::from(self.profile_usage_scroll.max_offset().y).max(0.0);
        self.profile_usage_scroll_state
            .cancel_to(f32::from(offset.y), maximum);
        self.profile_usage_scroll_frame_scheduled = false;
    }

    /// Whether the profile menu accepts pointer and wheel input: open and
    /// past its retained exit presentation, mirroring the model picker.
    pub(super) fn profile_menu_is_interactive(&self) -> bool {
        self.profile_menu.is_open()
            && self.profile_menu_motion.borrow().phase() != PickerMenuPhase::Closing
    }

    pub(super) fn begin_profile_menu_open(&mut self, cx: &mut Context<Self>) {
        let generation = self.profile_menu_motion.borrow_mut().begin_open();
        if cx.reduce_motion() {
            self.profile_menu_motion
                .borrow_mut()
                .finish_open(generation);
            self.profile_menu_motion_task = None;
        } else {
            self.schedule_profile_menu_motion_settle(generation, true, cx);
        }
    }

    pub(super) fn begin_profile_menu_close(&mut self, cx: &mut Context<Self>) {
        let generation = self.profile_menu_motion.borrow_mut().begin_close();
        if cx.reduce_motion() {
            self.profile_menu_motion
                .borrow_mut()
                .finish_close(generation);
            self.profile_menu_motion_task = None;
        } else {
            self.schedule_profile_menu_motion_settle(generation, false, cx);
        }
    }

    pub(super) fn schedule_profile_menu_motion_settle(
        &mut self,
        generation: u64,
        opening: bool,
        cx: &mut Context<Self>,
    ) {
        self.profile_menu_motion_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(PICKER_MENU_MOTION_DURATION_MS))
                .await;
            let _ = this.update(cx, |application, cx| {
                let settled = if opening {
                    application
                        .profile_menu_motion
                        .borrow_mut()
                        .finish_open(generation)
                } else {
                    application
                        .profile_menu_motion
                        .borrow_mut()
                        .finish_close(generation)
                };
                if settled {
                    application.profile_menu_motion_task = None;
                    cx.notify();
                }
            });
        }));
    }

    #[expect(
        clippy::float_cmp,
        reason = "the tween is settled only when the displayed value reached its exact target; an epsilon would drop the final frame"
    )]
    pub(super) fn schedule_profile_tip_frame(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let settled = {
            let tween = self.profile_tip_tween.borrow();
            tween.displayed == tween.to
        };
        if settled || self.profile_tip_tween.borrow().scheduled {
            return;
        }
        self.profile_tip_tween.borrow_mut().scheduled = true;
        cx.on_next_frame(window, |application, window, cx| {
            application.advance_profile_tip(window, cx);
        });
    }

    #[expect(
        clippy::cast_precision_loss,
        reason = "millisecond clocks and elapsed intervals are converted to f64 for the shared easing math; the ranges stay far below 2^53"
    )]
    pub(super) fn advance_profile_tip(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.profile_tip_tween.borrow_mut().scheduled = false;
        let total_ms = MotionDuration::Fast.as_duration().as_millis() as f64;
        let now_ms = profile_usage_now_ms();
        let done = {
            let mut tween = self.profile_tip_tween.borrow_mut();
            let elapsed = now_ms.saturating_sub(tween.started_ms).max(0) as f64;
            let progress = (elapsed / total_ms).clamp(0.0, 1.0);
            let eased = MotionCurve::SmoothOut.sample(progress);
            tween.displayed = tween.from + (tween.to - tween.from) * eased;
            if progress >= 1.0 {
                tween.displayed = tween.to;
                true
            } else {
                false
            }
        };
        cx.notify();
        if !done {
            self.schedule_profile_tip_frame(window, cx);
        }
    }

    /// Moves one refresh control toward its target state, starting from the
    /// currently displayed values so rapid hover/focus/refresh changes
    /// reverse mid-flight. Reduced motion settles instantly.
    #[expect(
        clippy::float_cmp,
        reason = "re-targeting is skipped only when the requested values are exactly the current target; an epsilon would restart settled tweens"
    )]
    pub(super) fn retarget_profile_swap(
        &self,
        engine_id: &str,
        target: RefreshSwapTarget,
        reduce_motion: bool,
    ) {
        let mut swaps = self.profile_refresh_swap.borrow_mut();
        let swap = swaps
            .entry(engine_id.to_owned())
            .or_insert_with(RefreshSwap::resting);
        let to = target.values();
        // A reduced-motion change settles everything immediately even when
        // the target itself is unchanged, so already-queued steps observe
        // settled endpoints instead of regressing toward stale ones.
        if reduce_motion {
            let off = swap_offsets_for(to);
            swap.from = to;
            swap.displayed = to;
            swap.to = to;
            swap.off_from = off;
            swap.off_displayed = off;
            swap.off_to = off;
            return;
        }
        if swap.to == to {
            return;
        }
        swap.from = swap.displayed;
        swap.to = to;
        swap.off_from = swap.off_displayed;
        swap.off_to = swap_offsets_for(to);
        swap.started_ms = profile_usage_now_ms();
    }

    /// Recomputes one control's target from its live hover/focus/refresh
    /// inputs and ensures the frame driver runs while anything is moving.
    /// Called from pointer events and every render so keyboard focus changes
    /// that arrive without pointer events still animate.
    pub(super) fn refresh_profile_swap(
        &self,
        engine_id: &str,
        refreshing: bool,
        reduce_motion: bool,
        window: &mut Window,
        cx: &Context<Self>,
    ) {
        let hovered = self
            .profile_refresh_swap
            .borrow()
            .get(engine_id)
            .is_some_and(|swap| swap.hovered);
        let focused = self
            .profile_refresh_focus
            .borrow()
            .iter()
            .find(|(id, _)| id == engine_id)
            .is_some_and(|(_, handle)| handle.is_focused(window));
        let target = if refreshing {
            RefreshSwapTarget::Loading
        } else if hovered || focused {
            RefreshSwapTarget::Action
        } else {
            RefreshSwapTarget::Reading
        };
        self.retarget_profile_swap(engine_id, target, reduce_motion);
        self.schedule_profile_swap_frame(window, cx);
    }

    #[expect(
        clippy::float_cmp,
        reason = "frames stay scheduled until every displayed value exactly reaches its target; an epsilon would strand the final sub-pixel frame"
    )]
    pub(super) fn schedule_profile_swap_frame(&self, window: &mut Window, cx: &Context<Self>) {
        let pending = self
            .profile_refresh_swap
            .borrow()
            .values()
            .any(|swap| swap.displayed != swap.to);
        if !pending || self.profile_swap_frame_scheduled.get() {
            return;
        }
        self.profile_swap_frame_scheduled.set(true);
        cx.on_next_frame(window, |application, window, cx| {
            application.advance_profile_swap(window, cx);
        });
    }

    pub(super) fn advance_profile_swap(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.profile_swap_frame_scheduled.set(false);
        let running = self.step_profile_swaps(profile_usage_now_ms());
        cx.notify();
        if running {
            self.schedule_profile_swap_frame(window, cx);
        }
    }

    /// Steps every retained swap toward its target on the source 150ms
    /// ease-in-out curve (`MotionDuration::Quick` + `MotionCurve::EaseInOut`,
    /// matching `--text-swap-dur` and `ease-in-out`). Returns whether any
    /// swap is still moving. Pure over the passed clock so tests can drive
    /// interrupted transitions deterministically.
    #[expect(
        clippy::float_cmp,
        reason = "settled swaps are skipped by exact identity so a finished clock can never re-drive them; an epsilon would re-animate settled values"
    )]
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "the swap clock is f64 easing math narrowed to the f32 values GPUI paints; millisecond ranges stay below 2^53"
    )]
    pub(super) fn step_profile_swaps(&self, now_ms: i64) -> bool {
        let total_ms = MotionDuration::Quick.as_duration().as_millis() as f64;
        let mut running = false;
        let mut swaps = self.profile_refresh_swap.borrow_mut();
        for swap in swaps.values_mut() {
            // Fully settled entries are never re-driven: their clock may be
            // newer than their values after a reduced-motion settle.
            if swap.displayed == swap.to && swap.off_displayed == swap.off_to {
                continue;
            }
            let elapsed = now_ms.saturating_sub(swap.started_ms).max(0) as f64;
            let progress = (elapsed / total_ms).clamp(0.0, 1.0);
            let eased = MotionCurve::EaseInOut.sample(progress) as f32;
            for index in 0..3 {
                swap.displayed[index] =
                    swap.from[index] + (swap.to[index] - swap.from[index]) * eased;
                swap.off_displayed[index] =
                    swap.off_from[index] + (swap.off_to[index] - swap.off_from[index]) * eased;
            }
            if progress >= 1.0 {
                swap.displayed = swap.to;
                swap.off_displayed = swap.off_to;
            } else {
                running = true;
            }
        }
        running
    }

    pub(super) fn activate_profile_selection(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for action in self.profile_menu.take_actions() {
            match action.item_id().as_ref() {
                "usage" => {
                    // The Usage action forces a refresh and keeps the menu
                    // open. Provider rows are never invented locally; the
                    // native adapter owns refresh and provider data.
                    // Restore the Usage highlight so the sliding pill stays
                    // on the row under the pointer instead of jumping to
                    // Settings after the transient state clears.
                    self.profile_menu.set_open(true);
                    self.set_profile_highlight(1);
                    self.profile_hover
                        .borrow_mut()
                        .set_active(PROFILE_USAGE_HOVER_ID.to_owned());
                    self.profile_hover_keyboard.set(false);
                    self.ensure_profile_usage(true, None, cx);
                }
                "settings" => {
                    self.clear_profile_hover();
                    self.cancel_profile_usage_scroll();
                    self.begin_profile_menu_close(cx);
                    self.navigate(
                        NativeRoute::Settings {
                            section: SettingsRoute::Models,
                            engine: None,
                        },
                        cx,
                    );
                }
                _ => {}
            }
        }
        window.focus(&self.profile_focus, cx);
        cx.notify();
    }

    pub(super) fn desktop_profile_usage(
        &self,
        theme: DesktopTheme,
        window: &mut Window,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        let section = div()
            .id(crate::native_profile_usage::PROFILE_USAGE_SELECTOR)
            .debug_selector(|| crate::native_profile_usage::PROFILE_USAGE_SELECTOR.to_owned())
            .flex()
            .flex_col();

        let visible = self.profile_usage.visible_usage_entries();
        self.profile_refresh_focus.borrow_mut().retain(|(id, _)| {
            visible
                .iter()
                .any(|entry| entry.engine_id.as_str() == id.as_str())
        });
        self.profile_refresh_swap.borrow_mut().retain(|id, _| {
            visible
                .iter()
                .any(|entry| entry.engine_id.as_str() == id.as_str())
        });
        let now_ms = profile_usage_now_ms();
        let mut section = section.px(px(4.0)).py(px(4.0));
        for (index, entry) in visible.iter().enumerate() {
            if index > 0 {
                section = section.child(
                    div()
                        .h(px(1.0))
                        .bg(self.profile_separator_paint())
                        .my(px(4.0)),
                );
            }
            section =
                section.child(self.desktop_profile_usage_engine(entry, theme, window, now_ms, cx));
        }
        section
    }

    /// One provider block matching `sidebar-engine-usage.svelte`: the engine
    /// mark plus name with an inline hover-swapping refresh control, one
    /// cadence group per disclosed cadence with 12px labels and 72x8 accent
    /// meters, and a muted reset sentence with the duration in foreground.
    /// Entries without renderable windows never reach this renderer (see
    /// [`NativeProfileUsageState::visible_usage_entries`]).
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI composition keeps the arrangement in visual order; extraction would split the shared reactive state"
    )]
    pub(super) fn desktop_profile_usage_engine(
        &self,
        entry: &NativeUsageEntry,
        theme: DesktopTheme,
        window: &mut Window,
        now_ms: i64,
        cx: &Context<Self>,
    ) -> Div {
        let engine_id = entry.engine_id.clone();
        let Some(report) = entry.report.as_ref() else {
            return div();
        };
        let refreshing = self
            .profile_usage
            .refreshing_engine_ids
            .iter()
            .any(|current| current == &engine_id);
        let mark_asset = engine_asset(&engine_id);
        let accent = engine_accent(&engine_id).map_or_else(
            || self.theme.colors.primary.to_paint(),
            |hex| gpui::rgb_to_hsla(gpui::rgb(hex)),
        );
        let dim = self.theme.colors.foreground.with_alpha(0.11).to_paint();
        let block_selector = format!("artisan-profile-usage-engine-{engine_id}");
        let mut block = div()
            .debug_selector({
                let block_selector = block_selector.clone();
                move || block_selector.clone()
            })
            .flex()
            .flex_col()
            .gap(px(6.0))
            .px(px(8.0))
            .py(px(4.0));
        let mut title = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .min_w(px(0.0))
                    .items_center()
                    .gap(px(8.0))
                    // Foreground ambient: monochrome provider marks resolve
                    // through it like source `dark:invert`; full-color marks
                    // keep their authored colors either way.
                    .text_color(theme.foreground)
                    .child(
                        icon(IconStyle::resolve(
                            self.theme,
                            mark_asset,
                            IconSize::Default,
                            if mark_asset == AssetId::TABLER_QUESTION_MARK {
                                IconTint::Muted
                            } else {
                                IconTint::Inherit
                            },
                        ))
                        .size(px(16.0))
                        .flex_shrink_0(),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12.0))
                            .line_height(px(16.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.foreground)
                            .child(report.display_name.clone()),
                    ),
            );
        if let Some(checked) = checked_label(entry.fetched_at_ms, now_ms) {
            title = title.child(self.desktop_profile_usage_refresh(
                &engine_id, &checked, refreshing, theme, window, cx,
            ));
        }
        block = block.child(title);
        if !refreshing
            && let Some(failure) = engine_refresh_failure(&self.profile_usage, &engine_id)
        {
            let failure_selector = format!("artisan-profile-usage-failure-{engine_id}");
            block = block.child(
                div()
                    .debug_selector(move || failure_selector.clone())
                    .text_size(px(12.0))
                    .line_height(px(16.0))
                    .text_color(theme.secondary)
                    .child(failure),
            );
        }
        for (group_index, group) in group_usage_windows(&report.windows).iter().enumerate() {
            let mut group_view = div().flex().flex_col().gap(px(6.0));
            if group_index > 0 {
                group_view = group_view.mt(px(8.0));
            }
            group_view = group_view.child(
                div()
                    .text_size(px(12.0))
                    .line_height(px(16.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.foreground)
                    .child(group.cadence.title()),
            );
            for window in &group.windows {
                group_view = group_view.child(
                    self.desktop_profile_usage_meter(&engine_id, window, accent, dim, theme, cx),
                );
            }
            if let Some(duration) = reset_duration(&group.windows, now_ms) {
                // One inline run like source: the duration range carries the
                // foreground highlight so spaces and wrapping are preserved.
                let sentence = format!(
                    "Your {} limit resets in {}.",
                    group.cadence.title().to_lowercase(),
                    duration
                );
                let body = match sentence.find(duration.as_str()) {
                    Some(start) => {
                        let end = start + duration.len();
                        StyledText::new(SharedString::from(sentence)).with_highlights([(
                            start..end,
                            HighlightStyle {
                                color: Some(theme.foreground),
                                ..Default::default()
                            },
                        )])
                    }
                    None => StyledText::new(SharedString::from(sentence)),
                };
                group_view = group_view.child(
                    div()
                        .mt(px(8.0))
                        .text_size(px(12.0))
                        .line_height(px(16.0))
                        .text_color(theme.secondary)
                        .child(body),
                );
            }
            block = block.child(group_view);
        }
        block
    }

    /// Inline checked/refresh control: the provider's own "last checked"
    /// reading at rest, swapping to a foreground Refresh action on hover or
    /// keyboard focus and to a spinner while its refresh is in flight. All
    /// three readings share one grid cell like the source `t-checked`
    /// grid, so the width is always the max of reading and action and the
    /// swap never shifts layout; each reading animates on the source 150ms
    /// ease-in-out opacity/blur(2px)/Â±4px paint offset, interrupted from the
    /// retained visual values. Withheld until the engine has answered at
    /// least once. Keyboard focus plus Enter/Space refreshes once through
    /// the same path as a click.
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI composition keeps the arrangement in visual order; extraction would split the shared reactive state"
    )]
    pub(super) fn desktop_profile_usage_refresh(
        &self,
        engine_id: &str,
        checked: &str,
        refreshing: bool,
        theme: DesktopTheme,
        window: &mut Window,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        let group = format!("profile-usage-refresh-{engine_id}");
        let selector = format!("artisan-profile-usage-refresh-{engine_id}");
        let focus = self.profile_refresh_focus_handle(engine_id, cx);
        self.refresh_profile_swap(engine_id, refreshing, cx.reduce_motion(), window, cx);
        let swap = self
            .profile_refresh_swap
            .borrow()
            .get(engine_id)
            .copied()
            .unwrap_or_else(RefreshSwap::resting);
        // One grid cell shared by all three readings, so the control width
        // is always the max of reading and action like the source
        // `t-checked` grid â€” never collapsing, never shifting on swap. Each
        // reading paints its retained opacity/blur/offset triple, so a
        // reversal keeps interpolating without sign flips. The ring only
        // attaches for keyboard-driven focus, matching source
        // `:focus-visible` (a mouse click followed by pointer leave is
        // still mouse focus and shows no ring).
        let paint = |displayed: f32, off_displayed: f32| {
            (displayed, (1.0 - displayed) * 2.0, off_displayed)
        };
        let (reading_opacity, reading_blur, reading_top) =
            paint(swap.displayed[0], swap.off_displayed[0]);
        let (action_opacity, action_blur, action_top) =
            paint(swap.displayed[1], swap.off_displayed[1]);
        let (spinner_opacity, spinner_blur, spinner_top) =
            paint(swap.displayed[2], swap.off_displayed[2]);
        let ring = vec![gpui::BoxShadow {
            color: self.theme.interaction.focus_ring_color.to_paint(),
            offset: gpui::point(px(0.0), px(0.0)),
            blur_radius: px(0.0),
            spread_radius: self.theme.interaction.focus_ring_width,
            inset: false,
        }];
        let mut control = div()
            .id(selector.clone())
            .debug_selector({
                let selector = selector.clone();
                move || selector.clone()
            })
            .track_focus(&focus)
            .group(group)
            .grid()
            .flex_shrink_0()
            .text_size(px(12.0))
            .line_height(px(16.0));
        if focus.is_focused(window) && window.last_input_was_keyboard() {
            control = control.focus(move |style| style.shadow(ring));
        }
        control = control
            .child(
                div()
                    .col_start(1)
                    .row_start(1)
                    .relative()
                    .top(px(reading_top))
                    .whitespace_nowrap()
                    .text_color(theme.secondary)
                    .child(checked.to_owned())
                    .opacity(reading_opacity)
                    .blur(px(reading_blur)),
            )
            .child(
                div()
                    .col_start(1)
                    .row_start(1)
                    .relative()
                    .top(px(action_top))
                    .flex()
                    .items_center()
                    .justify_end()
                    .opacity(action_opacity)
                    .blur(px(action_blur))
                    .child(
                        div()
                            .whitespace_nowrap()
                            .text_color(theme.foreground)
                            .child("Refresh"),
                    ),
            )
            .child(
                div()
                    .col_start(1)
                    .row_start(1)
                    .relative()
                    .top(px(spinner_top))
                    .flex()
                    .items_center()
                    .justify_end()
                    .opacity(spinner_opacity)
                    .blur(px(spinner_blur))
                    .child(
                        FadeArc::new(SharedString::from(selector.clone()), self.theme)
                            .size(px(14.0))
                            .active(refreshing)
                            .debug_selector(format!("{selector}-spinner")),
                    ),
            )
            .on_hover(cx.listener({
                let swap_engine_id = engine_id.to_owned();
                let swap_focus = focus.clone();
                move |app, hovered: &bool, window, cx| {
                    if let Some(swap) = app
                        .profile_refresh_swap
                        .borrow_mut()
                        .get_mut(swap_engine_id.as_str())
                    {
                        swap.hovered = *hovered;
                    }
                    let target = if refreshing {
                        RefreshSwapTarget::Loading
                    } else if *hovered || swap_focus.is_focused(window) {
                        RefreshSwapTarget::Action
                    } else {
                        RefreshSwapTarget::Reading
                    };
                    app.retarget_profile_swap(&swap_engine_id, target, cx.reduce_motion());
                    app.schedule_profile_swap_frame(window, cx);
                    cx.notify();
                }
            }));
        if !refreshing {
            let click_engine_id = engine_id.to_owned();
            let key_engine_id = engine_id.to_owned();
            control = control
                .cursor_pointer()
                .on_click(cx.listener(move |app, _, _, cx| {
                    app.refresh_single_profile_engine(&click_engine_id, cx);
                }))
                .on_key_down(cx.listener(move |app, event: &gpui::KeyDownEvent, _, cx| {
                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        cx.stop_propagation();
                        app.refresh_single_profile_engine(&key_engine_id, cx);
                        cx.notify();
                    }
                }));
        }
        control
    }

    /// One cadence meter row: scope label plus a fixed 72x8 provider-accent
    /// meter with the source 14-tick quantization. Hovering arms the glass
    /// remaining tooltip; the exact percentage lives only there, never as
    /// inline text.
    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI composition keeps the arrangement in visual order; extraction would split the shared reactive state"
    )]
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the meter tick count and its percentage are both bounded small whole numbers; the f64 ratio round-trips through usize exactly"
    )]
    pub(super) fn desktop_profile_usage_meter(
        &self,
        engine_id: &str,
        window: &NativeUsageWindow,
        accent: gpui::Hsla,
        dim: gpui::Hsla,
        theme: DesktopTheme,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        let segments = usize::from(crate::usage_meter::USAGE_METER_SEGMENTS);
        let lit_segments =
            (usage_segment_fraction(window.percent_used) * segments as f64).round() as usize;
        // Fourteen full pitches across 72px with a 2px transparent tail cut
        // from every pitch including the last, matching the source mask.
        let tip_key = (engine_id.to_owned(), window.id.clone());
        let meter_selector = format!("artisan-profile-usage-meter-{engine_id}-{}", window.id);
        let bar_selector = format!("{meter_selector}-bar");
        let mut meter = div()
            .debug_selector({
                let bar_selector = bar_selector.clone();
                move || bar_selector.clone()
            })
            .w(px(72.0))
            .h(px(8.0))
            .flex_shrink_0()
            .flex();
        for index in 0..segments {
            meter = meter.child(
                div()
                    .w(px(PROFILE_METER_TICK_PX))
                    .mr(px(2.0))
                    .h_full()
                    .bg(if index < lit_segments { accent } else { dim }),
            );
        }
        let meter_hover = Rc::clone(&self.profile_meter_hover);
        let tip_surface = Rc::clone(&self.profile_tip_surface_bounds);
        let tip_anchor = Rc::clone(&self.profile_tip_anchor);
        let probe_key = tip_key.clone();
        let meter_probe = canvas(
            |_, _, _| {},
            move |bounds, (), window, cx| {
                if meter_hover.borrow().as_ref() != Some(&probe_key) {
                    return;
                }
                let Some(surface) = *tip_surface.borrow() else {
                    return;
                };
                let rect = HoverRect {
                    left: f32::from(bounds.left() - surface.left()),
                    top: f32::from(bounds.top() - surface.top()),
                    width: f32::from(bounds.size.width),
                    height: f32::from(bounds.size.height),
                };
                let mut anchor = tip_anchor.borrow_mut();
                if anchor
                    .as_ref()
                    .is_some_and(|(key, current)| key == &probe_key && *current == rect)
                {
                    return;
                }
                *anchor = Some((probe_key.clone(), rect));
                window.defer(cx, |window, _| window.refresh());
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();
        div()
            .id(meter_selector.clone())
            .relative()
            .flex()
            .items_center()
            .gap(px(16.0))
            .debug_selector({
                let meter_selector = meter_selector.clone();
                move || meter_selector.clone()
            })
            .child(meter_probe)
            .child(
                div()
                    .min_w(px(0.0))
                    .flex_1()
                    .truncate()
                    .pl(px(8.0))
                    .text_size(px(12.0))
                    .line_height(px(16.0))
                    .text_color(theme.secondary)
                    .child(window.scope_label().to_owned()),
            )
            .child(meter)
            .on_hover(cx.listener({
                let tip_key = tip_key.clone();
                let percent_used = window.percent_used;
                move |app, hovered: &bool, window, cx| {
                    if *hovered {
                        *app.profile_meter_hover.borrow_mut() = Some(tip_key.clone());
                        // One shared tween per menu-open session: the first
                        // reading runs up from just short of its value,
                        // later rows carry the displayed value across.
                        let remaining = usage_remaining_percent(percent_used) as f64;
                        {
                            let mut tween = app.profile_tip_tween.borrow_mut();
                            if tween.seen {
                                tween.from = tween.displayed;
                            } else {
                                tween.seen = true;
                                tween.displayed = tip_run_up_from(remaining);
                                tween.from = tween.displayed;
                            }
                            tween.to = remaining;
                            tween.started_ms = profile_usage_now_ms();
                            if cx.reduce_motion() {
                                tween.displayed = remaining;
                                tween.scheduled = false;
                            }
                        }
                        app.schedule_profile_tip_frame(window, cx);
                    } else if app.profile_meter_hover.borrow().as_ref() == Some(&tip_key) {
                        app.profile_meter_hover.borrow_mut().take();
                        app.profile_tip_anchor.borrow_mut().take();
                    }
                    cx.notify();
                }
            }))
    }

    /// Glass remaining tooltip for the hovered meter, anchored beside its
    /// row with the source 8px offset and clamped into the viewport. The
    /// number is the shared tweened remaining percentage, so moving across
    /// rows carries one value onto the next.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the tweened remaining percentage is clamped to 0..=100 before it is rounded to the integer tooltip label"
    )]
    pub(super) fn desktop_profile_usage_tooltip(
        &self,
        window: &Window,
        theme: DesktopTheme,
    ) -> Option<Stateful<Div>> {
        let (engine_id, window_id) = self.profile_meter_hover.borrow().clone()?;
        let ((anchor_engine, anchor_window), anchor) = self.profile_tip_anchor.borrow().clone()?;
        if (engine_id.clone(), window_id.clone()) != (anchor_engine, anchor_window) {
            return None;
        }
        let known = self
            .profile_usage
            .entry(&engine_id)
            .and_then(|entry| entry.report.as_ref())
            .and_then(|report| report.windows.iter().find(|window| window.id == window_id))
            .is_some();
        if !known {
            return None;
        }
        let displayed = self.profile_tip_tween.borrow().displayed;
        let viewport_width = f32::from(window.bounds().size.width);
        let surface_left = f32::from(self.profile_tip_surface_bounds.borrow().as_ref()?.left());
        let minimum_left = PROFILE_MENU_VIEWPORT_MARGIN_PX - surface_left;
        let max_left = (viewport_width - PROFILE_MENU_VIEWPORT_MARGIN_PX - 224.0 - surface_left)
            .max(minimum_left);
        let left = (anchor.left + anchor.width + 8.0).clamp(minimum_left, max_left);
        Some(
            div()
                .id("artisan-profile-usage-tooltip")
                .debug_selector(|| "artisan-profile-usage-tooltip".to_owned())
                .absolute()
                .left(px(left))
                .top(px(anchor.top))
                .max_w(px(224.0))
                .rounded(RadiusTokens::value(RadiusStep::X2l))
                .backdrop_blur(glass_blur_radius(GlassStrength::Quiet))
                .bg(glass_foreground_base(&self.theme))
                .border_1()
                .border_color(theme.line)
                .shadow(glass_card_shadows())
                .child(glass_material_layer(
                    GlassStrength::Quiet,
                    RadiusTokens::value(RadiusStep::X2l),
                ))
                .child(glass_highlight_layer(
                    GlassStrength::Quiet,
                    RadiusTokens::value(RadiusStep::X2l),
                ))
                .child(
                    div()
                        .px(px(12.0))
                        .py(px(8.0))
                        .flex()
                        .flex_row()
                        .items_center()
                        .text_size(px(12.0))
                        .line_height(px(16.0))
                        .text_color(theme.foreground)
                        .child("You have ")
                        .child(
                            div()
                                .text_color(theme.foreground)
                                .child(format!("{}%", displayed.round() as i64)),
                        )
                        .child(" left."),
                ),
        )
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI composition keeps the arrangement in visual order; extraction would split the shared reactive state"
    )]
    pub(super) fn desktop_profile(&self, window: &mut Window, cx: &Context<Self>) -> Div {
        let theme = self.desktop_theme;
        let show_usage = self.profile_usage_visible();
        let origin = self.profile_origin.clone();
        let host_identity = crate::native_hosts::presentation(self.machine_home.as_deref());
        let render_avatar = || {
            crate::shell::profile_avatar(
                &self.theme,
                crate::shell::RailIdentity::new(Some(&host_identity.avatar_seed), None),
            )
            .rounded(px(8.0))
            .overflow_hidden()
            .into_any_element()
        };
        let avatar = render_avatar();
        let focus_ring = vec![gpui::BoxShadow {
            color: self.theme.interaction.focus_ring_color.to_paint(),
            offset: gpui::point(px(0.0), px(0.0)),
            blur_radius: px(0.0),
            spread_radius: self.theme.interaction.focus_ring_width,
            inset: false,
        }];
        // The ring marks keyboard focus only: a mouse click focuses too, but
        // mouse modality never shows it, matching source `:focus-visible`.
        let keyboard_focused =
            self.profile_focus.is_focused(window) && window.last_input_was_keyboard();
        let mut trigger = div()
            .id("artisan-desktop-profile-trigger")
            .debug_selector(|| "artisan-desktop-profile-trigger".to_string())
            .track_focus(&self.profile_focus)
            .tab_index(0)
            .cursor_pointer()
            .rounded(px(8.0))
            .w_full()
            .h(px(44.0))
            .p(px(5.0));
        if keyboard_focused {
            trigger = trigger.focus(move |style| style.shadow(focus_ring));
        }
        trigger = trigger
            .flex()
            .items_center()
            .gap(px(8.0))
            .relative()
            .on_hover(cx.listener(|app: &mut Self, hovered: &bool, _, cx| {
                if *hovered {
                    app.sidebar_hover
                        .borrow_mut()
                        .set_active(SIDEBAR_PROFILE_HOVER_ID.to_owned());
                } else if app.sidebar_hover.borrow().active_id() == Some(SIDEBAR_PROFILE_HOVER_ID) {
                    app.sidebar_hover.borrow_mut().hide();
                }
                cx.notify();
            }))
            .child(sidebar_hover_probe(
                Rc::clone(&self.sidebar_hover),
                Rc::clone(&self.sidebar_hover_surface_bounds),
                SIDEBAR_PROFILE_HOVER_ID,
            ))
            .child(
                div()
                    .size(px(32.0))
                    .flex_shrink_0()
                    .rounded(px(8.0))
                    .overflow_hidden()
                    .child(avatar),
            )
            .children((!self.sidebar_collapsed).then(|| {
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .gap(px(0.0))
                    .child(
                        div()
                            .truncate()
                            .text_size(px(14.0))
                            .line_height(px(16.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.foreground)
                            .child(self.profile_display_name(cx)),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(10.0))
                            .line_height(px(12.0))
                            .text_color(theme.secondary)
                            .child(self.machine_label.clone()),
                    )
            }))
            .children((!self.sidebar_collapsed).then(|| {
                desktop_nav_glyph(
                    if self.profile_menu.is_open() {
                        AssetId::TABLER_CHEVRON_DOWN
                    } else {
                        AssetId::TABLER_CHEVRON_UP
                    },
                    theme,
                )
            }))
            .on_click(cx.listener(|app, _, window, cx| {
                cx.stop_propagation();
                let was_open = app.profile_menu.is_open();
                let _ = app.profile_menu.press_trigger();
                window.focus(&app.profile_focus, cx);
                if !was_open && app.profile_menu.is_open() {
                    app.clear_profile_hover();
                    app.begin_profile_menu_open(cx);
                    app.ensure_profile_usage(false, None, cx);
                } else {
                    app.clear_profile_hover();
                    app.cancel_profile_usage_scroll();
                    if was_open {
                        app.begin_profile_menu_close(cx);
                    }
                }
                cx.notify();
            }))
            .on_action(cx.listener(|app, _: &NextTabStop, window, cx| {
                if app.profile_menu.is_open() {
                    app.focus_machine_trigger(window, cx);
                    cx.stop_propagation();
                } else {
                    window.focus_next(cx);
                }
            }))
            .on_key_down(cx.listener(|app, event: &gpui::KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => {
                        let was_open = app.profile_menu.is_open();
                        let _ = app.profile_menu.dismiss();
                        app.clear_profile_hover();
                        app.cancel_profile_usage_scroll();
                        if was_open {
                            app.begin_profile_menu_close(cx);
                        }
                    }
                    "down" => {
                        let was_open = app.profile_menu.is_open();
                        app.profile_menu.set_open(true);
                        let _ = app.profile_menu.move_next();
                        app.sync_profile_hover_to_highlight();
                        if !was_open {
                            app.begin_profile_menu_open(cx);
                        }
                        app.ensure_profile_usage(false, None, cx);
                    }
                    "up" => {
                        let was_open = app.profile_menu.is_open();
                        app.profile_menu.set_open(true);
                        let _ = app.profile_menu.move_previous();
                        app.sync_profile_hover_to_highlight();
                        if !was_open {
                            app.begin_profile_menu_open(cx);
                        }
                        app.ensure_profile_usage(false, None, cx);
                    }
                    "home" => {
                        let _ = app.profile_menu.move_first();
                        app.sync_profile_hover_to_highlight();
                    }
                    "end" => {
                        let _ = app.profile_menu.move_last();
                        app.sync_profile_hover_to_highlight();
                    }
                    "enter" | "space" => {
                        if app.profile_menu.is_open() {
                            let _ = app.profile_menu.activate_highlighted();
                            app.activate_profile_selection(window, cx);
                        } else {
                            let _ = app.profile_menu.press_trigger();
                            if app.profile_menu.is_open() {
                                app.clear_profile_hover();
                                app.begin_profile_menu_open(cx);
                                app.ensure_profile_usage(false, None, cx);
                            }
                        }
                    }
                    "tab" if app.profile_menu.is_open() && !event.keystroke.modifiers.shift => {
                        app.focus_machine_trigger(window, cx);
                    }
                    "tab" => {
                        let was_open = app.profile_menu.is_open();
                        let _ = app.profile_menu.dismiss();
                        app.clear_profile_hover();
                        app.cancel_profile_usage_scroll();
                        if was_open {
                            app.begin_profile_menu_close(cx);
                        }
                        cx.notify();
                        return;
                    }
                    _ => return,
                }
                cx.stop_propagation();
                cx.notify();
            }));
        let mut root =
            div()
                .relative()
                .w_full()
                .child(
                    div()
                        .child(trigger)
                        .on_children_prepainted(move |bounds, _, _| {
                            if let Some(bounds) = bounds.first() {
                                origin.set(*bounds);
                            }
                        }),
                );
        // The retained exit presentation stays mounted through Closing so
        // the shared 100ms fade/slide-out can complete, exactly like the
        // model picker popover.
        let menu_phase = self.profile_menu_motion.borrow().phase();
        if self.profile_menu.is_open() || menu_phase == PickerMenuPhase::Closing {
            // The dropdown paints from the shared Artisan text tokens rather
            // than the desktop shell's custom palette; the bottom trigger
            // keeps the shell palette.
            let theme = DesktopTheme {
                foreground: self.theme.colors.foreground.to_paint(),
                secondary: self.theme.colors.muted_foreground.to_paint(),
                ..theme
            };
            // Source `bg-border/50` resolved from the shared border token.
            let separator = self.profile_separator_paint();
            let tip_surface = Rc::clone(&self.profile_tip_surface_bounds);
            let tip_surface_probe = canvas(
                |_, _, _| {},
                move |bounds, (), window, cx| {
                    let changed = {
                        let mut surface = tip_surface.borrow_mut();
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
            let profile_hover = Rc::clone(&self.profile_hover);
            let profile_hover_surface = Rc::clone(&self.profile_hover_surface_bounds);
            let profile_surface_probe = {
                let surface = Rc::clone(&profile_hover_surface);
                canvas(
                    |_, _, _| {},
                    move |bounds, (), window, cx| {
                        let changed = {
                            let mut surface = surface.borrow_mut();
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
                .size_full()
            };
            let mut panel = div()
                .id("artisan-desktop-profile-menu")
                .debug_selector(|| "artisan-desktop-profile-menu".to_string())
                .min_w(px(256.0))
                .max_w(px(352.0))
                .rounded(RadiusTokens::value(RadiusStep::X2l))
                .backdrop_blur(glass_blur_radius(GlassStrength::Quiet))
                .bg(glass_foreground_base(&self.theme))
                .border_1()
                .border_color(theme.line)
                .shadow(glass_card_shadows())
                .flex()
                .flex_col()
                .relative()
                .child(glass_material_layer(
                    GlassStrength::Quiet,
                    RadiusTokens::value(RadiusStep::X2l),
                ))
                .child(glass_highlight_layer(
                    GlassStrength::Quiet,
                    RadiusTokens::value(RadiusStep::X2l),
                ))
                .child(tip_surface_probe)
                .child(profile_surface_probe)
                .child(render_picker_hover_pill(
                    &self.theme,
                    &profile_hover,
                    "profile",
                    RadiusTokens::value(RadiusStep::Xl),
                    cx.reduce_motion(),
                ))
                .on_hover(cx.listener(|app, hovered: &bool, _, cx| {
                    if !*hovered && !app.profile_hover_keyboard.get() && !app.machine_menu.is_open()
                    {
                        app.profile_hover.borrow_mut().clear();
                        cx.notify();
                    }
                }))
                .block_mouse_except_scroll()
                .on_mouse_down_out(cx.listener(|app, event: &gpui::MouseDownEvent, _, cx| {
                    if app.machine_menu.is_open() {
                        return;
                    }
                    let trigger = app.profile_origin.get();
                    if !trigger.contains(&event.position) {
                        let was_open = app.profile_menu.is_open();
                        let _ = app.profile_menu.dismiss();
                        app.clear_profile_hover();
                        app.cancel_profile_usage_scroll();
                        if was_open {
                            app.begin_profile_menu_close(cx);
                        }
                        cx.notify();
                    }
                }))
                .child(
                    div()
                        .debug_selector(|| "artisan-desktop-profile-header".to_owned())
                        .p(px(4.0))
                        .child(
                            self.machine_trigger(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .flex()
                                    .items_center()
                                    .gap(px(12.0))
                                    .child(
                                        div()
                                            .size(px(32.0))
                                            .flex_shrink_0()
                                            .rounded(px(8.0))
                                            .overflow_hidden()
                                            .child(render_avatar()),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w(px(0.0))
                                            .flex()
                                            .flex_col()
                                            .gap(px(0.0))
                                            .child(
                                                div()
                                                    .truncate()
                                                    .text_size(px(14.0))
                                                    .line_height(px(20.0))
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .text_color(theme.foreground)
                                                    .child(self.profile_display_name(cx)),
                                            )
                                            .child(
                                                div()
                                                    .truncate()
                                                    .text_size(px(12.0))
                                                    .line_height(px(16.0))
                                                    .text_color(theme.secondary)
                                                    .child(
                                                        self.host_switch_status().unwrap_or_else(
                                                            || self.machine_label.clone(),
                                                        ),
                                                    ),
                                            ),
                                    )
                                    .into_any_element(),
                                cx,
                            ),
                        ),
                )
                .child(div().h(px(1.0)).bg(separator).my(px(4.0)))
                .children(show_usage.then(|| {
                    div()
                        .id("artisan-profile-usage-scroll")
                        .debug_selector(|| "artisan-profile-usage-scroll".to_owned())
                        .min_h(px(0.0))
                        .max_h(self.profile_usage_max_height(window))
                        .overflow_y_scroll()
                        .track_scroll(&self.profile_usage_scroll)
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .flex_col()
                                .min_w(px(0.0))
                                .flex_shrink_0()
                                .on_scroll_wheel(
                                    cx.listener(Self::handle_profile_usage_scroll_wheel),
                                )
                                .child(self.desktop_profile_usage(theme, window, cx)),
                        )
                }))
                .children(show_usage.then(|| div().h(px(1.0)).bg(separator).my(px(4.0))));
            self.profile_hover.borrow_mut().clear_if_missing(&[
                PROFILE_SETTINGS_HOVER_ID.to_owned(),
                PROFILE_USAGE_HOVER_ID.to_owned(),
                "profile-host".to_owned(),
            ]);
            let profile_row_hover = Rc::clone(&profile_hover);
            let profile_row_surface = Rc::clone(&profile_hover_surface);
            let profile_hover_probe = move |id: &'static str| {
                let measured_id = id.to_owned();
                let hover = Rc::clone(&profile_row_hover);
                let surface_bounds = Rc::clone(&profile_row_surface);
                canvas(
                    |_, _, _| {},
                    move |bounds, (), window, cx| {
                        let Some(surface) = *surface_bounds.borrow() else {
                            return;
                        };
                        let rect = HoverRect {
                            left: f32::from(bounds.left() - surface.left()),
                            top: f32::from(bounds.top() - surface.top()),
                            width: f32::from(bounds.size.width),
                            height: f32::from(bounds.size.height),
                        };
                        if hover.borrow_mut().measure(&measured_id, rect) {
                            window.defer(cx, |window, _| window.refresh());
                        }
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full()
            };
            let mut actions = div()
                .id("artisan-desktop-profile-actions-hover-surface")
                .debug_selector(|| "artisan-desktop-profile-actions-hover-surface".to_owned())
                .relative()
                .w_full()
                .flex()
                .flex_col()
                .p(px(4.0));
            for (index, (label, icon, hover_id)) in [
                (
                    "Settings",
                    AssetId::TABLER_SETTINGS,
                    PROFILE_SETTINGS_HOVER_ID,
                ),
                (
                    "Usage",
                    AssetId::TABLER_LIST_DETAILS,
                    PROFILE_USAGE_HOVER_ID,
                ),
            ]
            .into_iter()
            .enumerate()
            .filter(|(index, _)| show_usage || *index == 0)
            {
                let row_selector = format!("artisan-desktop-profile-action-{index}");
                let row_probe = profile_hover_probe(hover_id);
                let row_hover_id = hover_id.to_owned();
                actions = actions.child(
                    div()
                        .id(("artisan-profile-action", index))
                        .debug_selector(move || row_selector.clone())
                        .relative()
                        .px(px(12.0))
                        .py(px(8.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .rounded(RadiusTokens::value(RadiusStep::Xl))
                        .cursor_pointer()
                        .child(row_probe)
                        .child(desktop_nav_glyph(icon, theme))
                        .child(
                            div()
                                .text_size(px(14.0))
                                .line_height(px(20.0))
                                .text_color(theme.foreground)
                                .child(label),
                        )
                        .on_hover(cx.listener(move |app, hovered: &bool, window, cx| {
                            if *hovered {
                                if app.machine_menu.is_open() {
                                    app.profile_focus.focus(window, cx);
                                }
                                app.dismiss_machine_submenu();
                                app.set_profile_highlight(index);
                                app.profile_hover
                                    .borrow_mut()
                                    .set_active(row_hover_id.clone());
                                app.profile_hover_keyboard.set(false);
                                cx.notify();
                            }
                        }))
                        .on_click(cx.listener(move |app, _, window, cx| {
                            if !app.profile_menu_is_interactive() {
                                return;
                            }
                            cx.stop_propagation();
                            let _ = app.profile_menu.activate_index(index);
                            app.activate_profile_selection(window, cx);
                        })),
                );
            }
            panel = panel.child(actions);
            if let Some(tooltip) = self.desktop_profile_usage_tooltip(window, theme) {
                panel = panel.child(tooltip);
            }
            let motion = *self.profile_menu_motion.borrow();
            root = root.child(gpui::deferred(
                gpui::anchored()
                    .anchor(gpui::Anchor::BottomLeft)
                    .position(self.profile_origin.get().origin)
                    .offset(gpui::point(px(0.0), px(-4.0)))
                    .child(animate_picker_menu(
                        panel,
                        Rc::clone(&self.profile_menu_motion),
                        motion,
                        "profile",
                    )),
            ));
        }
        root
    }
}
