//! Construction, event handling, menu motion, scrolling, tooltips, and key
//! gestures for the native model selector.
//!
//! Extracted verbatim from `native_model_selector.rs` during the module split;
//! visibility was widened to `pub(super)` for methods read by sibling modules.

use super::*;

impl EventEmitter<NativeModelSelectorEvent> for NativeModelSelector {}

impl Focusable for NativeModelSelector {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.trigger_focus.clone()
    }
}

impl NativeModelSelector {
    /// Builds a selector entity over a complete catalog snapshot.
    pub fn new(
        snapshot: NativeModelCatalog,
        policy: Option<NativeModelPolicy>,
        mode: ThemeMode,
        cx: &mut Context<Self>,
    ) -> Self {
        let virtual_model_scroll = gpui::UniformListScrollHandle::new();
        let menu_scroll = virtual_model_scroll.0.borrow().base_handle.clone();
        Self {
            state: NativeModelSelectorState::new(snapshot, policy),
            theme: ArtisanTheme::for_mode(mode),
            trigger_focus: cx.focus_handle().tab_index(1).tab_stop(true),
            menu_focus: cx.focus_handle(),
            menu_scroll,
            virtual_model_scroll,
            axis_menu_scroll: ScrollHandle::new(),
            trigger_origin: Rc::new(RefCell::new(None)),
            menu_bounds: Rc::new(RefCell::new(None)),
            trigger_bounds: Rc::new(RefCell::new(None)),
            engine_indicator: Rc::new(RefCell::new(EngineSectionIndicatorPolicy::new())),
            engine_surface_bounds: Rc::new(RefCell::new(None)),
            engine_indicator_transition: Rc::new(RefCell::new(None)),
            engine_indicator_animation_generation: Rc::new(RefCell::new(0)),
            menu_motion: Rc::new(RefCell::new(PickerMenuMotion::default())),
            menu_motion_task: None,
            axis_menu_motion: Rc::new(RefCell::new(PickerMenuMotion::default())),
            axis_menu_motion_task: None,
            axis_menu_motion_axis: None,
            axis_menu_pending_axis: None,
            model_hover: Rc::new(RefCell::new(SlidingHoverState::default())),
            model_hover_surface_bounds: Rc::new(RefCell::new(None)),
            axis_hover: Rc::new(RefCell::new(SlidingHoverState::default())),
            axis_hover_surface_bounds: Rc::new(RefCell::new(None)),
            model_scroll: PickerScrollState::default(),
            model_scroll_frame_scheduled: false,
            axis_scroll: PickerScrollState::default(),
            axis_scroll_frame_scheduled: false,
            axis_trigger_bounds: Rc::new(RefCell::new([None; 5])),
            axis_menu_bounds: Rc::new(RefCell::new(None)),
            option_tooltip: Rc::new(RefCell::new(None)),
            option_tooltip_task: None,
            option_tooltip_generation: 0,
            highlighted_axis_option: None,
        }
    }

    /// Returns read-only pure interaction state.
    #[must_use]
    pub const fn state(&self) -> &NativeModelSelectorState {
        &self.state
    }

    /// Replaces the complete static-plus-runtime catalog snapshot.
    pub fn set_snapshot(&mut self, snapshot: NativeModelCatalog, cx: &mut Context<Self>) {
        let closing_axis = self.axis_menu_motion_axis.or(self.state.open_axis);
        self.state.set_snapshot(snapshot);
        self.axis_menu_pending_axis = None;
        self.clear_option_tooltip();
        if let Some(axis) = closing_axis {
            self.begin_axis_menu_close(axis, cx);
        }
        cx.notify();
    }

    /// Replaces the owner-authoritative policy.
    pub fn set_policy(&mut self, policy: Option<SelectPolicy>, cx: &mut Context<Self>) {
        if !self.state.set_policy(policy) {
            self.state.set_local_error(Some(
                "The supplied model policy is not compatible with the catalog.".to_owned(),
            ));
        }
        cx.notify();
    }

    /// Supplies owner-controlled save/error/authority state.
    pub fn set_status(&mut self, status: NativeModelSelectorStatus, cx: &mut Context<Self>) {
        self.state.set_status(status);
        cx.notify();
    }

    pub(super) fn menu_is_interactive(&self) -> bool {
        self.state.is_open() && self.menu_motion.borrow().phase() != PickerMenuPhase::Closing
    }

    pub(super) fn axis_is_interactive(&self, axis: NativePolicyAxis) -> bool {
        self.menu_is_interactive()
            && self.state.is_axis_open(axis)
            && self.axis_menu_motion_axis == Some(axis)
            && self.axis_menu_motion.borrow().phase() != PickerMenuPhase::Closing
    }

    pub(super) fn clear_option_tooltip(&mut self) {
        self.option_tooltip_generation = self.option_tooltip_generation.wrapping_add(1);
        self.option_tooltip.borrow_mut().take();
        self.option_tooltip_task = None;
    }

    pub(super) fn clear_option_tooltip_for(&mut self, key: &str) {
        let matches = self
            .option_tooltip
            .borrow()
            .as_ref()
            .is_some_and(|target| target.key == key);
        if matches {
            self.clear_option_tooltip();
        }
    }

    pub(super) fn begin_option_tooltip(
        &mut self,
        axis: NativePolicyAxis,
        option: &SelectorOption,
        cx: &mut Context<Self>,
    ) {
        let key = option_tooltip_key(axis, &option.id);
        if option_tooltip_text(option.advisory.as_deref(), option.description.as_deref()).is_none()
        {
            self.clear_option_tooltip();
            return;
        }

        self.option_tooltip_generation = self.option_tooltip_generation.wrapping_add(1);
        let generation = self.option_tooltip_generation;
        *self.option_tooltip.borrow_mut() = Some(PickerTooltipTarget {
            key: key.clone(),
            advisory: option.advisory.clone(),
            description: option.description.clone(),
            row_bounds: None,
            visible: false,
        });
        self.option_tooltip_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(PICKER_TOOLTIP_SHOW_DELAY_MS))
                .await;
            let _ = this.update(cx, |selector, cx| {
                if selector.option_tooltip_generation == generation
                    && selector.axis_is_interactive(axis)
                    && selector
                        .option_tooltip
                        .borrow()
                        .as_ref()
                        .is_some_and(|target| target.key == key)
                {
                    if let Some(target) = selector.option_tooltip.borrow_mut().as_mut() {
                        target.visible = true;
                    }
                    selector.option_tooltip_task = None;
                    cx.notify();
                }
            });
        }));
    }

    fn schedule_menu_motion_settle(
        &mut self,
        generation: u64,
        opening: bool,
        cx: &mut Context<Self>,
    ) {
        self.menu_motion_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(PICKER_MENU_MOTION_DURATION_MS))
                .await;
            let _ = this.update(cx, |selector, cx| {
                let settled = if opening {
                    selector.menu_motion.borrow_mut().finish_open(generation)
                } else {
                    selector.menu_motion.borrow_mut().finish_close(generation)
                };
                if settled {
                    selector.menu_motion_task = None;
                    if !opening {
                        selector.menu_bounds.borrow_mut().take();
                        selector.axis_menu_bounds.borrow_mut().take();
                        selector.model_hover.borrow_mut().clear();
                        selector.axis_hover.borrow_mut().clear();
                    }
                    cx.notify();
                }
            });
        }));
    }

    fn schedule_axis_menu_motion_settle(
        &mut self,
        generation: u64,
        opening: bool,
        cx: &mut Context<Self>,
    ) {
        self.axis_menu_motion_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(PICKER_MENU_MOTION_DURATION_MS))
                .await;
            let _ = this.update(cx, |selector, cx| {
                let settled = if opening {
                    selector
                        .axis_menu_motion
                        .borrow_mut()
                        .finish_open(generation)
                } else {
                    selector
                        .axis_menu_motion
                        .borrow_mut()
                        .finish_close(generation)
                };
                if settled {
                    selector.axis_menu_motion_task = None;
                    if !opening {
                        selector.axis_menu_bounds.borrow_mut().take();
                        selector.axis_hover_surface_bounds.borrow_mut().take();
                        selector.axis_hover.borrow_mut().clear();
                        selector.clear_option_tooltip();
                        if let Some(axis) = selector.axis_menu_pending_axis.take() {
                            if selector.state.is_open()
                                && selector.menu_motion.borrow().phase() != PickerMenuPhase::Closing
                            {
                                selector.state.open_axis = Some(axis);
                                selector.begin_axis_menu_open(axis, cx);
                            } else {
                                selector.axis_menu_motion_axis = None;
                            }
                        } else {
                            selector.axis_menu_motion_axis = None;
                        }
                    }
                    cx.notify();
                }
            });
        }));
    }

    fn begin_menu_open(&mut self, cx: &mut Context<Self>) {
        if self.axis_menu_motion.borrow().phase() == PickerMenuPhase::Closing {
            self.axis_menu_motion.borrow_mut().hide();
            self.axis_menu_motion_axis = None;
            self.axis_menu_pending_axis = None;
            self.axis_menu_motion_task = None;
            self.axis_menu_bounds.borrow_mut().take();
            self.axis_hover_surface_bounds.borrow_mut().take();
            self.axis_hover.borrow_mut().clear();
        }
        let generation = self.menu_motion.borrow_mut().begin_open();
        if cx.reduce_motion() {
            self.menu_motion.borrow_mut().finish_open(generation);
            self.menu_motion_task = None;
        } else {
            self.schedule_menu_motion_settle(generation, true, cx);
        }
    }

    fn begin_axis_menu_open(&mut self, axis: NativePolicyAxis, cx: &mut Context<Self>) {
        self.axis_menu_motion_axis = Some(axis);
        self.axis_menu_pending_axis = None;
        let generation = self.axis_menu_motion.borrow_mut().begin_open();
        if cx.reduce_motion() {
            self.axis_menu_motion.borrow_mut().finish_open(generation);
            self.axis_menu_motion_task = None;
        } else {
            self.schedule_axis_menu_motion_settle(generation, true, cx);
        }
    }

    fn begin_axis_menu_close(&mut self, axis: NativePolicyAxis, cx: &mut Context<Self>) {
        self.axis_menu_motion_axis = Some(axis);
        self.clear_option_tooltip();
        self.highlighted_axis_option = None;
        self.axis_menu_bounds.borrow_mut().take();
        self.axis_hover_surface_bounds.borrow_mut().take();
        self.axis_hover.borrow_mut().clear();
        self.axis_scroll.cancel_to(
            f32::from(self.axis_menu_scroll.offset().y),
            f32::from(self.axis_menu_scroll.max_offset().y),
        );
        let generation = self.axis_menu_motion.borrow_mut().begin_close();
        if cx.reduce_motion() {
            self.axis_menu_motion.borrow_mut().finish_close(generation);
            self.axis_menu_motion_task = None;
            if let Some(next_axis) = self.axis_menu_pending_axis.take() {
                if self.state.is_open()
                    && self.menu_motion.borrow().phase() != PickerMenuPhase::Closing
                {
                    self.state.open_axis = Some(next_axis);
                    self.begin_axis_menu_open(next_axis, cx);
                } else {
                    self.axis_menu_motion_axis = None;
                }
            } else {
                self.axis_menu_motion_axis = None;
            }
        } else {
            self.schedule_axis_menu_motion_settle(generation, false, cx);
        }
    }

    fn begin_menu_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let closing_axis = self.axis_menu_motion_axis.or(self.state.open_axis);
        self.state.dismiss();
        self.axis_menu_pending_axis = None;
        self.clear_option_tooltip();
        self.highlighted_axis_option = None;
        self.axis_menu_bounds.borrow_mut().take();
        self.axis_hover_surface_bounds.borrow_mut().take();
        self.model_hover.borrow_mut().clear();
        self.axis_hover.borrow_mut().clear();
        self.model_scroll.cancel_to(
            f32::from(self.menu_scroll.offset().y),
            f32::from(self.menu_scroll.max_offset().y),
        );
        self.axis_scroll.cancel_to(
            f32::from(self.axis_menu_scroll.offset().y),
            f32::from(self.axis_menu_scroll.max_offset().y),
        );
        if let Some(axis) = closing_axis {
            self.begin_axis_menu_close(axis, cx);
        }
        let generation = self.menu_motion.borrow_mut().begin_close();
        if cx.reduce_motion() {
            self.menu_motion.borrow_mut().finish_close(generation);
            self.menu_motion_task = None;
        } else {
            self.schedule_menu_motion_settle(generation, false, cx);
        }
        window.focus(&self.trigger_focus, cx);
    }

    pub(super) fn handle_model_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_picker_scroll(event, window, cx, true);
    }

    pub(super) fn handle_axis_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.handle_picker_scroll(event, window, cx, false);
    }

    fn handle_picker_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
        model_list: bool,
    ) {
        // The content wrapper is the first bubble listener inside the GPUI
        // scroll container. Stopping propagation there prevents its default
        // immediate offset update from being applied a second time.
        cx.stop_propagation();
        if !self.menu_is_interactive() || (!model_list && self.state.open_axis.is_none()) {
            return;
        }

        let delta = event.delta.pixel_delta(window.line_height()).y;
        let delta = f32::from(delta);
        if delta.abs() <= f32::EPSILON {
            return;
        }
        if !model_list {
            self.clear_option_tooltip();
        }

        let handle = if model_list {
            self.menu_scroll.clone()
        } else {
            // Keep policy-menu scrolling independent so a future long option
            // list cannot fight model-list inertia.
            self.axis_menu_scroll.clone()
        };
        let offset = handle.offset();
        let current = f32::from(offset.y);
        let maximum = f32::from(handle.max_offset().y).max(0.0);

        if event.delta.precise() || cx.reduce_motion() {
            let next = (current + delta).clamp(-maximum, 0.0);
            handle.set_offset(point(offset.x, px(next)));
            if model_list {
                self.model_scroll.cancel_to(next, maximum);
            } else {
                self.axis_scroll.cancel_to(next, maximum);
            }
            cx.notify();
            return;
        }

        if model_list {
            self.model_scroll.push(current, delta, maximum);
            self.schedule_model_scroll_frame(window, cx);
        } else {
            self.axis_scroll.push(current, delta, maximum);
            self.schedule_axis_scroll_frame(window, cx);
        }
    }

    fn schedule_model_scroll_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.model_scroll.active() || self.model_scroll_frame_scheduled {
            return;
        }
        self.model_scroll_frame_scheduled = true;
        cx.on_next_frame(window, |selector, window, cx| {
            selector.advance_model_scroll(window, cx);
        });
    }

    fn schedule_axis_scroll_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.axis_scroll.active() || self.axis_scroll_frame_scheduled {
            return;
        }
        self.axis_scroll_frame_scheduled = true;
        cx.on_next_frame(window, |selector, window, cx| {
            selector.advance_axis_scroll(window, cx);
        });
    }

    fn advance_model_scroll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.model_scroll_frame_scheduled = false;
        let offset = self.menu_scroll.offset();
        let maximum = f32::from(self.menu_scroll.max_offset().y).max(0.0);
        if let Some(next) = self.model_scroll.step(f32::from(offset.y), maximum) {
            self.menu_scroll.set_offset(point(offset.x, px(next)));
            cx.notify();
        }
        self.schedule_model_scroll_frame(window, cx);
    }

    fn advance_axis_scroll(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.axis_scroll_frame_scheduled = false;
        let offset = self.axis_menu_scroll.offset();
        let maximum = f32::from(self.axis_menu_scroll.max_offset().y).max(0.0);
        if let Some(next) = self.axis_scroll.step(f32::from(offset.y), maximum) {
            self.axis_menu_scroll.set_offset(point(offset.x, px(next)));
            cx.notify();
        }
        self.schedule_axis_scroll_frame(window, cx);
    }

    pub(super) fn handle_trigger_click(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.is_open() {
            self.begin_menu_close(window, cx);
        } else {
            self.state.press_trigger();
            self.model_hover.borrow_mut().clear();
            self.axis_hover.borrow_mut().clear();
            self.begin_menu_open(cx);
            self.sync_focus_after_transition(window, cx);
        }
        cx.notify();
    }

    pub(super) fn handle_menu_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(key) = selector_key_from_event(event) else {
            return;
        };
        if !self.menu_is_interactive() {
            cx.stop_propagation();
            return;
        }
        if let Some(axis) = self.state.open_axis {
            let options = self
                .axis_options(axis, &self.preview_view())
                .into_iter()
                .filter(|option| !option.disabled)
                .collect::<Vec<_>>();
            let current = options
                .iter()
                .position(|option| Some(&option.id) == self.highlighted_axis_option.as_ref())
                .or_else(|| options.iter().position(|option| option.selected))
                .unwrap_or(0);
            let next = match key {
                NativeModelSelectorKey::ArrowDown if !options.is_empty() => {
                    Some((current + 1) % options.len())
                }
                NativeModelSelectorKey::ArrowUp if !options.is_empty() => {
                    Some((current + options.len() - 1) % options.len())
                }
                NativeModelSelectorKey::Home if !options.is_empty() => Some(0),
                NativeModelSelectorKey::End if !options.is_empty() => Some(options.len() - 1),
                NativeModelSelectorKey::Enter | NativeModelSelectorKey::Space => {
                    if let Some(option) = options.get(current) {
                        self.choose_axis(axis, &option.id, window, cx);
                    }
                    None
                }
                NativeModelSelectorKey::Escape | NativeModelSelectorKey::Tab => {
                    self.state.open_axis = None;
                    self.begin_axis_menu_close(axis, cx);
                    None
                }
                _ => None,
            };
            if let Some(index) = next {
                self.highlighted_axis_option = Some(options[index].id.clone());
                self.axis_hover
                    .borrow_mut()
                    .set_active(options[index].id.clone());
                self.begin_option_tooltip(axis, &options[index], cx);
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        let was_open = self.state.is_open();
        let emitted = self.state.handle_key(key);
        if let Some(event) = emitted {
            cx.emit(event);
        }
        if was_open && !self.state.is_open() {
            self.begin_menu_close(window, cx);
        } else if self.state.is_open() {
            if matches!(
                key,
                NativeModelSelectorKey::ArrowDown
                    | NativeModelSelectorKey::ArrowUp
                    | NativeModelSelectorKey::Home
                    | NativeModelSelectorKey::End
            ) && let Some(model_id) = self.state.highlighted_model_id().map(str::to_owned)
            {
                self.model_hover.borrow_mut().set_active(model_id);
            }
            self.reveal_highlight();
        }
        cx.notify();
    }

    pub(super) fn handle_outside_press(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.state.is_open()
            || (self.state.open_axis.is_some()
                && self
                    .axis_menu_bounds
                    .borrow()
                    .as_ref()
                    .is_some_and(|bounds| bounds.contains(&event.position)))
            || self
                .menu_bounds
                .borrow()
                .as_ref()
                .is_some_and(|bounds| bounds.contains(&event.position))
            || self
                .trigger_bounds
                .borrow()
                .as_ref()
                .is_some_and(|bounds| bounds.contains(&event.position))
        {
            return;
        }
        self.begin_menu_close(window, cx);
        cx.notify();
    }

    pub(super) fn choose_model(
        &mut self,
        model_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.menu_is_interactive() {
            return;
        }
        let closing_axis = self.axis_menu_motion_axis.or(self.state.open_axis);
        let emitted = self.state.select_model(model_id);
        if let Some(event) = emitted {
            cx.emit(event);
        }
        if let Some(axis) = closing_axis.filter(|_| self.state.open_axis.is_none()) {
            self.begin_axis_menu_close(axis, cx);
        }
        if self.state.is_open() {
            self.sync_focus_after_transition(window, cx);
        } else {
            self.begin_menu_close(window, cx);
        }
        cx.notify();
    }

    pub(super) fn toggle_favorite(&mut self, model_id: &str, cx: &mut Context<Self>) {
        if !self.menu_is_interactive() {
            return;
        }
        if let Some(event) = self.state.toggle_favorite(model_id) {
            cx.emit(event);
        }
        cx.notify();
    }

    pub(super) fn choose_axis(
        &mut self,
        axis: NativePolicyAxis,
        option_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.axis_is_interactive(axis) {
            return;
        }
        let axis_was_open = self.state.is_axis_open(axis);
        match self.state.choose_option(axis, option_id) {
            Ok(Some(event)) => cx.emit(event),
            Ok(None) => {}
            Err(error) => self.state.set_local_error(Some(error.to_string())),
        }
        if axis_was_open && !self.state.is_axis_open(axis) {
            self.begin_axis_menu_close(axis, cx);
        }
        if !self.state.is_open() {
            self.begin_menu_close(window, cx);
        }
        cx.notify();
    }

    pub(super) fn toggle_axis(
        &mut self,
        axis: NativePolicyAxis,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.menu_is_interactive() {
            return;
        }
        self.highlighted_axis_option = None;
        self.axis_menu_bounds.borrow_mut().take();
        self.axis_hover_surface_bounds.borrow_mut().take();
        self.clear_option_tooltip();
        self.axis_hover.borrow_mut().clear();
        self.axis_menu_scroll.set_offset(point(px(0.0), px(0.0)));
        self.axis_scroll.cancel_to(0.0, 0.0);
        if let Some(open_axis) = self.state.open_axis {
            if open_axis == axis {
                self.state.open_axis = None;
                self.axis_menu_pending_axis = None;
                self.begin_axis_menu_close(axis, cx);
            } else {
                self.state.open_axis = None;
                self.axis_menu_pending_axis = Some(axis);
                self.begin_axis_menu_close(open_axis, cx);
            }
        } else {
            self.state.toggle_axis(axis);
            self.begin_axis_menu_open(axis, cx);
        }
        cx.notify();
    }

    pub(super) fn switch_engine(&mut self, engine_id: String, cx: &mut Context<Self>) {
        if !self.menu_is_interactive() {
            return;
        }
        let closing_axis = self.axis_menu_motion_axis.or(self.state.open_axis);
        self.state.set_active_engine(engine_id);
        self.axis_menu_pending_axis = None;
        self.clear_option_tooltip();
        if let Some(axis) = closing_axis {
            self.begin_axis_menu_close(axis, cx);
        }
        self.model_hover.borrow_mut().clear();
        self.menu_scroll.set_offset(point(px(0.0), px(0.0)));
        self.model_scroll.cancel_to(0.0, 0.0);
        cx.notify();
    }

    fn sync_focus_after_transition(&mut self, window: &mut Window, cx: &mut App) {
        if self.state.is_open() {
            window.focus(&self.menu_focus, cx);
            self.reveal_highlight();
        } else {
            window.focus(&self.trigger_focus, cx);
        }
    }

    fn reveal_highlight(&mut self) {
        let groups = self.state.model_groups();
        let headers = groups.len() > 1 || groups.first().is_some_and(|group| group.id != "default");
        let mut index = 0;
        for group in groups.iter() {
            index += usize::from(headers);
            if self.state.group_collapsed(&group.id) {
                continue;
            }
            for model in &group.models {
                if Some(model.id.as_str()) == self.state.highlighted_model_id() {
                    self.virtual_model_scroll
                        .scroll_to_item(index, gpui::ScrollStrategy::Nearest);
                    return;
                }
                index += 1;
            }
        }
    }
}

pub(super) fn option_tooltip_text(
    advisory: Option<&str>,
    description: Option<&str>,
) -> Option<String> {
    let mut text = String::new();
    if let Some(advisory) = advisory.filter(|advisory| !advisory.is_empty()) {
        text.push_str(advisory);
    }
    if let Some(description) = description.filter(|description| !description.is_empty()) {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(description);
    }
    (!text.is_empty()).then_some(text)
}

pub(super) fn option_tooltip_key(axis: NativePolicyAxis, option_id: &str) -> String {
    format!("{axis:?}:{option_id}")
}

fn selector_key_from_event(event: &KeyDownEvent) -> Option<NativeModelSelectorKey> {
    let key = event.keystroke.key.as_str();
    let modified = event.keystroke.modifiers.modified();
    match key {
        "down" if !modified => Some(NativeModelSelectorKey::ArrowDown),
        "up" if !modified => Some(NativeModelSelectorKey::ArrowUp),
        "home" if !modified => Some(NativeModelSelectorKey::Home),
        "end" if !modified => Some(NativeModelSelectorKey::End),
        "enter" if !modified => Some(NativeModelSelectorKey::Enter),
        "space" if !modified => Some(NativeModelSelectorKey::Space),
        "escape" => Some(NativeModelSelectorKey::Escape),
        "tab" => Some(NativeModelSelectorKey::Tab),
        _ => None,
    }
}
