use crate::frame::account::{MENU_GAP, TRIGGER};
use crate::ui::popover::{AnimatedPickerHost, PickerEntrance};
use aqua::UiApp;
use aqua_ui::element::Element;
use std::{rc::Rc, time::Duration};

const OPEN: Duration = Duration::from_millis(250);
const CLOSE: Duration = Duration::from_millis(150);

#[derive(Default)]
pub struct Popup {
    host: Option<AnimatedPickerHost>,
}

impl Popup {
    pub fn sync(
        &mut self,
        ui: &mut UiApp,
        open: bool,
        content: impl FnOnce() -> Element,
        now: f64,
        dismissed: Rc<dyn Fn()>,
    ) -> Result<(), String> {
        if self.host.as_ref().is_some_and(|host| !host.is_open(ui)) {
            self.host = None;
        }
        match (&mut self.host, open) {
            (None, true) => {
                let Some(owner) = ui.find_debug_selector(TRIGGER) else {
                    return Ok(());
                };
                self.host = Some(AnimatedPickerHost::open_content(
                    ui,
                    owner,
                    content(),
                    MENU_GAP,
                    aqua_ui::popup::PopupFocus::Preserve,
                    PickerEntrance::Scale {
                        origin: aqua_core::Point::new(0.0, 1.0),
                    },
                    OPEN,
                    CLOSE,
                    now,
                    move |_| dismissed(),
                )?);
            }
            (Some(host), true) => {
                if host.is_closing() {
                    host.begin_open(ui, now)?;
                }
                host.refresh_content(ui, content())?;
            }
            (Some(host), false) if !host.is_closing() => host.begin_close(ui, now)?,
            (Some(_) | None, false) => {}
        }
        Ok(())
    }

    pub fn advance(&mut self, ui: &mut UiApp, now: f64) -> Result<bool, String> {
        let Some(host) = &mut self.host else {
            return Ok(false);
        };
        host.advance(ui, now)?;
        if !host.is_open(ui) {
            self.host = None;
            return Ok(false);
        }
        Ok(host.is_animating(ui))
    }

    pub fn dismiss(&mut self, ui: &mut UiApp, input: &aqua_core::InputEvent) -> Result<bool, String> {
        match &mut self.host {
            Some(host) if !host.is_closing() => host.request_dismissal(ui, input),
            _ => Ok(false),
        }
    }

    pub fn dispose(&mut self, ui: &mut UiApp) {
        if let Some(host) = self.host.take() {
            host.dispose(ui);
        }
    }
}
