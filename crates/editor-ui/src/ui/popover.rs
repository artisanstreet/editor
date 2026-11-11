use aqua_ui::{
    element::{Box as UiBox, Element, Key, Scroll},
    style::{Axis, Length, Style},
};

use super::{
    glass::{GlassStrength, glass_card_shadows, glass_layers},
    theme::{ArtisanTheme, Elevation, RadiusStep, RadiusTokens},
};

pub struct PopoverPanel {
    id: Key,
    theme: ArtisanTheme,
    content: Element,
    viewport_width: f32,
    viewport_height: f32,
    menu_padding: f32,
    glass: Option<GlassStrength>,
    select_menu: bool,
}

impl PopoverPanel {
    pub fn new(
        id: impl Into<Key>,
        theme: ArtisanTheme,
        content: Element,
        viewport_width: f32,
        viewport_height: f32,
    ) -> Self {
        Self {
            id: id.into(),
            theme,
            content,
            viewport_width,
            viewport_height,
            menu_padding: 6.0,
            glass: None,
            select_menu: false,
        }
    }

    pub const fn menu_padding(mut self, padding: f32) -> Self {
        self.menu_padding = padding;
        self
    }
    pub const fn quiet_glass_menu(mut self) -> Self {
        self.glass = Some(GlassStrength::Quiet);
        self
    }
    pub const fn strong_glass_menu(mut self) -> Self {
        self.glass = Some(GlassStrength::Strong);
        self
    }
    pub const fn select_menu(mut self) -> Self {
        self.select_menu = true;
        self
    }

    pub fn build(self) -> Element {
        let radius = RadiusTokens::value(if self.select_menu || self.glass.is_some() {
            RadiusStep::X2l
        } else {
            RadiusStep::Lg
        });
        let mut style = Style::new()
            .axis(Axis::Column)
            .max_width(Length::Px((self.viewport_width - 16.0).max(1.0)))
            .max_height(Length::Px((self.viewport_height - 16.0).max(1.0)))
            .radius(Length::Px(radius))
            .gpui_source_text_color(self.theme.colors.text.to_reference_text_color());
        if self.select_menu {
            let mut shadows = vec![self.theme.elevation.hairline.into()];
            shadows.extend(
                self.theme
                    .elevation
                    .drops(Elevation::Overlay)
                    .iter()
                    .copied()
                    .map(aqua_ui::style::GpuiShadowLayer::from),
            );
            style = style.gpui_shadows(shadows);
        } else if self.glass.is_none() {
            style = style.gpui_border(
                aqua_core::Insets::uniform(1.0),
                self.theme.colors.line.to_reference_paint(),
                false,
            );
        }
        if self.glass.is_some() {
            style = style.gpui_shadows(
                glass_card_shadows(&self.theme, Elevation::Overlay)
                    .into_iter()
                    .map(aqua_ui::style::GpuiShadowLayer::from)
                    .collect(),
            );
        } else {
            style = style.gpui_background(aqua_core::GpuiBackground::Solid(
                self.theme.colors.raised.to_reference_paint(),
            ));
        }
        let scroll = Scroll::new()
            .style(
                Style::new()
                    .axis(Axis::Column)
                    .padding(Length::Px(if self.select_menu {
                        0.0
                    } else {
                        self.menu_padding
                    }))
                    .max_width(Length::Px((self.viewport_width - 16.0).max(1.0)))
                    .max_height(Length::Px((self.viewport_height - 16.0).max(1.0))),
            )
            .child(self.content)
            .build();
        if self.glass == Some(GlassStrength::Strong) {
            return crate::ui::chrome::account::menu_surface(&self.theme, vec![scroll])
                .style(
                    Style::new()
                        .max_width(Length::Px((self.viewport_width - 16.0).max(1.0)))
                        .max_height(Length::Px((self.viewport_height - 16.0).max(1.0))),
                )
                .with_key(self.id);
        }
        let mut root = UiBox::new()
            .style(style)
            .clip(self.select_menu || self.glass.is_some());
        if let Some(strength) = self.glass {
            root = root.child(glass_layers(&self.theme, strength, radius));
        }
        root.child(scroll).build().with_key(self.id)
    }
}

pub struct Popover {
    id: Key,
    label: String,
    position: aqua_core::Point,
    theme: ArtisanTheme,
    content: Element,
    viewport: aqua_core::Size,
    dismiss: std::rc::Rc<dyn Fn()>,
    menu_padding: f32,
    glass: Option<GlassStrength>,
}

impl Popover {
    pub fn new(
        id: impl Into<Key>,
        label: impl Into<String>,
        position: aqua_core::Point,
        theme: ArtisanTheme,
        content: Element,
        viewport: aqua_core::Size,
        dismiss: impl Fn() + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            position,
            theme,
            content,
            viewport,
            dismiss: std::rc::Rc::new(dismiss),
            menu_padding: 6.0,
            glass: None,
        }
    }
    pub const fn menu_padding(mut self, padding: f32) -> Self {
        self.menu_padding = padding;
        self
    }
    pub const fn quiet_glass_menu(mut self) -> Self {
        self.glass = Some(GlassStrength::Quiet);
        self
    }
    pub const fn strong_glass_menu(mut self) -> Self {
        self.glass = Some(GlassStrength::Strong);
        self
    }
    pub fn into_parts(self) -> (Element, aqua_ui::popup::PopupOptions) {
        use aqua_core::{InputEvent, Key as InputKey, NamedKey, Role};
        use aqua_ui::input::{EventPhase, InputKind, InputResponse, RoutedInput};
        let mut panel = PopoverPanel::new(
            self.id,
            self.theme,
            self.content,
            self.viewport.w,
            self.viewport.h,
        )
        .menu_padding(self.menu_padding);
        panel.glass = self.glass;
        let tab_dismiss = self.dismiss.clone();
        let panel = panel.build().role(Role::Menu).aria_label(self.label).debug_selector(crate::ui::menu_keys::POPOVER_MENU).on_input(InputKind::KeyDown, EventPhase::Bubble, move |event| {
            if matches!(&event.input, RoutedInput::Native(InputEvent::KeyDown(key)) if key.key == InputKey::Named(NamedKey::Tab)) {
                tab_dismiss();
                InputResponse { stop_propagation: true, ..InputResponse::handled() }
            } else { InputResponse::default() }
        });
        let mut options = aqua_ui::popup::PopupOptions::menu().at(self.position);
        options.focus = aqua_ui::popup::PopupFocus::Root;
        let dismiss = self.dismiss;
        options.on_close = Some(std::rc::Rc::new(move |_| dismiss()));
        (panel, options)
    }
}

pub struct PopoverHost {
    overlay: aqua_ui::overlay::OverlayId,
    pending: std::rc::Rc<std::cell::Cell<bool>>,
}

pub struct PickerPopupHost {
    overlay: aqua_ui::overlay::OverlayId,
}

impl PickerPopupHost {
    pub fn root(&self, app: &aqua::UiApp) -> Option<aqua_core::NodeId> {
        app.host().popup_root(self.overlay)
    }

    pub fn open_above(
        app: &mut aqua::UiApp,
        owner: aqua_core::NodeId,
        content: Element,
        gap: f32,
        focus: aqua_ui::popup::PopupFocus,
        dismiss: aqua_ui::overlay::DismissPolicy,
        on_close: impl Fn(aqua_ui::overlay::CloseReason) + 'static,
    ) -> Result<Self, String> {
        let mut options = aqua_ui::popup::PopupOptions::menu();
        options.focus = focus;
        options.dismiss = dismiss;
        options.placement.preferred = aqua_ui::overlay::Side::Above;
        options.placement.align = aqua_ui::overlay::Align::Start;
        options.placement.gap = gap;
        options.placement.flip = false;
        options.on_close = Some(std::rc::Rc::new(on_close));
        let overlay = app
            .open_popup_measured(owner, content, options)
            .map_err(|error| error.to_string())?;
        Ok(Self { overlay })
    }

    pub fn refresh(&self, app: &mut aqua::UiApp, content: Element) -> Result<(), String> {
        app.refresh_popup(self.overlay, content)
            .map_err(|error| error.to_string())
    }

    pub fn close(&self, app: &mut aqua::UiApp) {
        app.close_popup(self.overlay);
    }

    pub fn is_open(&self, app: &aqua::UiApp) -> bool {
        app.host().is_open(self.overlay)
    }
}

#[derive(Clone, Copy)]
pub enum PickerEntrance {
    Slide,
    Scale { origin: aqua_core::Point },
}

pub struct AnimatedPickerHost {
    popup: PickerPopupHost,
    factory: Option<std::rc::Rc<dyn Fn() -> Element>>,
    motion: super::picker_motion::MenuMotion,
    entrance: PickerEntrance,
    scale: f32,
    from_scale: f32,
    open_duration: std::time::Duration,
    close_duration: std::time::Duration,
    flight: Option<(u64, f64)>,
    now: f64,
}

impl AnimatedPickerHost {
    pub fn set_reduced_motion(&mut self, reduced: bool) {
        self.open_duration = if reduced {
            std::time::Duration::ZERO
        } else {
            super::picker_motion::DROPDOWN_OPEN
        };
        self.close_duration = if reduced {
            std::time::Duration::ZERO
        } else {
            super::picker_motion::DROPDOWN_CLOSE
        };
    }

    pub fn overlay(&self) -> aqua_ui::overlay::OverlayId {
        self.popup.overlay
    }

    pub fn root(&self, app: &aqua::UiApp) -> Option<aqua_core::NodeId> {
        self.popup.root(app)
    }

    pub fn open_above(
        app: &mut aqua::UiApp,
        owner: aqua_core::NodeId,
        factory: impl Fn() -> Element + 'static,
        gap: f32,
        focus: aqua_ui::popup::PopupFocus,
        entrance: PickerEntrance,
        open_duration: std::time::Duration,
        close_duration: std::time::Duration,
        now: f64,
        on_close: impl Fn(aqua_ui::overlay::CloseReason) + 'static,
    ) -> Result<Self, String> {
        let factory: std::rc::Rc<dyn Fn() -> Element> = std::rc::Rc::new(factory);
        let content = factory();
        let mut host = Self::open_content(
            app,
            owner,
            content,
            gap,
            focus,
            entrance,
            open_duration,
            close_duration,
            now,
            on_close,
        )?;
        host.factory = Some(factory);
        Ok(host)
    }

    pub fn open_content(
        app: &mut aqua::UiApp,
        owner: aqua_core::NodeId,
        content: Element,
        gap: f32,
        focus: aqua_ui::popup::PopupFocus,
        entrance: PickerEntrance,
        open_duration: std::time::Duration,
        close_duration: std::time::Duration,
        now: f64,
        on_close: impl Fn(aqua_ui::overlay::CloseReason) + 'static,
    ) -> Result<Self, String> {
        let popup = PickerPopupHost::open_above(
            app,
            owner,
            content,
            gap,
            focus,
            aqua_ui::overlay::DismissPolicy::MANUAL,
            on_close,
        )?;
        let mut motion = super::picker_motion::MenuMotion::default();
        motion.begin_open();
        let mut host = Self {
            popup,
            factory: None,
            motion,
            entrance,
            open_duration,
            close_duration,
            scale: super::picker_motion::DROPDOWN_PRE_SCALE,
            from_scale: super::picker_motion::DROPDOWN_PRE_SCALE,
            flight: None,
            now,
        };
        host.advance(app, now)?;
        Ok(host)
    }

    pub fn open_options(
        app: &mut aqua::UiApp,
        owner: aqua_core::NodeId,
        content: Element,
        mut options: aqua_ui::popup::PopupOptions,
        now: f64,
    ) -> Result<Self, String> {
        let origin = match options.placement.preferred {
            aqua_ui::overlay::Side::Above => aqua_core::Point::new(0.0, 1.0),
            aqua_ui::overlay::Side::Before => aqua_core::Point::new(1.0, 0.0),
            _ => aqua_core::Point::new(0.0, 0.0),
        };
        options.dismiss = aqua_ui::overlay::DismissPolicy::MANUAL;
        let overlay = app
            .open_popup_measured(owner, content, options)
            .map_err(|error| error.to_string())?;
        let mut motion = super::picker_motion::MenuMotion::default();
        motion.begin_open();
        let mut host = Self {
            popup: PickerPopupHost { overlay },
            factory: None,
            motion,
            entrance: PickerEntrance::Scale { origin },
            open_duration: super::picker_motion::DROPDOWN_OPEN,
            close_duration: super::picker_motion::DROPDOWN_CLOSE,
            scale: super::picker_motion::DROPDOWN_PRE_SCALE,
            from_scale: super::picker_motion::DROPDOWN_PRE_SCALE,
            flight: None,
            now,
        };
        host.advance(app, now)?;
        Ok(host)
    }

    pub fn dispose(mut self, app: &mut aqua::UiApp) {
        self.flight.take();
        self.popup.close(app);
    }

    pub fn begin_close(&mut self, app: &mut aqua::UiApp, now: f64) -> Result<(), String> {
        if self.motion.phase() != super::picker_motion::MenuPhase::Closing {
            self.motion.begin_close();
        }
        self.advance(app, now)
    }

    pub fn begin_open(&mut self, app: &mut aqua::UiApp, now: f64) -> Result<(), String> {
        self.motion.begin_open();
        self.advance(app, now)
    }

    pub fn is_closing(&self) -> bool {
        self.motion.phase() == super::picker_motion::MenuPhase::Closing
    }

    pub fn advance(&mut self, app: &mut aqua::UiApp, now: f64) -> Result<(), String> {
        use super::picker_motion::MenuPhase;
        self.now = now;
        let Some((_, _, _, _, generation)) = self.motion.transition() else {
            return Ok(());
        };
        let closing = self.motion.phase() == MenuPhase::Closing;
        if self
            .flight
            .as_ref()
            .is_none_or(|(old, _)| *old != generation)
        {
            self.from_scale = self.scale;
            self.flight = Some((generation, now));
        }
        let Some((_, started)) = self.flight else {
            return Ok(());
        };
        let duration = if closing {
            self.close_duration
        } else {
            self.open_duration
        }
        .as_secs_f64();
        let reduced =
            app.host().motion().reduced_motion() == aqua_ui::motion::ReducedMotion::Reduce;
        let progress = if duration > 0.0 && !reduced {
            ((now - started) / duration).clamp(0.0, 1.0) as f32
        } else {
            1.0
        };
        let settled = progress >= 1.0;
        let eased = super::picker_motion::smooth_out(progress);
        let target_scale = if closing {
            super::picker_motion::DROPDOWN_CLOSING_SCALE
        } else {
            1.0
        };
        self.scale = (target_scale - self.from_scale).mul_add(eased, self.from_scale);
        self.motion.apply_progress(generation, eased);
        self.apply_frame(app)?;
        if settled {
            self.flight = None;
            if closing {
                self.motion.finish_close(generation);
                self.popup.close(app);
            } else {
                self.motion.finish_open(generation);
            }
        }
        Ok(())
    }

    fn frame_values(&self, app: &aqua::UiApp) -> (f32, aqua_core::Transform2D) {
        let (opacity, offset) = self.motion.frame();
        let transform = match self.entrance {
            PickerEntrance::Slide => aqua_core::Transform2D::translate(0.0, offset),
            PickerEntrance::Scale { origin } => {
                let scale = self.scale;
                let bounds = app
                    .host()
                    .service()
                    .bounds(self.popup.overlay)
                    .unwrap_or(aqua_core::Rect::ZERO);
                let pivot = aqua_core::Point::new(bounds.w * origin.x, bounds.h * origin.y);
                aqua_core::Transform2D::translate(-pivot.x, -pivot.y)
                    .then(aqua_core::Transform2D::scale(scale, scale))
                    .then(aqua_core::Transform2D::translate(pivot.x, pivot.y))
            }
        };
        (opacity, transform)
    }

    fn apply_frame(&self, app: &mut aqua::UiApp) -> Result<(), String> {
        let root = app
            .host()
            .popup_root(self.popup.overlay)
            .ok_or_else(|| "picker popup is closed".to_owned())?;
        let mut paint = {
            let node = app
                .tree()
                .get(root)
                .ok_or_else(|| "picker popup is closed".to_owned())?;
            match &node.kind {
                aqua_ui::tree::NodeKind::Box(content) => content.paint().clone(),
                _ => node.paint().clone(),
            }
        };
        let (opacity, transform) = self.frame_values(app);
        paint.opacity = Some(opacity);
        paint.transform = Some(transform);
        app.tree_mut().set_paint(root, paint);
        Ok(())
    }

    pub fn refresh(&mut self, app: &mut aqua::UiApp) -> Result<(), String> {
        let factory = self
            .factory
            .as_ref()
            .ok_or_else(|| "picker popup has no content factory".to_owned())?;
        let content = factory();
        self.refresh_content(app, content)
    }

    pub fn refresh_content(
        &mut self,
        app: &mut aqua::UiApp,
        content: Element,
    ) -> Result<(), String> {
        let (opacity, transform) = self.frame_values(app);
        self.popup.refresh(
            app,
            content.style(Style::new().opacity(opacity).transform(transform)),
        )
    }

    pub fn step(
        &mut self,
        app: &mut aqua::UiApp,
        input: aqua::RuntimeInput,
    ) -> Result<aqua::RuntimeOutput, String> {
        if let aqua::RuntimeInput::Shell(aqua_protocol::ShellEvent::Input { event, .. }) = &input {
            self.request_dismissal(app, event)?;
        }
        if let aqua::RuntimeInput::Shell(aqua_protocol::ShellEvent::Frame {
            presentation: now,
            ..
        })
        | aqua::RuntimeInput::Tick(now) = &input
        {
            self.advance(app, now.as_secs_f64())?;
        }
        app.drive(input);
        let output = app.take_pending();
        if self.popup.is_open(app) {
            self.advance(app, self.now)?;
        }
        Ok(output)
    }

    pub fn request_dismissal(
        &mut self,
        app: &mut aqua::UiApp,
        event: &aqua_core::InputEvent,
    ) -> Result<bool, String> {
        if app.host().service().topmost() != Some(self.popup.overlay) {
            return Ok(false);
        }
        let dismiss = match event {
            aqua_core::InputEvent::PointerDown(pointer, aqua_core::PointerButton::Primary) => app
                .host()
                .service()
                .bounds(self.popup.overlay)
                .is_some_and(|bounds| !bounds.contains(pointer.position)),
            aqua_core::InputEvent::KeyDown(key) => {
                key.key == aqua_core::Key::Named(aqua_core::NamedKey::Escape)
            }
            _ => false,
        };
        if dismiss {
            self.begin_close(app, self.now)?;
        }
        Ok(dismiss)
    }

    pub fn is_open(&self, app: &aqua::UiApp) -> bool {
        self.popup.is_open(app)
    }

    pub fn is_animating(&self, _app: &aqua::UiApp) -> bool {
        self.motion.transition().is_some() && self.flight.is_some()
    }
}

impl PopoverHost {
    pub fn open(
        app: &mut aqua::UiApp,
        owner: aqua_core::NodeId,
        mut popover: Popover,
    ) -> Result<Self, String> {
        let callback = popover.dismiss.clone();
        let pending = std::rc::Rc::new(std::cell::Cell::new(false));
        let queue = pending.clone();
        popover.dismiss = std::rc::Rc::new(move || queue.set(true));
        let (panel, mut options) = popover.into_parts();
        options.on_close = Some(std::rc::Rc::new(move |_| callback()));
        let overlay = app
            .open_popup_measured(owner, panel, options)
            .map_err(|error| error.to_string())?;
        Ok(Self { overlay, pending })
    }

    pub fn sync(&self, app: &mut aqua::UiApp) {
        if self.pending.replace(false) {
            app.close_popup(self.overlay);
        }
    }

    pub fn handle_input(&self, app: &mut aqua::UiApp, event: &aqua_core::InputEvent) -> bool {
        let handled = app.handle_input(event);
        self.sync(app);
        handled
    }

    pub fn is_open(&self, app: &aqua::UiApp) -> bool {
        app.host().is_open(self.overlay)
    }
}
