use super::{ResolvedBrowser, RootViews};
use crate::shell::host::Shell;
use aqua::{AppCtx, UiApp};
use std::rc::Rc;

impl RootViews {
    pub(super) fn warm_browser(&mut self, shell: &Shell) {
        let preferred = match &shell.core.settings().preferences().browser {
            crate::core::Browser::SystemDefault => None,
            crate::core::Browser::Named(name) => Some(name.clone()),
        };
        if self
            .browser_lookup
            .browser
            .as_ref()
            .is_some_and(|resolved| resolved.preferred == preferred)
        {
            return;
        }
        let pending = self
            .browser_lookup
            .browser_task
            .as_ref()
            .filter(|(wanted, _)| *wanted == preferred)
            .map(|(_, found)| found.try_recv());
        match pending {
            Some(Ok((browser, icon))) => {
                let icon = icon.and_then(|icon| {
                    match aqua::images::DecodedImage::from_rgba(icon.width, icon.height, icon.rgba)
                    {
                        Ok(image) => match shell.images().insert(image) {
                            Ok(handle) => Some(handle),
                            Err(error) => {
                                tracing::warn!(%error, "the browser icon could not be uploaded");
                                None
                            }
                        },
                        Err(error) => {
                            tracing::warn!(%error, "the browser icon could not be decoded");
                            None
                        }
                    }
                });
                self.browser_lookup.browser_task = None;
                self.browser_lookup.browser = Some(ResolvedBrowser {
                    preferred,
                    browser,
                    icon,
                });
            }
            Some(Err(std::sync::mpsc::TryRecvError::Empty)) => {}
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                self.browser_lookup.browser_task = None;
                self.browser_lookup.browser = Some(ResolvedBrowser {
                    preferred,
                    browser: None,
                    icon: None,
                });
            }
            None => {
                let (found, receiver) = std::sync::mpsc::channel();
                let name = preferred.clone();
                let spawned = std::thread::Builder::new()
                    .name("browser-lookup".into())
                    .spawn(move || {
                        let browser = crate::shell::browser::resolve(name.as_deref());
                        let icon = browser
                            .as_ref()
                            .and_then(|browser| crate::shell::browser::icon(&browser.program));
                        if found.send((browser, icon)).is_err() {
                            tracing::debug!("the browser lookup finished after its menu moved on");
                        }
                    });
                match spawned {
                    Ok(_) => self.browser_lookup.browser_task = Some((preferred, receiver)),
                    Err(error) => tracing::warn!(%error, "could not start the browser lookup"),
                }
            }
        }
    }

    pub(super) fn open_selection_menu(
        &mut self,
        shell: &Shell,
        ui: &mut UiApp,
        ctx: &AppCtx<'_>,
    ) -> bool {
        use crate::ui::rich::actions::{CopyEntry, ShowCopyMenu};
        let Some((position, text)) = self.menus.selection_menu.take() else {
            return false;
        };
        self.menus.dispose(ui);
        let Some(owner) = ui.find_debug_selector("workspace-root") else {
            return false;
        };
        self.warm_browser(shell);
        let icon = self
            .browser_lookup
            .browser
            .as_ref()
            .and_then(|resolved| resolved.icon.clone());
        let browser = self
            .browser_lookup
            .browser
            .as_ref()
            .and_then(|resolved| resolved.browser.as_ref());
        let entries = std::iter::once(CopyEntry::primary("Copy", text.clone()))
            .chain(browser.map(|browser| {
                CopyEntry::search(format!("Search with {}", browser.name), text.clone())
            }))
            .collect();
        let effects = Rc::clone(&self.menus.color_effects);
        match crate::ui::color_copy_menu::ColorCopyMenuHost::open(
            ui,
            owner,
            ShowCopyMenu { position, entries },
            icon,
            shell.preferences.theme(),
            Self::reduced(ui),
            ctx.now().as_secs_f64(),
            move |effect| effects.borrow_mut().push_back(effect),
        ) {
            Ok(menu) => {
                self.menus.color_menu = menu;
                true
            }
            Err(error) => {
                tracing::warn!(%error, "could not open the selection menu");
                false
            }
        }
    }
}
