//! The Marketplace (master's `marketplace_screen`): every skill the active
//! host's Forge keeps in the engines, the skills engines have on their own,
//! and the global prompt.
//!
//! The screen renders Forge data and sends the user's choices to Forge;
//! nothing here decides what an engine holds.

#![forbid(unsafe_code)]

mod presentation;
mod render;
#[cfg(test)]
mod tests;

use artisan_api::{CatalogSkill, GlobalPrompt};
use editor_core::{Core, CoreEvent, HostId, Screen};
use editor_ui::theme::{ArtisanTheme, ThemeMode};
use gpui::{Context, Entity, FocusHandle, Subscription};
use std::collections::BTreeSet;

/// Root selector of the Marketplace surface.
pub const MARKETPLACE_SCREEN_SELECTOR: &str = "route-marketplace";

/// The Marketplace surface.
pub struct Marketplace {
    core: Entity<Core>,
    theme: ArtisanTheme,
    focus: FocusHandle,
    /// The host the catalog was listed for.
    host: Option<HostId>,
    /// The host's skill catalog once Forge answered, or why it couldn't.
    catalog: Option<Result<Vec<CatalogSkill>, String>>,
    /// The global prompt and its state per engine, once Forge answered.
    prompt: Option<GlobalPrompt>,
    /// Skills with a change in flight; their controls are disabled.
    pending: BTreeSet<String>,
    /// The last refused change, shown above the sections.
    notice: Option<String>,
    /// Whether the Marketplace was on screen at the last core change.
    shown: bool,
    _core: Subscription,
}

impl Marketplace {
    pub fn new(core: Entity<Core>, cx: &mut Context<Self>) -> Self {
        // Opening the Marketplace lists the skills again: a folder added on
        // the host shows the next time it opens.
        let subscription = cx.subscribe(&core, |this, core, event: &CoreEvent, cx| {
            let shown = matches!(core.read(cx).screen(), Screen::Marketplace);
            if matches!(event, CoreEvent::ScreenChanged | CoreEvent::HostsChanged)
                && shown
                && !this.shown
            {
                this.host = None;
            }
            this.shown = shown;
            cx.notify();
        });
        let shown = matches!(core.read(cx).screen(), Screen::Marketplace);
        Self {
            core,
            theme: ArtisanTheme::for_mode(ThemeMode::Dark),
            focus: cx.focus_handle(),
            host: None,
            catalog: None,
            prompt: None,
            pending: BTreeSet::new(),
            notice: None,
            shown,
            _core: subscription,
        }
    }

    /// Lists the active host's catalog and prompt when the host changes.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let host = self.core.read(cx).active_host().cloned();
        let id = host.as_ref().map(|host| host.read(cx).id.clone());
        if self.host == id {
            return;
        }
        self.host.clone_from(&id);
        self.catalog = None;
        self.prompt = None;
        self.pending.clear();
        self.notice = None;
        let Some(host) = host else {
            return;
        };
        let client = host.read(cx).client.clone();
        cx.spawn(async move |this, cx| {
            let catalog = client
                .skill_catalog()
                .await
                .map_err(|error| format!("Couldn't list the skills: {error:#}"));
            let prompt = client.global_prompt().await;
            let _ = this.update(cx, |this, cx| {
                if this.host == id {
                    this.catalog = Some(catalog);
                    match prompt {
                        Ok(prompt) => this.prompt = Some(prompt),
                        Err(error) => {
                            this.notice =
                                Some(format!("Couldn't read the global prompt: {error:#}"));
                        }
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Turns a skill on or off (`enable`/`disable`) or adopts one (`adopt`)
    /// and shows the catalog Forge answers with.
    fn skill_action(&mut self, id: String, action: &'static str, cx: &mut Context<Self>) {
        let Some(host) = self.core.read(cx).active_host().cloned() else {
            return;
        };
        let host_id = host.read(cx).id.clone();
        let client = host.read(cx).client.clone();
        self.pending.insert(id.clone());
        self.notice = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = client.skill_action(&id, action).await;
            let _ = this.update(cx, |this, cx| {
                this.pending.remove(&id);
                if this.host.as_ref() != Some(&host_id) {
                    return;
                }
                match result {
                    Ok(catalog) => this.catalog = Some(Ok(catalog)),
                    Err(error) => {
                        let verb = match action {
                            "adopt" => "adopt",
                            "enable" => "turn on",
                            _ => "turn off",
                        };
                        this.notice = Some(format!("Couldn't {verb} the skill: {error:#}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn navigate(&self, screen: Screen, cx: &mut Context<Self>) {
        self.core.update(cx, |core, cx| core.navigate(screen, cx));
    }
}
