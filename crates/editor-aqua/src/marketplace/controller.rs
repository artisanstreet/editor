use std::num::NonZeroUsize;
use std::time::Instant;

use artisan_api::{CatalogSkill, Listing, McpServer};
use tokio_util::task::AbortOnDropHandle;

use crate::core::delivery::Delivery;
use crate::core::{Core, CoreEvent, HostEvent, HostId, HostInstance, Screen, runtime};
use crate::ui::text::mount::{EditorController, EditorEvent};
use crate::ui::text::{Editor, ListKey as EditorListKey, Mode};
use std::rc::Rc;

use super::{Action, ListKey, Marketplace, Operation, OperationRequest, SearchRequest};

enum Completion {
    Search(
        HostId,
        HostInstance,
        SearchRequest,
        anyhow::Result<Vec<Listing>>,
    ),
    Operation(
        HostId,
        HostInstance,
        OperationRequest,
        anyhow::Result<Vec<CatalogSkill>>,
    ),
}

#[derive(PartialEq, Eq)]
pub(crate) struct ViewSnapshot {
    screen: Screen,
    shown: bool,
    host: Option<HostId>,
    host_instance: Option<HostInstance>,
    query: String,
    kind: super::presentation::Kind,
    entries: Vec<super::Entry>,
    items: Vec<super::presentation::Item>,
    pending: Option<String>,
    notice: Option<String>,
    found_open: bool,
    prompt_hovered: bool,
    catalog_loading: bool,
    catalog_error: Option<String>,
    prompt_error: Option<String>,
    prompt_attention: Option<String>,
}

pub struct MarketplaceController {
    pub page: Marketplace,
    pub search: EditorController,
    completion: Delivery<Completion>,
    search_task: Option<AbortOnDropHandle<()>>,
    host_instance: Option<HostInstance>,
    loading_started: Option<Instant>,
    pub reduced_motion: bool,
    pub prompt_hovered: bool,
}

impl Default for MarketplaceController {
    fn default() -> Self {
        Self::new(Rc::new(|_| {}))
    }
}

impl MarketplaceController {
    pub fn new(emit: Rc<dyn Fn(super::view::ViewAction)>) -> Self {
        let search =
            EditorController::new(
                Editor::new("", Mode::SingleLine),
                move |event| match event {
                    EditorEvent::Changed => emit(super::view::ViewAction::SearchChanged),
                    EditorEvent::List(key) => emit(super::view::ViewAction::SearchKey(key)),
                    _ => {}
                },
            );
        Self {
            page: Marketplace::default(),
            search,
            completion: Delivery::new(NonZeroUsize::new(64).unwrap()),
            search_task: None,
            host_instance: None,
            loading_started: None,
            reduced_motion: false,
            prompt_hovered: false,
        }
    }

    pub(crate) fn view_snapshot(&self, core: &Core) -> ViewSnapshot {
        let host = core.active_host();
        ViewSnapshot {
            screen: core.screen().clone(),
            shown: self.page.shown,
            host: self.page.host.clone(),
            host_instance: self.host_instance,
            query: self.page.query.clone(),
            kind: self.page.kind,
            entries: super::entries(&self.page),
            items: self.page.items.clone(),
            pending: self.page.pending.clone(),
            notice: self.page.notice.clone(),
            found_open: self.page.found_open,
            prompt_hovered: self.prompt_hovered,
            catalog_loading: host.is_some_and(|host| host.catalog_load().loading),
            catalog_error: host.and_then(|host| host.catalog_load().error.clone()),
            prompt_error: host.and_then(|host| host.prompt_load().error.clone()),
            prompt_attention: host
                .and_then(|host| host.global_prompt.as_deref())
                .and_then(super::presentation::prompt_attention),
        }
    }

    pub fn wake_mailbox(&self) -> &aqua::mailbox::Mailbox<()> {
        self.completion.wake_mailbox().unwrap()
    }

    pub fn sync(&mut self, core: &mut Core) {
        self.page
            .set_shown(matches!(core.screen(), Screen::Marketplace));
        if !self.page.shown {
            self.prompt_hovered = false;
            return;
        }
        let active = core
            .active_host()
            .map(|host| (host.id.clone(), host.instance()));
        let id = active.as_ref().map(|(id, _)| id.clone());
        let instance = active.map(|(_, instance)| instance);
        if self.page.host == id && self.host_instance != instance {
            self.page.sync_host(None, &[], &[]);
            self.search_task = None;
            self.page.cancel_search();
        }
        self.host_instance = instance;
        if let Some(host) = id.as_ref().and_then(|id| core.host_mut(id)) {
            host.load_skill_catalog(false);
            host.load_prompt(false);
            let servers: Vec<McpServer> = host
                .setting("mcp_servers")
                .and_then(|value| serde_json::from_value(value.clone()).ok())
                .unwrap_or_default();
            let loading = host.catalog_load().loading
                && host
                    .skill_catalog
                    .as_ref()
                    .is_none_or(|catalog| catalog.is_empty());
            if loading && self.loading_started.is_none() {
                self.loading_started = Some(Instant::now());
            } else if !loading {
                self.loading_started = None;
            }
            self.page.sync_host(
                id.clone(),
                host.skill_catalog
                    .as_deref()
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
                &servers,
            );
        } else {
            self.loading_started = None;
            self.page.sync_host(None, &[], &[]);
        }
    }

    pub fn loading_started(&self) -> Option<Instant> {
        self.loading_started
    }

    pub fn needs_animation(&self) -> bool {
        self.page.shown && self.loading_started.is_some() && !self.reduced_motion
    }

    pub fn take_focus_request(&mut self) -> bool {
        std::mem::take(&mut self.page.focus_search)
    }

    pub fn selected_entry_index(&self) -> Option<usize> {
        super::entries(&self.page)
            .iter()
            .position(|entry| matches!(entry, super::Entry::Item { selected: true, .. }))
    }

    pub fn selected_entry_key(&self) -> Option<String> {
        self.selected_entry_index()
            .map(|index| format!("marketplace-entry-{index}"))
    }

    pub fn core_event(&mut self, core: &mut Core, event: &CoreEvent) {
        match event {
            CoreEvent::ScreenChanged | CoreEvent::HostsChanged => self.sync(core),
            CoreEvent::Host {
                host,
                event: HostEvent::Changed | HostEvent::Reset,
            } if self.page.shown && self.page.host.as_ref() == Some(host) => self.sync(core),
            _ => {}
        }
    }

    pub fn edited(&mut self, core: &Core) {
        let query = self.search.state().borrow().text().to_owned();
        if query == self.page.query {
            return;
        }
        self.search_task = None;
        self.page.set_query(query, Instant::now());
        self.search
            .state()
            .borrow_mut()
            .set_list_open(!self.page.query.trim().is_empty());
        let Some(deadline) = self.page.search_deadline() else {
            return;
        };
        let Some(host) = core.active_host() else {
            return;
        };
        let client = host.client.clone();
        let id = host.id.clone();
        let instance = host.instance();
        let Some(request) = self.page.take_search(deadline) else {
            return;
        };
        let sender = self.completion.sender();
        self.search_task = Some(AbortOnDropHandle::new(runtime::runtime().spawn(
            async move {
                tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
                let result = Marketplace::search(&client, &request).await;
                let _ = sender
                    .send(Completion::Search(id, instance, request, result))
                    .await;
            },
        )));
    }

    pub fn clear_search(&mut self, core: &Core) {
        self.search.state().borrow_mut().set_text("");
        self.edited(core);
    }

    pub fn list_key(&mut self, core: &mut Core, key: EditorListKey) {
        let key = match key {
            EditorListKey::Up => ListKey::Up,
            EditorListKey::Down => ListKey::Down,
            EditorListKey::Accept => ListKey::Accept,
            EditorListKey::Dismiss => {
                self.clear_search(core);
                return;
            }
        };
        if let Some(action) = self.page.list_key(key, Instant::now()) {
            self.act(core, action);
        }
    }

    pub fn act(&mut self, core: &mut Core, action: Action) {
        match action {
            Action::Navigate(screen) => core.navigate(screen),
            Action::Operation(operation) => self.operate(core, operation),
        }
    }

    pub fn operate(&mut self, core: &Core, operation: Operation) {
        let Some(host) = core.active_host() else {
            return;
        };
        if self.page.host.as_ref() != Some(&host.id) || self.host_instance != Some(host.instance())
        {
            return;
        }
        let Some(request) = self.page.begin_operation(operation) else {
            return;
        };
        let client = host.client.clone();
        let id = host.id.clone();
        let instance = host.instance();
        let sender = self.completion.sender();
        runtime::runtime().spawn(async move {
            let result = Marketplace::execute(&client, &request).await;
            let _ = sender
                .send(Completion::Operation(id, instance, request, result))
                .await;
        });
    }

    pub fn retry(&mut self, core: &mut Core) {
        if let Some(host) = core
            .active_host()
            .map(|host| host.id.clone())
            .and_then(|id| core.host_mut(&id))
        {
            host.load_prompt(true);
            host.load_skill_catalog(true);
        }
    }

    pub fn drain(&mut self, core: &mut Core) -> bool {
        let before = self.view_snapshot(core);
        let completions = self.completion.drain();
        for completion in completions {
            match completion {
                Completion::Search(id, instance, request, result) => {
                    if core.host_if_instance(&id, instance).is_some() {
                        self.page.finish_search(&request, result);
                    } else {
                        self.sync(core);
                    }
                }
                Completion::Operation(id, instance, request, result) => {
                    if core.host_if_instance(&id, instance).is_some() {
                        if self.page.finish_operation(&request, result) {
                            if let Some(host) = core.host_mut_if_instance(&id, instance) {
                                host.invalidate_catalog();
                            }
                        }
                    }
                    self.sync(core);
                }
            }
        }
        before != self.view_snapshot(core)
    }
}
