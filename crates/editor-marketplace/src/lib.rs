//! Skills and the global prompt from the active Forge.
//! This screen renders the shared HostData projections and sends user choices
//! to Forge, which owns installation and adoption.

#![forbid(unsafe_code)]

mod presentation;
mod render;
#[cfg(test)]
mod tests;

use artisan_api::SkillAction;
use editor_core::{Core, CoreEvent, HostEvent, HostId, Screen};
use editor_ui::theme::{ActiveTheme as _, ArtisanTheme};
use gpui::{Context, Entity, ListAlignment, ListState, Subscription, px};
use std::sync::Arc;

pub const MARKETPLACE_SCREEN_SELECTOR: &str = "route-marketplace";

pub struct Marketplace {
    core: Entity<Core>,
    theme: ArtisanTheme,
    host: Option<HostId>,
    generation: u64,
    catalog: Option<Arc<Vec<artisan_api::CatalogSkill>>>,
    entries: Vec<render::Entry>,
    list: ListState,
    pending: Option<String>,
    notice: Option<String>,
    shown: bool,
    _host: Option<Subscription>,
    _core: Subscription,
}

impl Marketplace {
    pub fn new(core: Entity<Core>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&core, |this, core, event: &CoreEvent, cx| {
            this.shown = matches!(core.read(cx).screen(), Screen::Marketplace);
            if this.shown && matches!(event, CoreEvent::ScreenChanged | CoreEvent::HostsChanged) {
                this.sync(cx);
                cx.notify();
            } else if !this.shown {
                this._host = None;
            }
        });
        let shown = matches!(core.read(cx).screen(), Screen::Marketplace);
        let mut page = Self {
            core,
            theme: *cx.theme(),
            host: None,
            generation: 0,
            catalog: None,
            entries: vec![
                render::Entry::Header,
                render::Entry::Prompt,
                render::Entry::Notices,
            ],
            list: ListState::new(3, ListAlignment::Top, px(256.0)),
            pending: None,
            notice: None,
            shown,
            _host: None,
            _core: subscription,
        };
        if shown {
            page.sync(cx);
        }
        page
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        if !self.shown {
            return;
        }
        let host = self.core.read(cx).active_host().cloned();
        let id = host.as_ref().map(|host| host.read(cx).id.clone());
        if self.host != id {
            self.host = id;
            self.generation += 1;
            self.catalog = None;
            self.entries.truncate(3);
            self.list.reset(3);
            self.pending = None;
            self.notice = None;
            self._host = None;
        }
        let Some(host) = host else {
            return;
        };
        if self._host.is_none() {
            self._host = Some(cx.subscribe(&host, |this, _, event: &HostEvent, cx| {
                if matches!(event, HostEvent::Changed | HostEvent::Reset) {
                    this.sync(cx);
                    cx.notify();
                }
            }));
        }
        host.update(cx, |host, cx| {
            host.load_skill_catalog(false, cx);
            host.load_prompt(false, cx);
        });
        let catalog = host.read(cx).skill_catalog.clone();
        if catalog.as_ref().map(Arc::as_ptr) != self.catalog.as_ref().map(Arc::as_ptr) {
            self.entries =
                render::entries(catalog.as_deref().map(Vec::as_slice).unwrap_or_default());
            self.catalog = catalog;
            self.list.reset(self.entries.len());
        } else {
            // Header/prompt/request states can resize without changing the skill catalog.
            self.list.splice(0..3, 3);
        }
    }

    fn skill_action(&mut self, id: String, action: SkillAction, cx: &mut Context<Self>) {
        if self.pending.is_some() {
            return;
        }
        let Some(host) = self.core.read(cx).active_host().cloned() else {
            return;
        };
        let generation = self.generation;
        let client = host.read(cx).client.clone();
        self.pending = Some(id.clone());
        self.notice = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = client.skill_action(&id, action).await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.pending = None;
                match result {
                    Ok(_) => host.update(cx, |host, _| {
                        host.invalidate_catalog();
                    }),
                    Err(error) => {
                        this.notice =
                            Some(format!("Couldn't {} the skill: {error:#}", action.as_str()))
                    }
                }
                this.sync(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn navigate(&self, screen: Screen, cx: &mut Context<Self>) {
        self.core.update(cx, |core, cx| core.navigate(screen, cx));
    }
}
