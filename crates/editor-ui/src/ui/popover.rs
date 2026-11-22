use aqua_ui::{
    element::{Element, Key, Scroll},
    style::{Axis, Length, Style},
};

use super::{kit::Surface, theme::ArtisanTheme};
use std::rc::Rc;

const VIEWPORT_MARGIN: f32 = 16.0;

#[derive(Debug, thiserror::Error)]
pub enum PopoverError {
    #[error("open the picker popup")]
    Open(#[source] aqua_runtime::PopupError),
    #[error("refresh the picker popup {overlay:?}")]
    Refresh {
        overlay: aqua_ui::overlay::OverlayId,
        #[source]
        source: aqua_runtime::PopupError,
    },
    #[error("picker popup {overlay:?} is closed")]
    Closed {
        overlay: aqua_ui::overlay::OverlayId,
    },
    #[error("picker popup {overlay:?} has no content factory")]
    NoFactory {
        overlay: aqua_ui::overlay::OverlayId,
    },
}

struct PopoverPanel {
    id: Key,
    theme: ArtisanTheme,
    content: Element,
    viewport_width: f32,
    viewport_height: f32,
}

impl PopoverPanel {
    #[expect(
        clippy::large_types_passed_by_value,
        reason = "the popover panel stores this theme copy and paints from it when built"
    )]
    fn new(
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
        }
    }

    fn build(self) -> Element {
        let width = Length::Px((self.viewport_width - VIEWPORT_MARGIN).max(1.0));
        let height = Length::Px(
            2.0f32
                .mul_add(
                    -Surface::Menu.padding_px(),
                    self.viewport_height - VIEWPORT_MARGIN,
                )
                .max(1.0),
        );
        let scroll = Scroll::new()
            .style(
                Style::new()
                    .axis(Axis::Column)
                    .max_width(width)
                    .max_height(height),
            )
            .child(self.content)
            .build();
        Surface::Menu
            .build(&self.theme, vec![scroll])
            .style(
                Style::new()
                    .max_width(width)
                    .gpui_source_text_color(self.theme.colors.text.to_reference_text_color()),
            )
            .with_key(self.id)
    }
}

pub struct Popover {
    id: Key,
    label: String,
    position: aqua_core::Point,
    theme: ArtisanTheme,
    content: Element,
    viewport: aqua_core::Size,
    dismiss: Rc<dyn Fn()>,
}

impl std::fmt::Debug for Popover {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Popover").finish_non_exhaustive()
    }
}

impl Popover {
    #[expect(
        clippy::large_types_passed_by_value,
        reason = "the popover stores this theme copy and paints from it when built"
    )]
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
            dismiss: Rc::new(dismiss),
        }
    }
    pub fn into_parts(self) -> (Element, aqua_ui::popup::PopupOptions) {
        use aqua_core::{InputEvent, Key as InputKey, NamedKey, Role};
        use aqua_ui::input::{EventPhase, InputKind, InputResponse, RoutedInput};
        let panel = PopoverPanel::new(
            self.id,
            self.theme,
            self.content,
            self.viewport.w,
            self.viewport.h,
        );
        let tab_dismiss = Rc::clone(&self.dismiss);
        let panel = panel.build().role(Role::Menu).aria_label(self.label).debug_selector(crate::ui::menu_keys::POPOVER_MENU).on_input(InputKind::KeyDown, EventPhase::Bubble, move |event| {
            if matches!(&event.input, RoutedInput::Native(InputEvent::KeyDown(key)) if key.key == InputKey::Named(NamedKey::Tab)) {
                tab_dismiss();
                InputResponse { stop_propagation: true, ..InputResponse::handled() }
            } else { InputResponse::default() }
        });
        let mut options = aqua_ui::popup::PopupOptions::menu().at(self.position);
        options.focus = aqua_ui::popup::PopupFocus::Root;
        let dismiss = self.dismiss;
        options.on_close = Some(Rc::new(move |_| dismiss()));
        (panel, options)
    }
}

#[derive(Debug)]
pub struct PickerPopupHost {
    overlay: aqua_ui::overlay::OverlayId,
}

impl PickerPopupHost {
    fn root(&self, app: &aqua::UiApp) -> Option<aqua_core::NodeId> {
        app.host().popup_root(self.overlay)
    }

    fn open_above(
        app: &mut aqua::UiApp,
        owner: aqua_core::NodeId,
        content: Element,
        gap: f32,
        focus: aqua_ui::popup::PopupFocus,
        dismiss: aqua_ui::overlay::DismissPolicy,
        on_close: impl Fn(aqua_ui::overlay::CloseReason) + 'static,
    ) -> Result<Self, PopoverError> {
        let mut options = aqua_ui::popup::PopupOptions::menu();
        options.focus = focus;
        options.dismiss = dismiss;
        options.placement.preferred = aqua_ui::overlay::Side::Above;
        options.placement.align = aqua_ui::overlay::Align::Start;
        options.placement.gap = gap;
        options.placement.flip = false;
        options.on_close = Some(Rc::new(on_close));
        let overlay = app
            .open_popup_measured(owner, content, options)
            .map_err(PopoverError::Open)?;
        Ok(Self { overlay })
    }

    fn refresh(&self, app: &mut aqua::UiApp, content: Element) -> Result<(), PopoverError> {
        app.refresh_popup(self.overlay, content)
            .map_err(|source| PopoverError::Refresh {
                overlay: self.overlay,
                source,
            })
    }

    fn close(&self, app: &mut aqua::UiApp) {
        app.close_popup(self.overlay);
    }

    fn is_open(&self, app: &aqua::UiApp) -> bool {
        app.host().is_open(self.overlay)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum PickerEntrance {
    Slide,
    Scale { origin: aqua_core::Point },
}

pub struct AnimatedPickerHost {
    popup: PickerPopupHost,
    factory: Option<Rc<dyn Fn() -> Element>>,
    motion: super::picker_motion::MenuMotion,
    entrance: PickerEntrance,
    scale: f32,
    from_scale: f32,
    open_duration: std::time::Duration,
    close_duration: std::time::Duration,
    flight: Option<(u64, f64)>,
    now: f64,
}

impl std::fmt::Debug for AnimatedPickerHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnimatedPickerHost").finish_non_exhaustive()
    }
}

impl AnimatedPickerHost {
    pub(crate) fn set_reduced_motion(&mut self, reduced: bool) {
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

    pub(crate) fn overlay(&self) -> aqua_ui::overlay::OverlayId {
        self.popup.overlay
    }

    pub fn root(&self, app: &aqua::UiApp) -> Option<aqua_core::NodeId> {
        self.popup.root(app)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the popover opens with its owner, factory, gap, focus, entrance and both durations"
    )]
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
    ) -> Result<Self, PopoverError> {
        let factory: Rc<dyn Fn() -> Element> = Rc::new(factory);
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

    #[expect(
        clippy::too_many_arguments,
        reason = "the popover opens with its owner, content, gap, focus, entrance and both durations"
    )]
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
    ) -> Result<Self, PopoverError> {
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
            scale: super::picker_motion::DROPDOWN_PRE_SCALE,
            from_scale: super::picker_motion::DROPDOWN_PRE_SCALE,
            open_duration,
            close_duration,
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
    ) -> Result<Self, PopoverError> {
        let origin = match options.placement.preferred {
            aqua_ui::overlay::Side::Above => aqua_core::Point::new(0.0, 1.0),
            aqua_ui::overlay::Side::Before => aqua_core::Point::new(1.0, 0.0),
            aqua_ui::overlay::Side::Below | aqua_ui::overlay::Side::After => {
                aqua_core::Point::new(0.0, 0.0)
            }
        };
        options.dismiss = aqua_ui::overlay::DismissPolicy::MANUAL;
        let overlay = app
            .open_popup_measured(owner, content, options)
            .map_err(PopoverError::Open)?;
        let mut motion = super::picker_motion::MenuMotion::default();
        motion.begin_open();
        let mut host = Self {
            popup: PickerPopupHost { overlay },
            factory: None,
            motion,
            entrance: PickerEntrance::Scale { origin },
            scale: super::picker_motion::DROPDOWN_PRE_SCALE,
            from_scale: super::picker_motion::DROPDOWN_PRE_SCALE,
            open_duration: super::picker_motion::DROPDOWN_OPEN,
            close_duration: super::picker_motion::DROPDOWN_CLOSE,
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

    pub fn begin_close(&mut self, app: &mut aqua::UiApp, now: f64) -> Result<(), PopoverError> {
        if self.motion.phase() != super::picker_motion::MenuPhase::Closing {
            self.motion.begin_close();
        }
        self.advance(app, now)
    }

    pub fn begin_open(&mut self, app: &mut aqua::UiApp, now: f64) -> Result<(), PopoverError> {
        self.motion.begin_open();
        self.advance(app, now)
    }

    pub fn is_closing(&self) -> bool {
        self.motion.phase() == super::picker_motion::MenuPhase::Closing
    }

    pub fn advance(&mut self, app: &mut aqua::UiApp, now: f64) -> Result<(), PopoverError> {
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
        #[expect(
            clippy::cast_possible_truncation,
            reason = "progress is clamped to 0..=1"
        )]
        let progress = if duration > 0.0 && !reduced {
            ((now - started) / duration).clamp(0.0, 1.0) as f32
        } else {
            1.0
        };
        let settled = progress >= 1.0;
        let phase = self.motion.phase();
        let eased = super::picker_motion::eased(phase, progress);
        let target_scale = super::picker_motion::travel_scale(phase);
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

    fn apply_frame(&self, app: &mut aqua::UiApp) -> Result<(), PopoverError> {
        let root = app
            .host()
            .popup_root(self.popup.overlay)
            .ok_or(PopoverError::Closed {
                overlay: self.popup.overlay,
            })?;
        let mut paint = {
            let node = app.tree().get(root).ok_or(PopoverError::Closed {
                overlay: self.popup.overlay,
            })?;
            match &node.kind {
                aqua_ui::tree::NodeKind::Box(content) => content.paint().clone(),
                aqua_ui::tree::NodeKind::Text(_)
                | aqua_ui::tree::NodeKind::Media(_)
                | aqua_ui::tree::NodeKind::Scroll(_)
                | aqua_ui::tree::NodeKind::Canvas(_)
                | aqua_ui::tree::NodeKind::Viewport3D(_) => node.paint().clone(),
            }
        };
        let (opacity, transform) = self.frame_values(app);
        paint.opacity = Some(opacity);
        paint.transform = Some(transform);
        app.tree_mut().set_paint(root, paint);
        Ok(())
    }

    pub fn refresh(&self, app: &mut aqua::UiApp) -> Result<(), PopoverError> {
        let factory = self.factory.as_ref().ok_or(PopoverError::NoFactory {
            overlay: self.popup.overlay,
        })?;
        let content = factory();
        self.refresh_content(app, content)
    }

    pub fn refresh_content(
        &self,
        app: &mut aqua::UiApp,
        content: Element,
    ) -> Result<(), PopoverError> {
        let (opacity, transform) = self.frame_values(app);
        self.popup.refresh(
            app,
            content.style(Style::new().opacity(opacity).transform(transform)),
        )
    }

    pub fn request_dismissal(
        &mut self,
        app: &mut aqua::UiApp,
        event: &aqua_core::InputEvent,
    ) -> Result<bool, PopoverError> {
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
            aqua_core::InputEvent::PointerMove(_)
            | aqua_core::InputEvent::PointerDown(..)
            | aqua_core::InputEvent::PointerUp(..)
            | aqua_core::InputEvent::PointerCancel { .. }
            | aqua_core::InputEvent::Scroll { .. }
            | aqua_core::InputEvent::KeyUp(_)
            | aqua_core::InputEvent::ModifiersChanged(_)
            | aqua_core::InputEvent::TextInput { .. }
            | aqua_core::InputEvent::Ime(_)
            | aqua_core::InputEvent::ImeReplace { .. }
            | aqua_core::InputEvent::ImeSelect { .. }
            | aqua_core::InputEvent::Focused(_) => false,
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
