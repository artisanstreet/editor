use self::open_thread::PendingStall;
use crate::core::Screen;
use crate::shell::{application::SectionViews, host::Shell, layout, view};
use aqua::prelude::{AquaWindowId, InputEvent, ObservedWindowState};
use aqua::{AppCtx, UiApp};
use aqua_ui::element::Element;
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    rc::Rc,
};

mod open_thread;
mod rail_refresh;
mod wheel_sync;

enum ConversationAction {
    Source(crate::conversation::view::row_sources::RowSourceAction),
    Link(
        crate::conversation::presentation::PresentationScope,
        crate::ui::markdown::visual::LinkAction,
    ),
    Color(
        crate::conversation::presentation::PresentationScope,
        crate::ui::rich::actions::ShowCopyMenu,
    ),
    Agent(
        crate::conversation::presentation::PresentationScope,
        Option<artisan_api::ItemId>,
    ),
    Copy(
        crate::conversation::presentation::PresentationScope,
        crate::conversation::view::source_actions::CopySource,
    ),
    RetryControls(crate::conversation::presentation::PresentationScope),
    Blob(
        crate::conversation::presentation::PresentationScope,
        artisan_api::ItemId,
        u64,
        crate::conversation::view::factory::BlobAction,
    ),
    Media(
        crate::conversation::presentation::PresentationScope,
        crate::conversation::presentation::BodyKey,
        u64,
        crate::ui::media::preview::Effect,
    ),
}

struct ConversationMedia {
    host: crate::core::HostId,
    instance: crate::core::HostInstance,
    thread: artisan_api::ThreadId,
    fetcher: crate::ui::media::source::MediaFetcher,
    owners: HashMap<artisan_api::ItemId, u64>,
    models: crate::ui::media::model::load::ModelAssets,
    audio: crate::ui::media::audio::AudioAssets,
}

impl Drop for ConversationMedia {
    fn drop(&mut self) {
        self.fetcher.clear_resources();
        self.models.drain();
        self.audio.drain();
    }
}

struct AppMedia {
    player: editor_audio::Player,
    audio_assets: crate::ui::media::audio::AudioAssets,
    model_assets: crate::ui::media::model::load::ModelAssets,
    image_assets: aqua::images::ImageAssets,
    model_capability: Rc<dyn Fn() -> Option<crate::ui::media::model::load::RendererCapability>>,
    scale: f32,
}

struct ImagePreview {
    scope: crate::conversation::presentation::PresentationScope,
    owner: Option<std::sync::Arc<[u64]>>,
    view: crate::ui::media::preview::Preview,
    focus_pending: bool,
}

const MOTION_REBUILD_INTERVAL: std::time::Duration = std::time::Duration::from_millis(33);

#[derive(Default)]
pub struct RootViews {
    motion_rebuilt: Cell<Option<std::time::Instant>>,
    #[cfg(feature = "qualification")]
    qualification_paint: crate::qualification::observation::PaintCollector,
    settings: Option<crate::ui::dialog::DialogHost>,
    settings_panel: Option<(Element, f32)>,
    settings_dismissed: Rc<Cell<bool>>,
    workspace_key: Option<u64>,
    palette: Option<crate::shell::palette::view::PaletteView>,
    palette_dialog: Option<crate::ui::dialog::DialogHost>,
    palette_content: Option<Element>,
    palette_events: Rc<RefCell<VecDeque<crate::shell::palette::view::Event>>>,
    resource: Option<(u64, crate::shell::resource_preview::view::ViewState)>,
    resource_dialog: Option<crate::ui::dialog::DialogHost>,
    resource_content: Option<(Element, String)>,
    resource_events: Rc<RefCell<VecDeque<crate::shell::resource_preview::view::Action>>>,
    failure_dismissed: Rc<Cell<bool>>,
    rows: Option<Rc<crate::conversation::view::factory::RowFactory>>,
    row_actions: Rc<RefCell<VecDeque<crate::conversation::adapter::ScopedAction>>>,
    observations: crate::conversation::view::transcript::TranscriptObservations,
    mounted_reveals: Rc<RefCell<crate::conversation::view::reveal::MountedReveals>>,
    row_cache: Rc<RefCell<crate::conversation::view::row_cache::RowCache>>,
    stored: Rc<RefCell<crate::conversation::stored::StoredConversation>>,
    source_dialogs: crate::shell::source::RowSourceDialogs,
    row_motion: Rc<RefCell<crate::conversation::view::motion::RowMotion>>,
    markdown_engine: Rc<RefCell<Option<crate::ui::markdown::MarkdownEngine>>>,
    conversation_invalidated: Rc<Cell<bool>>,
    navigation_invalidated: Rc<Cell<bool>>,
    row_token: Option<Rc<Cell<bool>>>,
    row_settled: Rc<RefCell<HashMap<crate::conversation::fold::RowKey, u64>>>,
    row_style: Option<(crate::ui::theme::ArtisanTheme, bool, bool, u32)>,
    row_style_epoch: u64,
    media: Option<ConversationMedia>,
    image_preview: Option<ImagePreview>,
    preview_generation: u64,
    preview_actions: Rc<RefCell<VecDeque<(u64, crate::ui::media::preview::Action)>>>,
    motion_epoch: Option<std::time::Instant>,
    ui_now: aqua::Instant,
    context_tooltip: crate::shell::titlebar::ContextMeterTooltip,
    sidebar_overlays: crate::sidebar::overlay::SidebarOverlays,
    sidebar_drag: Option<crate::sidebar::drag_preview::DragPreviewHost>,
    drag_position: Option<aqua_core::Point>,
    navigation: crate::shell::thread_navigation::ThreadNavigation,
    transcript_viewport: Option<aqua_ui::collection_runtime::CollectionViewport>,
    navigation_marker: Option<usize>,
    context_reading: Option<crate::shell::context_meter::ContextReading>,
    connection: crate::shell::connection::ConnectionPresenter,
    thread_loading: crate::shell::connection::WordmarkLoader,
    thread_landing: crate::shell::connection::WordmarkLoader,
    smooth_scroll: crate::conversation::scroll::SmoothScroll,
    wheel: crate::shell::wheel::WheelGlides,
    pending_stall: PendingStall,
    transcript_edges: crate::ui::scroll_edge::ScrollEdges,
    scroll_requests: VecDeque<crate::conversation::controller::ScrollAction>,
    composer_measurement: Rc<Cell<Option<f32>>>,
    composer_height: Option<f32>,
    conversation_actions: Rc<RefCell<VecDeque<ConversationAction>>>,
    color_menu: Option<crate::ui::color_copy_menu::ColorCopyMenuHost>,
    color_scope: Option<crate::conversation::presentation::PresentationScope>,
    color_effects: Rc<RefCell<VecDeque<crate::ui::color_copy_menu::ColorCopyEffect>>>,
    resize_key: Option<ResizeKey>,
    transcript_shape: Option<(u64, usize)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ResizeKey {
    sidebar: Option<u32>,
    inspector: Option<u32>,
    wide: bool,
}

impl ResizeKey {
    fn of(screen: &Screen, show_sidebar: bool, show_inspector: bool, window_width: f32) -> Self {
        let dimensions = layout::workspace_layout(
            window_width,
            screen,
            show_sidebar,
            show_inspector,
            false,
        );
        Self {
            sidebar: dimensions.sidebar.map(f32::to_bits),
            inspector: dimensions.inspector.map(f32::to_bits),
            wide: window_width >= 640.0,
        }
    }
}

impl RootViews {
    #[cfg(feature = "qualification")]
    pub fn arm_qualification_paint(&mut self, shell: &Shell, generation: u64) -> bool {
        let scope = shell.entry.workspace().and_then(|workspace| {
            crate::conversation::presentation::PresentationScope::capture(
                &workspace.conversation,
                &shell.core,
            )
        });
        self.qualification_paint.arm(
            scope,
            generation,
            self.observations.qualification_paint_serial(),
        )
    }

    #[cfg(feature = "qualification")]
    pub fn qualification_paint(
        &self,
        shell: &Shell,
    ) -> Option<crate::qualification::observation::Paint> {
        let workspace = shell.entry.workspace()?;
        let scope = crate::conversation::presentation::PresentationScope::capture(
            &workspace.conversation,
            &shell.core,
        )?;
        self.qualification_paint.current(&scope)
    }

    #[cfg(feature = "qualification")]
    pub fn qualification_observation(
        &self,
        shell: &Shell,
        ui: &UiApp,
    ) -> Option<crate::qualification::observation::Observation> {
        let account = shell.entry.qualification_account()?.to_owned();
        let workspace = shell.entry.workspace()?;
        let conversation = &workspace.conversation;
        let scope = crate::conversation::presentation::PresentationScope::capture(
            conversation,
            &shell.core,
        )?;
        let view = conversation.view.as_ref();
        let markdown = workspace.markdown.borrow();
        let prepared_inline_prose = conversation.items().filter_map(|item| {
            let artisan_api::ItemKind::AssistantMessage {
                text: artisan_api::TextContent::Inline(source), ..
            } = &item.kind else { return None; };
            let key = crate::conversation::presentation::BodyKey {
                item: item.id.clone(),
                slot: crate::conversation::fold::Slot::Prose,
            };
            conversation.rows.iter().any(|(row, _)| {
                matches!(row.semantic(), crate::conversation::fold::Row::Text { id, slot: crate::conversation::fold::Slot::Prose, block, .. }
                    if id == &item.id && markdown.prepared_body_current(&scope, &key, source, item.seq, *block))
            }).then(|| item.id.to_string())
        }).collect();
        let bounds = |selector| {
            ui.find_debug_selector(selector)
                .and_then(|node| ui.tree().visual_rect(node))
                .map(crate::qualification::observation::rect_bits)
        };
        Some(crate::qualification::observation::Observation {
            account,
            host: scope.host.0,
            thread: scope.thread.to_string(),
            agent: scope.agent.as_ref().map(ToString::to_string),
            items: conversation.items().cloned().collect(),
            page_loaded: view.is_some_and(|view| view.viewport.loaded()),
            page_in_flight: view.is_some_and(|view| view.viewport.in_flight),
            page_exhausted: view.is_some_and(|view| view.viewport.exhausted),
            page_oldest: view.and_then(|view| view.viewport.oldest),
            page_gap: view.and_then(|view| view.viewport.gap),
            load_error: conversation.load_error.clone(),
            landing: conversation.landing.is_some(),
            work_in_flight: conversation.work_in_flight(),
            row_count: conversation.rows.len(),
            measured_row_count: conversation.row_heights.len(),
            prepared_inline_prose,
            visible_row_start: self
                .transcript_viewport
                .map(|viewport| viewport.visible.start),
            scroll_y_bits: self
                .transcript_viewport
                .map(|viewport| viewport.scroll_y.to_bits()),
            transcript_bounds_bits: bounds("conversation-transcript"),
            composer_bounds_bits: bounds("composer"),
        })
    }

    fn source_export_is_current(
        shell: &Shell,
        export: &crate::conversation::view::row_sources::RowSourceExport,
    ) -> bool {
        let Some(workspace) = shell.entry.workspace() else {
            return false;
        };
        export.scope.current(&workspace.conversation, &shell.core)
            && workspace
                .conversation
                .view
                .as_ref()
                .and_then(|view| view.item(&export.item))
                .is_some_and(|item| export.is_current(&shell.core, &export.scope, item))
    }

    fn finish_source_dialog(
        shell: &Shell,
        export: crate::conversation::view::row_sources::RowSourceExport,
        result: Result<Option<std::path::PathBuf>, String>,
    ) -> bool {
        if !Self::source_export_is_current(shell, &export) {
            return false;
        }
        let Some(item) = shell
            .entry
            .workspace()
            .and_then(|workspace| workspace.conversation.view.as_ref())
            .and_then(|view| view.item(&export.item))
        else {
            return false;
        };
        let scope = export.scope.clone();
        export.finish(&shell.core, &scope, item, result);
        true
    }

    fn source_effect(
        &mut self,
        shell: &Shell,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
        effect: crate::conversation::view::row_sources::RowSourceEffect,
    ) -> bool {
        use crate::conversation::view::row_sources::RowSourceEffect;
        match effect {
            RowSourceEffect::Clipboard(copied) => {
                copied.write(&shell.core, |text| {
                    if let Err(error) = ctx.clipboard().set_text(text) {
                        tracing::warn!(%error, "could not copy conversation source");
                    }
                });
                false
            }
            RowSourceEffect::SavePath(export) => {
                if !Self::source_export_is_current(shell, &export) {
                    return false;
                }
                self.source_dialogs
                    .open(export, ctx, &shell.dialogs, window)
                    .is_some_and(|(export, result)| {
                        Self::finish_source_dialog(shell, export, result)
                    })
            }
        }
    }

    fn bind_row_sources(&self, ui: &mut UiApp, ctx: &AppCtx<'_>, window: AquaWindowId) {
        let _tm = crate::shell::application::Tm::new("v.bind_sources");
        self.stored.borrow().bind(ctx, window);
        self.stored.borrow_mut().drain_cleanup(ui);
        let owners = self.row_cache.borrow().live_owners();
        for owner in owners.into_iter().filter_map(|owner| owner.upgrade()) {
            owner.bind_sources(ctx, window);
        }
        self.row_cache.borrow().drain_cleanup(ui);
        self.source_dialogs.bind(ctx, window);
    }

    fn drain_row_sources(
        &mut self,
        shell: &Shell,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) -> bool {
        let Some(workspace) = shell.entry.workspace() else {
            return false;
        };
        let Some(scope) = crate::conversation::presentation::PresentationScope::capture(
            &workspace.conversation,
            &shell.core,
        ) else {
            return false;
        };
        let Some(provider) = shell.core.source_provider(&scope.host, scope.instance) else {
            return false;
        };
        let Some(view) = &workspace.conversation.view else {
            return false;
        };
        let owners = self.row_cache.borrow().live_owners();
        let mut changed = false;
        let mut effects = Vec::new();
        for owner in owners.into_iter().filter_map(|owner| owner.upgrade()) {
            if owner
                .identity()
                .is_none_or(|(_, current)| current != &scope)
            {
                continue;
            }
            let (source_changed, source_effects) =
                owner.drain_sources(&shell.core, &scope, &provider, |id| view.shared_item(id));
            changed |= source_changed;
            effects.extend(source_effects);
        }
        for effect in effects {
            changed |= self.source_effect(shell, ctx, window, effect);
        }
        changed
    }

    fn close_image_preview(&mut self, ui: &mut UiApp, restore_focus: bool) {
        if let Some(preview) = self.image_preview.take() {
            if restore_focus {
                if let Some(key) = preview.view.opener_key() {
                    ui.focus_key(key.to_owned());
                }
            }
        }
        self.preview_actions.borrow_mut().clear();
    }

    fn preview_root(ui: &UiApp) -> Option<aqua_core::NodeId> {
        let key = aqua_ui::tree::hash_key(aqua_ui::element::Key::from("image-preview"));
        ui.tree().sole_node_with_key(key)
    }

    fn cycle_preview_focus(ui: &mut UiApp, backwards: bool) -> bool {
        let Some(root) = Self::preview_root(ui) else {
            return false;
        };
        let mut subtree = std::collections::HashSet::new();
        let mut pending = vec![root];
        while let Some(node) = pending.pop() {
            if subtree.insert(node) {
                if let Some(record) = ui.tree().get(node) {
                    pending.extend(record.children.iter().copied());
                }
            }
        }
        let order: Vec<_> = aqua_ui::events::focusable_nodes(ui.tree())
            .into_iter()
            .filter(|node| subtree.contains(node))
            .collect();
        if order.is_empty() {
            return false;
        }
        let index = ui
            .tree()
            .focused()
            .and_then(|focused| order.iter().position(|node| *node == focused));
        let next = match (index, backwards) {
            (Some(0) | None, true) => order.len() - 1,
            (Some(index), true) => index - 1,
            (Some(index), false) => (index + 1) % order.len(),
            (None, false) => 0,
        };
        ui.tree_mut().set_focused(Some(order[next]))
    }

    fn sync_image_preview(
        &mut self,
        shell: &Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) -> bool {
        if self.image_preview.as_ref().is_some_and(|preview| {
            !matches!(shell.core.screen(), Screen::Thread { .. })
                || !shell.entry.workspace().is_some_and(|workspace| {
                    preview.scope.current(&workspace.conversation, &shell.core)
                })
        }) {
            self.close_image_preview(ui, false);
            return true;
        }
        let reduced = ui.host().motion().reduced_motion() == aqua_ui::motion::ReducedMotion::Reduce;
        let now = std::time::Instant::now();
        let events = std::mem::take(&mut *self.preview_actions.borrow_mut());
        let mut changed = false;
        for (generation, action) in events {
            let Some(preview) = &mut self.image_preview else {
                continue;
            };
            if generation != preview.view.generation() {
                continue;
            }
            if let crate::ui::media::preview::Action::CycleFocus(backwards) = action {
                changed |= Self::cycle_preview_focus(ui, backwards);
            } else {
                changed |= preview
                    .view
                    .dispatch(generation, action, &shell.core, now, reduced);
            }
        }
        let Some(preview) = &mut self.image_preview else {
            return changed;
        };
        ctx.bind_mailbox(window, preview.view.wake_mailbox());
        changed |= preview.view.refresh(&shell.core);
        let animating = preview.view.animating(now, reduced);
        preview.view.advance(now, reduced);
        if preview.view.finished(now, reduced) {
            self.close_image_preview(ui, true);
            return true;
        }
        if preview.focus_pending && ui.focus_key("image-preview") {
            preview.focus_pending = false;
            changed = true;
        }
        if animating {
            ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION);
        }
        changed || animating
    }

    fn thread_content(
        &mut self,
        workspace: &mut crate::shell::workspace::Workspace,
        core: &crate::core::Core,
        ui: &mut UiApp,
        theme: crate::ui::theme::ArtisanTheme,
        width: f32,
        window_height: f32,
        reduced_motion: bool,
        swift: bool,
        media_resources: AppMedia,
        now: std::time::Instant,
        transcript_only: bool,
    ) -> anyhow::Result<Element> {
        use crate::conversation::presentation::PresentationScope;
        use crate::conversation::view::factory::RowFactory;
        let scope = PresentationScope::capture(&workspace.conversation, core)
            .ok_or_else(|| anyhow::anyhow!("conversation Forge lifetime is unavailable"))?;
        if self.rows.as_ref().is_none_or(|rows| rows.scope != scope) {
            if let Some(token) = self.row_token.take() {
                token.set(false);
            }
            self.row_settled.borrow_mut().clear();
            self.observations.clear();
            self.navigation.dispose(ui);
            self.transcript_viewport = None;
            self.mounted_reveals.borrow_mut().rebind(None);
            self.row_cache.borrow_mut().rebind(None);
            self.smooth_scroll.cancel();
            self.scroll_requests.clear();
            self.row_motion = Rc::default();
            workspace.status_shimmers.borrow_mut().retain(|_| false);
        }
        workspace
            .markdown
            .borrow_mut()
            .rebind(ui, Some(scope.clone()));
        workspace
            .conversation_selection
            .rebind(ui, Some(scope.clone()));
        workspace
            .conversation
            .advance_disclosure_frame(core, now, reduced_motion);
        if self.media.as_ref().is_none_or(|media| {
            media.host != scope.host
                || media.instance != scope.instance
                || media.thread != scope.thread
        }) {
            drop(self.media.take());
            self.observations.clear();
            let client = core
                .host_if_instance(&scope.host, scope.instance)
                .ok_or_else(|| anyhow::anyhow!("conversation Forge lifetime is unavailable"))?
                .client
                .clone();
            let thread = scope.thread.clone();
            let fetcher = crate::ui::media::source::MediaFetcher::new(
                format!("{}/{}", scope.host, scope.thread),
                move |request| {
                    let client = client.clone();
                    let thread = thread.clone();
                    Box::pin(async move {
                        client
                            .resource(&thread, &request.path, request.from_home)
                            .await
                            .map(std::sync::Arc::<[u8]>::from)
                            .map_err(|error| {
                                use crate::ui::media::source::MediaFailure;
                                match error
                                    .chain()
                                    .find_map(|cause| cause.downcast_ref::<artisan_api::ApiError>())
                                    .map(|error| error.code)
                                {
                                    Some(artisan_api::ErrorCode::NotFound) => MediaFailure::Missing,
                                    Some(_) => MediaFailure::Unreadable,
                                    None => MediaFailure::Unreachable,
                                }
                            })
                    })
                },
            );
            self.media = Some(ConversationMedia {
                host: scope.host.clone(),
                instance: scope.instance,
                thread: scope.thread.clone(),
                fetcher,
                owners: HashMap::new(),
                models: media_resources.model_assets.clone(),
                audio: media_resources.audio_assets.clone(),
            });
        }
        let media = self
            .media
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("conversation media owner is unavailable"))?;
        let items: HashMap<_, _> = workspace
            .conversation
            .view
            .iter()
            .flat_map(|view| view.items.values().chain(view.context.values()))
            .map(|item| (item.id.clone(), item.seq))
            .collect();
        workspace
            .markdown
            .borrow_mut()
            .retain(ui, |key| items.contains_key(&key.item));
        workspace
            .conversation_selection
            .retain(ui, |key| items.contains_key(&key.item));
        let row_keys: std::collections::HashSet<_> = workspace
            .conversation
            .rows
            .iter()
            .map(|(row, _)| row.key())
            .collect();
        workspace
            .status_shimmers
            .borrow_mut()
            .retain(|key| row_keys.contains(key));
        self.row_cache.borrow_mut().retain_rows(&row_keys);
        media.owners.retain(|item, _| items.contains_key(item));
        for item in items.keys() {
            media
                .owners
                .entry(item.clone())
                .or_insert_with(|| workspace.selections.next_source().0);
        }
        let mut retained: Vec<_> = media.owners.values().map(|owner| vec![*owner]).collect();
        if let Some(owner) = self
            .image_preview
            .as_ref()
            .and_then(|preview| preview.owner.as_ref())
        {
            retained.push(owner.to_vec());
        }
        media.fetcher.retain_resources(&retained);
        let providers: HashMap<_, _> = items
            .into_iter()
            .filter_map(|(item, revision)| {
                Some((
                    item.clone(),
                    (
                        media
                            .fetcher
                            .clone()
                            .owned_by(&[*media.owners.get(&item)?], revision),
                        revision,
                    ),
                ))
            })
            .collect();
        let images = workspace.images.as_ref().clone();
        let media_actions = Rc::clone(&self.conversation_actions);
        let media_scope = scope.clone();
        let sender = core.wake_mailbox().sender();
        let wake: aqua::mailbox::Wake = std::sync::Arc::new(move || {
            let _queued = sender.send(());
        });
        let actions = Rc::clone(&self.row_actions);
        let dispatch = crate::conversation::view::scoped_dispatch(
            &workspace.conversation,
            Rc::new(move |action| actions.borrow_mut().push_back(action)),
        )
        .ok_or_else(|| anyhow::anyhow!("conversation action scope is unavailable"))?;
        let (on_link, on_color) = self.conversation_callbacks(scope.clone());
        let invalidated = Rc::clone(&self.conversation_invalidated);
        let row_token = Rc::clone(
            self.row_token
                .get_or_insert_with(|| Rc::new(Cell::new(true))),
        );
        let style = (theme, reduced_motion, swift, media_resources.scale.to_bits());
        if self.row_style != Some(style) {
            self.row_style = Some(style);
            self.row_style_epoch += 1;
            self.row_settled.borrow_mut().clear();
        }
        let settled = Rc::clone(&self.row_settled);
        let invalidate_settled = Rc::clone(&self.row_settled);
        let seconds = now
            .saturating_duration_since(*self.motion_epoch.get_or_insert(now))
            .as_secs_f64();
        let blob_images = core
            .host_if_instance(&scope.host, scope.instance)
            .ok_or_else(|| anyhow::anyhow!("conversation image owner is unavailable"))?
            .blob_images();
        let blob_actions = Rc::clone(&self.conversation_actions);
        let blob_scope = scope.clone();
        let source_provider = core
            .source_provider(&scope.host, scope.instance)
            .ok_or_else(|| anyhow::anyhow!("conversation source provider is unavailable"))?;
        let source_actions = Rc::clone(&self.conversation_actions);
        let factory = Rc::new(
            RowFactory::new(
                Vec::new(),
                scope.clone(),
                Rc::clone(&workspace.markdown),
                Rc::clone(&workspace.status_shimmers),
                theme,
                reduced_motion,
                workspace.selections.source_allocator(),
                Rc::new(move || {
                    crate::shell::rebuild_stats::record_event("inv_rows");
                    invalidated.set(true);
                    invalidate_settled.borrow_mut().clear();
                }),
                on_link,
                on_color,
                workspace.code_action_emit(),
            )
            .with_active(row_token)
            .with_settled(settled, self.row_style_epoch)
            .with_row_cache(Rc::clone(&self.row_cache))
            .with_sources(
                source_provider.clone(),
                workspace.selections.conversation,
                Rc::new(move |action| {
                    source_actions
                        .borrow_mut()
                        .push_back(ConversationAction::Source(action));
                }),
            )?
            .with_stored(Rc::clone(&self.stored), source_provider)?
            .with_dispatch(Rc::clone(&dispatch))
            .with_copy({
                let copy_actions = Rc::clone(&self.conversation_actions);
                let copy_scope = scope.clone();
                Rc::new(move |source| {
                    copy_actions
                        .borrow_mut()
                        .push_back(ConversationAction::Copy(copy_scope.clone(), source));
                })
            })
            .with_blob_images(
                blob_images,
                Rc::new(move |item, revision, action| {
                    blob_actions
                        .borrow_mut()
                        .push_back(ConversationAction::Blob(
                            blob_scope.clone(),
                            item,
                            revision,
                            action,
                        ));
                }),
            )
            .with_lazy_rows(&workspace.conversation, core, self.composer_height)?
            .with_reveals(
                &workspace.conversation,
                core,
                Rc::clone(&self.mounted_reveals),
                now,
                swift,
            )?
            .with_observations(self.observations.clone())
            .with_work(
                Rc::clone(&self.row_motion),
                crate::conversation::adapter::agent_names(&workspace.conversation),
                now,
                seconds,
                artisan_api::now(),
            )
            .with_engine(Rc::clone(&self.markdown_engine))
            .with_media(Rc::new(move |key, width| {
                let (fetcher, revision) = providers.get(&key.item)?;
                let revision = *revision;
                let actions = Rc::clone(&media_actions);
                let scope = media_scope.clone();
                let key = key.clone();
                Some(crate::ui::markdown::presenter::MediaServices {
                    width,
                    fetcher: fetcher.clone(),
                    effect: Rc::new(move |effect| {
                        actions.borrow_mut().push_back(ConversationAction::Media(
                            scope.clone(),
                            key.clone(),
                            revision,
                            effect,
                        ));
                    }),
                    images: images.clone(),
                    player: media_resources.player.clone(),
                    audio_assets: media_resources.audio_assets.clone(),
                    model_assets: media_resources.model_assets.clone(),
                    image_assets: media_resources.image_assets.clone(),
                    model_capability: Rc::clone(&media_resources.model_capability),
                    scale: media_resources.scale,
                    wake: wake.clone(),
                })
            })),
        );
        let viewport = factory.viewport_callback()?;
        self.transcript_shape = Some(transcript_shape(&workspace.conversation));
        if transcript_only {
            let heights = workspace.conversation.row_heights.clone();
            let revisions = Rc::clone(&factory);
            let list = crate::conversation::view::transcript::layout_batched_list(
                &workspace.conversation,
                24.0,
                factory.layout_callback(),
                Rc::new(move |index, key| revisions.revision(index, key)),
                viewport,
                Rc::new(move |_, key| heights.get(key).copied().unwrap_or(24.0)),
            );
            self.replace_rows(Some(factory));
            return Ok(list);
        }
        let jump = crate::shell::thread_navigation::jump_to_latest(
            &workspace.conversation,
            core,
            workspace.composer.is_some(),
            theme,
            Rc::clone(&dispatch),
        );
        let mut controls = Vec::new();
        controls.extend(jump);
        if workspace.conversation.agent.is_none() {
            if let Some(error) = core
                .host_if_instance(&scope.host, scope.instance)
                .filter(|host| host.is_live())
                .and_then(|host| host.thread_summary(&scope.thread))
                .and_then(|summary| summary.error.as_ref())
            {
                let actions = Rc::clone(&self.conversation_actions);
                let scope = scope.clone();
                let retry =
                    crate::ui::button::Button::text("retry-thread-controls", theme, "Retry")
                        .variant(crate::ui::button::ButtonVariant::Ghost)
                        .on_activate(move |_| {
                            actions
                                .borrow_mut()
                                .push_back(ConversationAction::RetryControls(scope.clone()))
                        })
                        .build();
                controls.push(
                    crate::ui::page::notice(
                        &theme,
                        format!("Couldn't refresh questions and goals: {error}"),
                    )
                    .action(retry)
                    .build(),
                );
            }
        }
        let above_composer = if controls.is_empty() {
            None
        } else {
            let mut column = aqua_ui::element::Box::new()
                .layout("column")
                .width(aqua_ui::style::Length::Pct(100.0))
                .gap(8.0);
            for control in controls {
                column = column.child(control);
            }
            Some(column.build())
        };
        let heights = workspace.conversation.row_heights.clone();
        let (questionnaires, attachments) =
            workspace.composer_content(ui, theme, reduced_motion)?;
        let parts = view::ComposerParts {
            nested: workspace.conversation.agent.is_some(),
            placeholder: String::new(),
            questionnaires,
            attachments,
            mention_overlay: None,
            thread_error: None,
        };
        let center = if workspace.conversation.rows.is_empty() {
            self.thread_landing.reset();
            let loading = workspace.conversation.view.as_ref().is_some_and(|view| {
                let window = view.window();
                window.in_flight || window.oldest.is_none() && !window.exhausted
            });
            let body = match &workspace.conversation.load_error {
                Some(error) => {
                    self.thread_loading.reset();
                    view::conversation_notice(&theme, error.clone(), true)
                }
                None if loading => view::centered_body(self.thread_loading.build(
                    theme,
                    now,
                    reduced_motion,
                    "conversation-loading",
                )),
                None => {
                    self.thread_loading.reset();
                    view::conversation_notice(&theme, "No messages yet.".to_owned(), false)
                }
            };
            view::conversation_body(
                workspace,
                core,
                &theme,
                width,
                body,
                Vec::new(),
                parts,
                Rc::clone(&self.composer_measurement),
                above_composer,
            )?
        } else {
            self.thread_loading.reset();
            let mut overlays = vec![
                self.transcript_edges.band(
                    "conversation-edge-top",
                    crate::ui::scroll_edge::Edge::Top,
                    theme.colors.background,
                ),
                self.transcript_edges.band(
                    "conversation-edge-bottom",
                    crate::ui::scroll_edge::Edge::Bottom,
                    theme.colors.background,
                ),
            ];
            if let Some(error) = &workspace.conversation.load_error {
                overlays.push(view::conversation_load_error(&theme, error.clone()));
            }
            if let Some(viewport) = self.transcript_viewport {
                let invalidated = Rc::clone(&self.navigation_invalidated);
                if let Some(navigation) = self.navigation.build(
                    &workspace.conversation,
                    core,
                    viewport,
                    window_height,
                    self.composer_height,
                    if width >= 640.0 { 24.0 } else { 16.0 },
                    theme,
                    reduced_motion,
                    now,
                    Rc::clone(&dispatch),
                    Rc::new(move || {
                        crate::shell::rebuild_stats::record_event("inv_nav_cb");
                        invalidated.set(true)
                    }),
                ) {
                    overlays.push(navigation);
                }
            }
            if workspace.conversation.is_landing() {
                overlays.push(
                    aqua_ui::element::Box::new()
                        .width(aqua_ui::style::Length::Pct(100.0))
                        .height(aqua_ui::style::Length::Pct(100.0))
                        .style(aqua_ui::style::Style::new().absolute_at(
                            Some(0.0),
                            Some(0.0),
                            Some(0.0),
                            Some(0.0),
                        ))
                        .child(view::centered_body(self.thread_landing.build(
                            theme,
                            now,
                            reduced_motion,
                            "conversation-landing",
                        )))
                        .build(),
                );
            } else {
                self.thread_landing.reset();
            }
            view::conversation_virtual(
                workspace,
                core,
                &theme,
                width,
                24.0,
                factory.layout_callback(),
                {
                    let revisions = Rc::clone(&factory);
                    Rc::new(move |index, key| revisions.revision(index, key))
                },
                viewport,
                Rc::new(move |_, key| heights.get(key).copied().unwrap_or(24.0)),
                overlays,
                parts,
                Rc::clone(&self.composer_measurement),
                above_composer,
            )?
        };
        self.replace_rows(Some(factory));
        Ok(center)
    }

    pub fn conversation_callbacks(
        &self,
        scope: crate::conversation::presentation::PresentationScope,
    ) -> (
        Rc<dyn Fn(crate::ui::markdown::visual::LinkAction)>,
        Rc<dyn Fn(crate::ui::rich::actions::ShowCopyMenu)>,
    ) {
        let links = Rc::clone(&self.conversation_actions);
        let colors = Rc::clone(&self.conversation_actions);
        let link_scope = scope.clone();
        (
            Rc::new(move |action| {
                links
                    .borrow_mut()
                    .push_back(ConversationAction::Link(link_scope.clone(), action))
            }),
            Rc::new(move |action| {
                colors
                    .borrow_mut()
                    .push_back(ConversationAction::Color(scope.clone(), action))
            }),
        )
    }

    fn dispose_color_menu(&mut self, ui: &mut UiApp) {
        if let Some(menu) = self.color_menu.take() {
            menu.dispose(ui);
        }
        self.color_effects.borrow_mut().clear();
        self.color_scope = None;
    }

    fn dispatch_blob(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        scope: crate::conversation::presentation::PresentationScope,
        id: artisan_api::ItemId,
        revision: u64,
        action: crate::conversation::view::factory::BlobAction,
    ) -> bool {
        use crate::conversation::view::factory::BlobAction;
        let Some(workspace) = shell.entry.workspace() else {
            return false;
        };
        if !scope.current(&workspace.conversation, &shell.core) {
            return false;
        }
        let hash = match &action {
            BlobAction::Load { hash, .. } | BlobAction::Open { hash, .. } => hash,
        };
        let current = workspace
            .conversation
            .view
            .as_ref()
            .and_then(|view| view.item(&id))
            .is_some_and(|item| {
                item.seq == revision
                    && match &item.kind {
                        artisan_api::ItemKind::UserMessage { images, .. } => images.contains(hash),
                        artisan_api::ItemKind::Image { blob, .. } => blob == hash,
                        _ => false,
                    }
            });
        if !current {
            return false;
        }
        match action {
            BlobAction::Load { hash, retry } => {
                let Some(host) = shell.core.host_mut_if_instance(&scope.host, scope.instance)
                else {
                    return false;
                };
                let before = host.image(&hash);
                host.load_image(hash.clone(), retry);
                before.as_ref().map(std::mem::discriminant)
                    != host.image(&hash).as_ref().map(std::mem::discriminant)
            }
            BlobAction::Open {
                image, opener_key, ..
            } => {
                self.preview_generation = self.preview_generation.wrapping_add(1);
                let view = crate::ui::media::preview::Preview::from_ready(
                    self.preview_generation,
                    scope.host.clone(),
                    scope.instance,
                    image,
                    opener_key,
                    std::time::Instant::now(),
                );
                self.dispose_color_menu(ui);
                self.image_preview = Some(ImagePreview {
                    scope,
                    owner: None,
                    view,
                    focus_pending: true,
                });
                true
            }
        }
    }

    fn dispatch_conversation(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) -> bool {
        let _tm = crate::shell::application::Tm::new("v.dispatch_conv");
        use crate::ui::color_copy_menu::ColorCopyEffect;
        use crate::ui::markdown::visual::LinkAction;
        self.connection.dispatch(&mut shell.core);
        let mut changed = crate::shell::rebuild_stats::note(
            "conv_invalidated",
            self.conversation_invalidated.replace(false),
        );
        if self.navigation_invalidated.replace(false) {
            changed |= crate::shell::rebuild_stats::note(
                "navigation",
                self.refresh_navigation(shell, ui, ctx, window),
            );
        }
        self.bind_row_sources(ui, ctx, window);
        if let Some(workspace) = shell.entry.workspace_mut() {
            changed |= crate::shell::rebuild_stats::note(
                "conv_stored",
                self.stored
                    .borrow_mut()
                    .drain_publish(&mut workspace.conversation, &shell.core),
            );
        }
        self.stored.borrow_mut().drain_cleanup(ui);
        changed |= crate::shell::rebuild_stats::note(
            "conv_row_sources",
            self.drain_row_sources(shell, ctx, window),
        );
        self.source_dialogs
            .retain(|export| Self::source_export_is_current(shell, export));
        for (export, result) in self.source_dialogs.drain() {
            changed |= Self::finish_source_dialog(shell, export, result);
        }
        let row_actions = std::mem::take(&mut *self.row_actions.borrow_mut());
        if let Some(workspace) = shell.entry.workspace_mut() {
            let reduced =
                ui.host().motion().reduced_motion() == aqua_ui::motion::ReducedMotion::Reduce;
            for action in row_actions {
                changed |= action.dispatch(
                    &mut workspace.conversation,
                    &shell.core,
                    reduced,
                    std::time::Instant::now(),
                );
            }
        }
        let actions = std::mem::take(&mut *self.conversation_actions.borrow_mut());
        for action in actions {
            let scope = match &action {
                ConversationAction::Source(action) => &action.scope,
                ConversationAction::Link(scope, _)
                | ConversationAction::Color(scope, _)
                | ConversationAction::Agent(scope, _)
                | ConversationAction::Copy(scope, _)
                | ConversationAction::Media(scope, _, _, _)
                | ConversationAction::Blob(scope, _, _, _)
                | ConversationAction::RetryControls(scope) => scope,
            };
            if !shell
                .entry
                .workspace()
                .is_some_and(|workspace| scope.current(&workspace.conversation, &shell.core))
            {
                continue;
            }
            let action = match action {
                ConversationAction::Source(action) => {
                    let Some(item) = shell
                        .entry
                        .workspace()
                        .and_then(|workspace| workspace.conversation.view.as_ref())
                        .and_then(|view| view.item(&action.item))
                    else {
                        continue;
                    };
                    let scope = action.scope.clone();
                    let (source_changed, effect) = action.dispatch(&shell.core, &scope, item);
                    changed |= source_changed;
                    if let Some(effect) = effect {
                        changed |= self.source_effect(shell, ctx, window, effect);
                    }
                    continue;
                }
                ConversationAction::Media(scope, key, revision, effect) => {
                    let Some(workspace) = shell.entry.workspace() else {
                        continue;
                    };
                    if !workspace
                        .conversation
                        .view
                        .as_ref()
                        .and_then(|view| view.item(&key.item))
                        .is_some_and(|item| item.seq == revision)
                    {
                        continue;
                    }
                    match effect {
                        crate::ui::media::preview::Effect::Open(request) => {
                            let owner = std::sync::Arc::clone(request.fetcher.owner());
                            self.preview_generation = self.preview_generation.wrapping_add(1);
                            if let Some(view) = crate::ui::media::preview::Preview::from_request(
                                self.preview_generation,
                                scope.host.clone(),
                                scope.instance,
                                workspace.images.as_ref().clone(),
                                request,
                                std::time::Instant::now(),
                            ) {
                                self.dispose_color_menu(ui);
                                self.image_preview = Some(ImagePreview {
                                    scope,
                                    owner: Some(owner),
                                    view,
                                    focus_pending: true,
                                });
                                changed = true;
                            }
                            continue;
                        }
                        crate::ui::media::preview::Effect::ShowCopyMenu(request) => {
                            ConversationAction::Color(scope, request)
                        }
                        crate::ui::media::preview::Effect::Clipboard(text) => {
                            if let Err(error) = ctx.clipboard().set_text(text) {
                                tracing::warn!(%error, "could not copy media path");
                            }
                            continue;
                        }
                    }
                }
                action => action,
            };
            match action {
                ConversationAction::Media(_, _, _, _) | ConversationAction::Source(_) => continue,
                ConversationAction::Blob(scope, item, revision, action) => {
                    changed |= self.dispatch_blob(shell, ui, scope, item, revision, action);
                }
                ConversationAction::RetryControls(scope) => {
                    if let Some(host) = shell.core.host_mut_if_instance(&scope.host, scope.instance)
                    {
                        host.load_thread_summary(&scope.thread, true);
                        changed = true;
                    }
                }
                ConversationAction::Copy(_, source) => {
                    if let Err(error) = ctx.clipboard().set_text(source.into_text()) {
                        tracing::warn!(%error, "could not copy full content");
                    }
                }
                ConversationAction::Agent(_, agent) => {
                    if let Some(workspace) = shell.entry.workspace_mut() {
                        let before = workspace.conversation.agent.clone();
                        workspace.conversation.open_agent(&shell.core, agent);
                        changed |= before != workspace.conversation.agent;
                    }
                }
                ConversationAction::Link(_, LinkAction::OpenReference(reference)) => {
                    let scope = reference
                        .scope
                        .map(|scope| (crate::core::HostId(scope.host), scope.thread));
                    shell.open_reference(ctx, window, reference.path, reference.line, scope);
                    changed = true;
                }
                ConversationAction::Link(_, LinkAction::OpenUrl(url)) => {
                    if let Err(error) = crate::shell::open_url(&url) {
                        tracing::warn!(%error, "could not open URL");
                    }
                }
                ConversationAction::Color(scope, request) => {
                    self.dispose_color_menu(ui);
                    if let Some(owner) = ui.find_debug_selector("workspace-root") {
                        let effects = Rc::clone(&self.color_effects);
                        let reduced = ui.host().motion().reduced_motion()
                            == aqua_ui::motion::ReducedMotion::Reduce;
                        match crate::ui::color_copy_menu::ColorCopyMenuHost::open(
                            ui,
                            owner,
                            request,
                            shell.preferences.theme(),
                            reduced,
                            ctx.now().as_secs_f64(),
                            move |effect| effects.borrow_mut().push_back(effect),
                        ) {
                            Ok(menu) => {
                                self.color_scope = menu.as_ref().map(|_| scope);
                                self.color_menu = menu;
                            }
                            Err(error) => tracing::warn!(%error, "could not open color copy menu"),
                        }
                    }
                }
            }
        }
        changed |= crate::shell::rebuild_stats::note(
            "conv_image_preview",
            self.sync_image_preview(shell, ui, ctx, window),
        );
        let effects = std::mem::take(&mut *self.color_effects.borrow_mut());
        for effect in effects {
            match effect {
                ColorCopyEffect::Clipboard(text) => {
                    if let Err(error) = ctx.clipboard().set_text(text) {
                        tracing::warn!(%error, "could not copy color");
                    }
                }
                ColorCopyEffect::RequestFrame => {
                    ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION)
                }
                ColorCopyEffect::Dismissed => {
                    self.color_menu = None;
                    self.color_scope = None;
                }
            }
        }
        changed
    }
    pub fn replace_rows(
        &mut self,
        rows: Option<Rc<crate::conversation::view::factory::RowFactory>>,
    ) {
        if let Some(previous) = self.rows.take() {
            previous.dispose();
        }
        if rows.is_none() {
            if let Some(token) = self.row_token.take() {
                token.set(false);
            }
            self.row_settled.borrow_mut().clear();
            self.row_cache.borrow_mut().rebind(None);
            self.stored.borrow_mut().rebind(None);
        }
        self.rows = rows;
    }

    fn sync_context(
        &mut self,
        shell: &Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) {
        let _tm = crate::shell::application::Tm::new("v.context");
        self.ui_now = ctx.now();
        let reduced = ui.host().motion().reduced_motion() == aqua_ui::motion::ReducedMotion::Reduce;
        let now = std::time::Instant::now();
        if let Some(deadline) = [
            self.connection.deadline(now, reduced),
            self.thread_loading.deadline(now, reduced),
            self.thread_landing.deadline(now, reduced),
        ]
        .into_iter()
        .flatten()
        .min()
        {
            ctx.request_animation_deadline(
                window,
                ctx.now() + deadline.saturating_duration_since(now),
            );
        }
        self.request_landing_deadline(shell, ctx, window, now);
        if self.connection.animating(now, reduced)
            || self.thread_loading.animating(now, reduced)
            || self.thread_landing.animating(now, reduced)
        {
            ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION);
        }
        match self.context_tooltip.sync(
            ui,
            self.context_reading.as_ref(),
            shell.preferences.theme(),
            ctx.now().as_secs_f64(),
            reduced,
        ) {
            Ok(true) => ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION),
            Ok(false) => {}
            Err(error) => tracing::warn!(%error, "could not synchronize context meter"),
        }
    }

    fn sync_scroll(
        &mut self,
        conversation: &mut crate::conversation::controller::Conversation,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) {
        let _tm = crate::shell::application::Tm::new("v.scroll");
        use crate::conversation::scroll::Step;
        self.scroll_requests.extend(conversation.scroll_actions());
        let Some(node) = crate::shell::wheel::keyed_node(ui, "conversation-transcript") else {
            return;
        };
        let Some(rect) = ui.tree().rect(node) else {
            return;
        };
        let Some(content) = ui.tree().content_size(node) else {
            return;
        };
        if rect.h <= 0.0 {
            return;
        }
        let maximum = (content.h - rect.h).max(0.0);
        let reduced = ui.host().motion().reduced_motion() == aqua_ui::motion::ReducedMotion::Reduce;
        let now = std::time::Instant::now();
        let mut changed = false;
        while let Some(action) = self.scroll_requests.front() {
            let Some(directive) = crate::conversation::adapter::scroll_directive(
                conversation,
                action.clone(),
                reduced,
            ) else {
                break;
            };
            let x = ui.tree().scroll_offset(node).map_or(0.0, |offset| offset.x);
            let Some(moved) = crate::conversation::scroll::apply_directive(
                ui,
                &mut self.smooth_scroll,
                &directive,
                now,
                x,
            ) else {
                break;
            };
            changed |= moved;
            self.scroll_requests.pop_front();
        }
        let current = ui.tree().scroll_offset(node).unwrap_or_default();
        let following = conversation
            .view
            .as_ref()
            .is_some_and(|view| view.viewport.state().is_following());
        let step = self.smooth_scroll.step(
            following,
            reduced,
            conversation.is_landing(),
            !conversation.disclosures.disclosures.is_empty(),
            crate::conversation::scroll::following_tail(ui),
            -current.y,
            maximum,
            now,
        );
        let animating = matches!(
            step,
            Step::Scroll {
                continue_animation: true,
                ..
            }
        );
        changed |= crate::conversation::scroll::apply_step(ui, step, current.x);
        if changed || animating {
            ctx.request_frame(
                window,
                if animating {
                    aqua::prelude::FrameReason::ANIMATION
                } else {
                    aqua::prelude::FrameReason::EXPLICIT
                },
            );
        }
    }

    fn sync_edges(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) {
        let _tm = crate::shell::application::Tm::new("v.edges");
        use crate::ui::scroll_edge::EdgeSurface;
        let reduced = ui.host().motion().reduced_motion() == aqua_ui::motion::ReducedMotion::Reduce;
        let now = std::time::Instant::now();
        let theme = shell.preferences.theme();
        let Some(workspace) = shell.entry.workspace() else {
            return;
        };
        let mut moving = EdgeSurface {
            edges: &self.transcript_edges,
            scroller: "conversation-transcript",
            top: "conversation-edge-top",
            bottom: "conversation-edge-bottom",
            top_color: theme.colors.background,
            bottom_color: theme.colors.background,
        }
        .advance(ui, reduced, now);
        moving |= EdgeSurface {
            edges: &workspace.sidebar.threads_edges,
            scroller: crate::shell::wheel::THREADS,
            top: "sidebar-threads-edge-top",
            bottom: "sidebar-threads-edge-bottom",
            top_color: theme.colors.background,
            bottom_color: theme.colors.background,
        }
        .advance(ui, reduced, now);
        if let Some(composer) = &workspace.composer {
            moving |= composer.advance_project_edges(ui, &theme, reduced, now);
        }
        if moving {
            ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION);
        }
    }

    fn sync_sidebar(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) -> bool {
        let _tm = crate::shell::application::Tm::new("v.sidebar");
        let Some(workspace) = shell.entry.workspace_mut() else {
            self.sidebar_overlays.dispose(ui);
            if let Some(preview) = self.sidebar_drag.take() {
                preview.dispose(ui);
            }
            self.drag_position = None;
            self.navigation.dispose(ui);
            self.transcript_viewport = None;
            self.mounted_reveals.borrow_mut().rebind(None);
            self.row_cache.borrow_mut().rebind(None);
            return false;
        };
        let now = ctx.now().as_secs_f64();
        let mut changed = false;
        changed |= workspace.dispatch_sidebar_actions(&mut shell.core);
        let Some(owner) = ui.find_debug_selector("workspace-root") else {
            return changed;
        };
        let snapshot = ui.drag_snapshot();
        let instance = snapshot.as_ref().and_then(|snapshot| {
            let payload = snapshot.payload.get::<crate::core::DraggedThread>()?;
            workspace.sidebar.drag_instance(&shell.core, payload)
        });
        let mut preview_changed = false;
        for effect in workspace.sidebar.drain_effects() {
            match effect {
                crate::sidebar::controller::SidebarEffect::ShowDragPreview { payload, .. } => {
                    if let Some(preview) = self.sidebar_drag.take() {
                        preview.dispose(ui);
                    }
                    if let Some((snapshot, instance)) =
                        snapshot.as_ref().zip(instance).filter(|(snapshot, _)| {
                            snapshot.payload.get::<crate::core::DraggedThread>() == Some(&payload)
                        })
                    {
                        match crate::sidebar::drag_preview::DragPreviewHost::open(
                            ui,
                            owner,
                            instance,
                            snapshot,
                            shell.preferences.theme(),
                        ) {
                            Ok(preview) => self.sidebar_drag = preview,
                            Err(error) => tracing::warn!(%error, "could not open drag preview"),
                        }
                    }
                    preview_changed = true;
                }
                crate::sidebar::controller::SidebarEffect::HideDragPreview => {
                    if let Some(preview) = self.sidebar_drag.take() {
                        preview.dispose(ui);
                        preview_changed = true;
                    }
                }
            }
        }
        if let Some(preview) = &mut self.sidebar_drag {
            match preview.sync(ui, instance, snapshot.as_ref()) {
                Ok(true) => {}
                Ok(false) => {
                    self.sidebar_drag = None;
                    preview_changed = true;
                }
                Err(error) => {
                    tracing::warn!(%error, "could not move drag preview");
                    if let Some(preview) = self.sidebar_drag.take() {
                        preview.dispose(ui);
                    }
                    preview_changed = true;
                }
            }
        }
        let position = snapshot.as_ref().map(|snapshot| snapshot.pointer.position);
        if preview_changed || self.sidebar_drag.is_some() && self.drag_position != position {
            ctx.request_frame(window, aqua::prelude::FrameReason::INPUT);
        }
        self.drag_position = position;
        let profile = ui.find_debug_selector("artisan-desktop-profile-trigger");
        if let Err(error) = self.sidebar_overlays.sync(
            ui,
            owner,
            profile,
            &workspace.sidebar,
            &shell.core,
            shell.preferences.theme(),
            now,
            workspace.sidebar_emit(false),
        ) {
            tracing::warn!(%error, "could not synchronize sidebar overlays");
        }
        let reduced = ui.host().motion().reduced_motion() == aqua_ui::motion::ReducedMotion::Reduce;
        match self.sidebar_overlays.advance(
            ui,
            &mut workspace.sidebar,
            &shell.preferences.theme(),
            reduced,
            now,
        ) {
            Ok(true) => ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION),
            Ok(false) => {}
            Err(error) => tracing::warn!(%error, "could not advance sidebar overlays"),
        }
        if let Some(editor) = self.sidebar_overlays.rename_editor() {
            if let Some(key) = editor.text_key() {
                let key = aqua_ui::tree::hash_key(key);
                let nodes: Vec<_> = ui
                    .tree()
                    .traverse()
                    .into_iter()
                    .filter(|node| {
                        ui.tree()
                            .get(*node)
                            .is_some_and(|record| record.key == Some(key))
                    })
                    .collect();
                if let [node] = nodes.as_slice() {
                    if editor.sync(ui.tree_mut(), *node) {
                        ctx.request_frame(window, aqua::prelude::FrameReason::INPUT);
                    }
                }
            }
        }
        changed
    }

    fn sync_navigation(
        &mut self,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
        theme: crate::ui::theme::ArtisanTheme,
    ) -> bool {
        let _tm = crate::shell::application::Tm::new("v.navigation");
        let reduced = ui.host().motion().reduced_motion() == aqua_ui::motion::ReducedMotion::Reduce;
        match self.navigation.sync(
            ui,
            &theme,
            std::time::Instant::now(),
            ctx.now().as_secs_f64(),
            reduced,
        ) {
            Ok(frame) => {
                if frame.changed || frame.animating || frame.ticking {
                    ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION);
                }
                frame.animating
            }
            Err(error) => {
                tracing::warn!(%error, "could not synchronize turn navigator");
                false
            }
        }
    }

    fn sync_resource_jump(&mut self, ui: &mut UiApp, ctx: &mut AppCtx<'_>, window: AquaWindowId) {
        let _tm = crate::shell::application::Tm::new("v.resource_jump");
        let Some((_, state)) = &mut self.resource else {
            return;
        };
        if state.pending_jump().is_none() {
            return;
        }
        let changed = state.sync_jump(ui);
        if changed {
            ctx.request_frame(window, aqua::prelude::FrameReason::EXPLICIT);
        }
    }
    fn close_resource(&mut self, ui: &mut UiApp) {
        if let Some(dialog) = self.resource_dialog.take() {
            dialog.dispose(ui);
        }
        if let Some((_, mut resource)) = self.resource.take() {
            resource.release(ui);
        }
        self.resource_content = None;
        self.resource_events.borrow_mut().clear();
    }

    fn resource_emit(&self) -> Rc<dyn Fn(crate::shell::resource_preview::view::Action)> {
        let events = Rc::clone(&self.resource_events);
        Rc::new(move |event| events.borrow_mut().push_back(event))
    }

    fn dispatch_resource(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
    ) -> bool {
        use crate::shell::resource_preview::view::Action;
        let events = std::mem::take(&mut *self.resource_events.borrow_mut());
        let mut changed = false;
        for event in events {
            let generation = match &event {
                Action::CopyPath { generation, .. }
                | Action::CopyFile { generation, .. }
                | Action::Dismiss { generation } => *generation,
            };
            if self.resource.as_ref().map(|(current, _)| *current) != Some(generation)
                || !shell.entry.workspace().is_some_and(|workspace| {
                    workspace.resource_preview.is_some()
                        && workspace.resource_preview_generation() == generation
                })
            {
                continue;
            }
            match event {
                Action::CopyPath { text, .. } => {
                    if let Err(error) = ctx.clipboard().set_text(text) {
                        tracing::warn!(%error, "could not copy resource path");
                    }
                }
                Action::CopyFile { text, .. } => {
                    if let Err(error) = ctx.clipboard().set_text(text.to_string()) {
                        tracing::warn!(%error, "could not copy resource");
                    }
                }
                Action::Dismiss { .. } => {
                    self.close_resource(ui);
                    shell.dismiss_reference();
                    changed = true;
                }
            }
        }
        changed
    }

    fn mount_resource(&mut self, shell: &Shell, ui: &mut UiApp) {
        let Some((content, title)) = self.resource_content.take() else {
            return;
        };
        if let Some(dialog) = &self.resource_dialog {
            if dialog.is_open(ui) {
                if let Err(error) = dialog.refresh_content(ui, content) {
                    tracing::warn!(%error, "could not refresh resource preview");
                }
                return;
            }
            self.resource_dialog = None;
        }
        let Some(owner) = ui.find_debug_selector("workspace-root") else {
            self.resource_content = Some((content, title));
            return;
        };
        let emit = self.resource_emit();
        let generation = self.resource.as_ref().map(|(generation, _)| *generation);
        let dialog = crate::ui::dialog::Dialog::new(
            "resource-preview",
            shell.preferences.theme(),
            title,
            content,
        )
        .max_width(1100.0)
        .on_dismiss(move |_| {
            if let Some(generation) = generation {
                emit(crate::shell::resource_preview::view::Action::Dismiss { generation });
            }
        });
        match crate::ui::dialog::DialogHost::open(ui, owner, dialog) {
            Ok(dialog) => self.resource_dialog = Some(dialog),
            Err(error) => tracing::warn!(%error, "could not open resource preview"),
        }
    }
    fn close_palette(&mut self, ui: &mut UiApp) {
        if let Some(dialog) = self.palette_dialog.take() {
            dialog.dispose(ui);
        }
        if let Some(palette) = self.palette.take() {
            palette.close();
        }
        self.palette_content = None;
        self.palette_events.borrow_mut().clear();
    }

    fn palette_emit(&self) -> Rc<dyn Fn(crate::shell::palette::view::Event)> {
        let events = Rc::clone(&self.palette_events);
        Rc::new(move |event| events.borrow_mut().push_back(event))
    }

    fn dispatch_palette(&mut self, shell: &mut Shell, ui: &mut UiApp) -> bool {
        use crate::shell::palette::view::Effect;
        let events = std::mem::take(&mut *self.palette_events.borrow_mut());
        let mut changed = false;
        for event in events {
            let outcome = match (self.palette.as_ref(), shell.entry.workspace_mut()) {
                (Some(view), Some(workspace)) => match workspace.palette.as_mut() {
                    Some(palette) => view.dispatch(palette, event),
                    None => continue,
                },
                _ => continue,
            };
            changed |= outcome.changed;
            match outcome.effect {
                Effect::None => {}
                Effect::Search(query) => {
                    if let Some(workspace) = shell.entry.workspace_mut() {
                        workspace.search_palette(&shell.core, query);
                    }
                }
                Effect::Close(screen) => {
                    self.close_palette(ui);
                    shell.dismiss_palette();
                    if let Some(screen) = screen {
                        shell.navigate(screen);
                    }
                }
            }
        }
        changed
    }

    fn mount_palette(&mut self, shell: &Shell, ui: &mut UiApp) {
        let Some(content) = self.palette_content.take() else {
            return;
        };
        if let Some(dialog) = &self.palette_dialog {
            if dialog.is_open(ui) {
                if let Err(error) = dialog.refresh_content(ui, content) {
                    tracing::warn!(%error, "could not refresh command palette");
                }
                return;
            }
            self.palette_dialog = None;
        }
        let Some(owner) = ui.find_debug_selector("workspace-root") else {
            self.palette_content = Some(content);
            return;
        };
        let emit = self.palette_emit();
        let session = self.palette.as_ref().map(|view| view.session());
        let dialog = crate::ui::dialog::Dialog::new(
            "command-palette",
            shell.preferences.theme(),
            "Search threads and commands",
            content,
        )
        .on_dismiss(move |_| {
            if let Some(session) = session {
                emit(crate::shell::palette::view::Event {
                    session,
                    action: crate::shell::palette::view::Action::Dismiss,
                });
            }
        });
        match crate::ui::dialog::DialogHost::open(ui, owner, dialog) {
            Ok(dialog) => {
                self.palette_dialog = Some(dialog);
                if let Some(view) = &self.palette {
                    view.sync_query(ui);
                    view.focus_query(ui);
                }
            }
            Err(error) => tracing::warn!(%error, "could not open command palette"),
        }
    }
}

fn transcript_shape(conversation: &crate::conversation::controller::Conversation) -> (u64, usize) {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    conversation.is_landing().hash(&mut hasher);
    for (row, _) in &conversation.rows {
        row.key().hash(&mut hasher);
    }
    (hasher.finish(), conversation.rows.len())
}

impl RootViews {
    fn app_media(shell: &Shell, window: &ObservedWindowState) -> AppMedia {
        let capability = Rc::clone(&shell.meshes_supported);
        AppMedia {
            player: shell.audio.clone(),
            audio_assets: shell.audio_assets.clone(),
            model_assets: shell.models.clone(),
            image_assets: shell.images().clone(),
            model_capability: Rc::new(move || {
                capability.get().map(|supported| {
                    if supported {
                        crate::ui::media::model::load::RendererCapability::Mesh
                    } else {
                        crate::ui::media::model::load::RendererCapability::Software
                    }
                })
            }),
            scale: window.scale_factor as f32,
        }
    }

    fn transcript_refreshable(&self, shell: &Shell, ui: &UiApp) -> bool {
        self.image_preview.is_none()
            && Self::preview_root(ui).is_none()
            && self.color_scope.is_none()
            && self.rows.is_some()
            && matches!(shell.core.workspace_screen(), Screen::Thread { .. })
            && shell.entry.workspace().is_some_and(|workspace| {
                self.workspace_key == Some(workspace.selections.key)
                    && !workspace.conversation.rows.is_empty()
                    && !workspace.conversation.is_landing()
                    && workspace.conversation.host_is_current(&shell.core)
                    && crate::shell::thread_navigation::latest_visible(
                        &workspace.conversation,
                        &shell.core,
                        workspace.composer.is_some(),
                    ) == crate::shell::wheel::keyed_node(ui, "jump-latest").is_some()
                    && self.transcript_shape == Some(transcript_shape(&workspace.conversation))
            })
    }
}

impl SectionViews for RootViews {
    fn refresh_transcript(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        window: &ObservedWindowState,
    ) -> bool {
        if !self.transcript_refreshable(shell, ui) {
            crate::shell::rebuild_stats::record_event("tf_not_refreshable");
            return false;
        }
        let theme = shell.preferences.theme();
        let swift = shell.preferences.streaming == crate::shell::preferences::StreamingMode::Swift;
        let reduced = ui.host().motion().reduced_motion() == aqua_ui::motion::ReducedMotion::Reduce;
        let app_media = Self::app_media(shell, window);
        let Some(workspace) = shell.entry.workspace_mut() else {
            return false;
        };
        let element = match self.thread_content(
            workspace,
            &shell.core,
            ui,
            theme,
            window.inner_size.width as f32,
            window.inner_size.height as f32,
            reduced,
            swift,
            app_media,
            std::time::Instant::now(),
            true,
        ) {
            Ok(element) => element,
            Err(error) => {
                tracing::warn!(%error, "could not build transcript");
                return false;
            }
        };
        match ui.refresh_subtree("conversation-transcript", element) {
            Ok(refreshed) => {
                if !refreshed {
                    crate::shell::rebuild_stats::record_event("tf_subtree_false");
                }
                refreshed
            }
            Err(error) => {
                tracing::warn!(%error, "could not refresh transcript");
                false
            }
        }
    }

    fn refresh_sidebar(
        &self,
        shell: &Shell,
        ui: &mut UiApp,
        window: &ObservedWindowState,
    ) -> bool {
        let Some(workspace) = shell.entry.workspace() else {
            return false;
        };
        if matches!(shell.core.screen(), Screen::Settings(_)) {
            return false;
        }
        let reduced = ui.host().motion().reduced_motion() == aqua_ui::motion::ReducedMotion::Reduce;
        let element = workspace.sidebar_view(
            &shell.core,
            shell.preferences.theme(),
            aqua_core::Size::new(
                window.inner_size.width as f32,
                window.inner_size.height as f32,
            ),
            workspace.show_sidebar,
            reduced,
            std::time::Instant::now(),
            false,
        );
        match ui.refresh_subtree(crate::sidebar::view::ROOT_KEY, element) {
            Ok(refreshed) => refreshed,
            Err(error) => {
                tracing::warn!(%error, "could not refresh sidebar");
                false
            }
        }
    }

    fn resize_needs_rebuild(&self, shell: &Shell, window: &ObservedWindowState) -> bool {
        let Some(built) = self.resize_key else {
            return true;
        };
        let Some(workspace) = shell.entry.workspace() else {
            return true;
        };
        let quiet = matches!(shell.core.screen(), Screen::Thread { .. })
            && self.transcript_viewport.is_some()
            && self.image_preview.is_none()
            && self.settings.is_none()
            && self.palette.is_none()
            && self.palette_dialog.is_none()
            && self.resource.is_none()
            && self.resource_dialog.is_none()
            && self.color_menu.is_none()
            && self.sidebar_drag.is_none();
        let key = ResizeKey::of(
            shell.core.workspace_screen(),
            workspace.show_sidebar,
            workspace.show_inspector,
            window.inner_size.width as f32,
        );
        !(quiet && key == built)
    }

    fn advance_glides(&mut self, shell: &mut Shell, ui: &mut UiApp) -> bool {
        self.advance_wheel_glides(shell, ui)
    }

    fn glide_active(&self) -> bool {
        self.wheel.active()
    }

    fn pre_input(&mut self, ui: &mut UiApp, input: &InputEvent) -> Option<bool> {
        let dismissal = {
            let _tm = crate::shell::application::Tm::new("pre.overlays");
            self.sidebar_overlays.request_dismissal(ui, input)
        };
        match dismissal {
            Ok(true) => return Some(true),
            Ok(false) => {}
            Err(error) => tracing::warn!(%error, "could not dismiss sidebar overlay"),
        }
        if let Some(consumed) = self
            .color_menu
            .as_mut()
            .map(|menu| menu.handle_input(ui, input))
        {
            return Some(consumed);
        }
        let cycled = {
            let _tm = crate::shell::application::Tm::new("pre.cycle");
            crate::shell::menu_keys::cycle(ui, input)
        };
        if cycled {
            return Some(true);
        }
        let _tm = crate::shell::application::Tm::new("pre.wheel");
        self.wheel.offer(ui, input).map(|_| true)
    }
    fn build(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        window: &ObservedWindowState,
    ) -> anyhow::Result<Element> {
        let theme = shell.preferences.theme();
        if self.image_preview.as_ref().is_some_and(|preview| {
            !matches!(shell.core.screen(), Screen::Thread { .. })
                || !shell.entry.workspace().is_some_and(|workspace| {
                    preview.scope.current(&workspace.conversation, &shell.core)
                })
        }) {
            self.close_image_preview(ui, false);
        }
        if self.color_scope.as_ref().is_some_and(|scope| {
            !shell
                .entry
                .workspace()
                .is_some_and(|workspace| scope.current(&workspace.conversation, &shell.core))
        }) {
            self.dispose_color_menu(ui);
        }
        let viewport = layout::Size {
            width: window.inner_size.width as f32,
            height: window.inner_size.height as f32,
        };
        if let Some(entry) =
            shell
                .entry
                .view(&shell.core, &theme, viewport, &shell.titlebar, window)
        {
            self.replace_rows(None);
            self.stored.borrow_mut().drain_cleanup(ui);
            self.composer_height = None;
            self.composer_measurement.set(None);
            self.context_tooltip.reset(ui);
            self.sidebar_overlays.dispose(ui);
            if let Some(preview) = self.sidebar_drag.take() {
                preview.dispose(ui);
            }
            self.drag_position = None;
            self.navigation.dispose(ui);
            self.transcript_viewport = None;
            self.mounted_reveals.borrow_mut().rebind(None);
            self.row_cache.borrow_mut().rebind(None);
            self.context_reading = None;
            self.observations.clear();
            drop(self.media.take());
            self.connection.reset();
            self.thread_loading.reset();
            self.thread_landing.reset();
            self.dispose_color_menu(ui);
            if let Some(dialog) = self.settings.take() {
                dialog.dispose(ui);
            }
            self.close_palette(ui);
            self.close_resource(ui);
            self.workspace_key = None;
            self.settings_panel = None;
            return Ok(entry);
        }
        let app_media = Self::app_media(shell, window);
        let workspace = shell
            .entry
            .workspace_mut()
            .ok_or_else(|| anyhow::anyhow!("account workspace is unavailable"))?;
        if self.workspace_key != Some(workspace.selections.key) {
            self.replace_rows(None);
            self.stored.borrow_mut().drain_cleanup(ui);
            self.composer_height = None;
            self.composer_measurement.set(None);
            self.context_tooltip.reset(ui);
            self.sidebar_overlays.dispose(ui);
            if let Some(preview) = self.sidebar_drag.take() {
                preview.dispose(ui);
            }
            self.drag_position = None;
            self.navigation.dispose(ui);
            self.transcript_viewport = None;
            self.mounted_reveals.borrow_mut().rebind(None);
            self.row_cache.borrow_mut().rebind(None);
            self.context_reading = None;
            self.observations.clear();
            drop(self.media.take());
            self.dispose_color_menu(ui);
            if let Some(dialog) = self.settings.take() {
                dialog.dispose(ui);
            }
            self.close_palette(ui);
            self.close_resource(ui);
            self.workspace_key = Some(workspace.selections.key);
        }
        let screen = match shell.core.workspace_screen() {
            Screen::Settings(_) => Screen::Home,
            screen => screen.clone(),
        };
        let reduced = ui.host().motion().reduced_motion() == aqua_ui::motion::ReducedMotion::Reduce;
        let size = aqua_core::Size::new(viewport.width, viewport.height);
        let now = std::time::Instant::now();
        self.resize_key = Some(ResizeKey::of(
            &screen,
            workspace.show_sidebar,
            workspace.show_inspector,
            viewport.width,
        ));
        let dimensions = layout::workspace_layout(
            viewport.width,
            &screen,
            workspace.show_sidebar,
            workspace.show_inspector,
            false,
        );
        let sidebar = workspace.sidebar_view(
            &shell.core,
            theme,
            size,
            workspace.show_sidebar,
            reduced,
            now,
            false,
        );
        aqua_core::checkpoint::reach();
        let center = match &screen {
            Screen::Home => workspace.home_view(&shell.core, &theme),
            Screen::Marketplace => workspace.marketplace_view(&shell.core, theme),
            Screen::Usage => workspace.usage_view(&shell.core, theme, size),
            Screen::Thread { .. } => {
                workspace.conversation.sync_screen(&mut shell.core);
                if !workspace.conversation.host_is_current(&shell.core) {
                    self.replace_rows(None);
                    view::centered_conversation(self.connection.waiting(theme, now, reduced))
                } else {
                self.thread_content(
                    workspace,
                    &shell.core,
                    ui,
                    theme,
                    viewport.width,
                    viewport.height,
                    reduced,
                    shell.preferences.streaming == crate::shell::preferences::StreamingMode::Swift,
                    app_media,
                    now,
                    false,
                )?
                }
            }
            Screen::NewThread { host, .. } => {
                let host = shell
                    .core
                    .host(host)
                    .ok_or_else(|| anyhow::anyhow!("new-thread Forge is unavailable"))?;
                if let Some(status) = self.connection.build(host, theme, now, reduced) {
                    view::centered_conversation(status)
                } else {
                    let (questionnaires, attachments) =
                        workspace.composer_content(ui, theme, reduced)?;
                    view::new_conversation(
                        workspace,
                        &shell.core,
                        &theme,
                        view::ComposerParts {
                            nested: false,
                            placeholder: String::new(),
                            questionnaires,
                            attachments,
                            mention_overlay: None,
                            thread_error: None,
                        },
                        Rc::clone(&self.composer_measurement),
                    )?
                }
            }
            Screen::Settings(_) => unreachable!(),
        };
        aqua_core::checkpoint::reach();
        if !matches!(screen, Screen::Thread { .. }) {
            self.replace_rows(None);
            self.stored.borrow_mut().drain_cleanup(ui);
            self.observations.clear();
            self.navigation.dispose(ui);
            self.transcript_viewport = None;
            self.mounted_reveals.borrow_mut().rebind(None);
            self.row_cache.borrow_mut().rebind(None);
            workspace.markdown.borrow_mut().rebind(ui, None);
            workspace.conversation_selection.rebind(ui, None);
            drop(self.media.take());
            self.thread_loading.reset();
            self.thread_landing.reset();
            self.smooth_scroll.cancel();
            self.scroll_requests.clear();
        }
        if !matches!(screen, Screen::NewThread { .. }) {
            self.connection.reset();
        }
        let notices = workspace
            .failure
            .as_ref()
            .map(|text| {
                let dismissed = Rc::clone(&self.failure_dismissed);
                view::failure_notice(&theme, text.clone(), move || dismissed.set(true))
            })
            .into_iter()
            .collect();
        if let Some(palette) = &workspace.palette {
            if self.palette.is_none() {
                self.palette = Some(crate::shell::palette::view::PaletteView::new(
                    self.palette_emit(),
                ));
            }
            self.palette_content = self
                .palette
                .as_ref()
                .map(|view| view.content(palette, &theme, self.palette_emit()));
        } else {
            self.close_palette(ui);
        }
        if let Some(preview) = &workspace.resource_preview {
            let generation = workspace.resource_preview_generation();
            if self.resource.as_ref().map(|(current, _)| *current) != Some(generation) {
                self.close_resource(ui);
                self.resource = Some((
                    generation,
                    crate::shell::resource_preview::view::ViewState::new(
                        generation,
                        workspace.selections.next_document(),
                        workspace.selections.next_source(),
                    ),
                ));
            }
            let emit = self.resource_emit();
            if let Some((_, state)) = &mut self.resource {
                state.prepare(preview, ui)?;
                let content = state
                    .content(preview, theme, viewport.height, emit)
                    .map_err(anyhow::Error::msg)?;
                self.resource_content = Some((content, preview.path.clone()));
            }
        } else {
            self.close_resource(ui);
        }
        let settings_open = matches!(shell.core.screen(), Screen::Settings(_));
        self.settings_panel = if settings_open {
            let settings_layout = layout::settings_layout(viewport);
            let nav = workspace.sidebar_view(
                &shell.core,
                theme,
                aqua_core::Size::new(settings_layout.size.width, settings_layout.size.height),
                true,
                reduced,
                now,
                true,
            );
            let build = crate::settings::about::BuildInfo {
                version: env!("CARGO_PKG_VERSION").to_owned(),
                commit: env!("EDITOR_COMMIT").to_owned(),
                profile: if cfg!(debug_assertions) {
                    "debug"
                } else {
                    "release"
                }
                .to_owned(),
                target: env!("EDITOR_TARGET").to_owned(),
            };
            let page = workspace.settings_view(&shell.core, theme, &build, size, true);
            let dismissed = Rc::clone(&self.settings_dismissed);
            let panel = view::settings_panel(
                theme,
                viewport,
                nav,
                page,
                Rc::new(move || dismissed.set(true)),
            );
            Some((panel, settings_layout.size.width))
        } else {
            None
        };
        aqua_core::checkpoint::reach();
        let titlebar_scope = matches!(shell.core.screen(), Screen::Thread { .. })
            .then(|| {
                crate::conversation::presentation::PresentationScope::capture(
                    &workspace.conversation,
                    &shell.core,
                )
            })
            .flatten();
        let header = titlebar_scope
            .clone()
            .and_then(|scope| {
                let (link, _) = self.conversation_callbacks(scope.clone());
                let actions = Rc::clone(&self.conversation_actions);
                shell.titlebar.thread_header(
                    &shell.core,
                    &workspace.conversation,
                    theme,
                    Rc::new(move |url| link(crate::ui::markdown::visual::LinkAction::OpenUrl(url))),
                    Rc::new(move |agent| {
                        actions
                            .borrow_mut()
                            .push_back(ConversationAction::Agent(scope.clone(), agent))
                    }),
                )
            })
            .into_iter()
            .collect();
        self.context_reading = titlebar_scope.and_then(|scope| {
            let host = shell.core.host_if_instance(&scope.host, scope.instance)?;
            let agent = scope.agent.as_ref().map(|id| {
                workspace
                    .conversation
                    .view
                    .as_ref()?
                    .item(id)
                    .and_then(|item| match &item.kind {
                        artisan_api::ItemKind::Agent {
                            context_used,
                            model,
                            ..
                        } => Some((*context_used.as_ref()?, model.clone())),
                        _ => None,
                    })
            });
            crate::shell::context_meter::reading(host, &scope.thread, agent)
        });
        let meter = self
            .context_reading
            .as_ref()
            .map(|reading| self.context_tooltip.ring(reading, theme));
        workspace
            .inspector
            .follow(&shell.core, &workspace.conversation, ui);
        let inspector = if dimensions.inspector.is_some() {
            Some(workspace.inspector_elements(
                &shell.core,
                ui,
                theme,
                (viewport.height - layout::TITLEBAR_HEIGHT).max(0.0),
                reduced,
                self.ui_now,
            )?)
        } else {
            None
        };
        aqua_core::checkpoint::reach();
        let titlebar = shell
            .titlebar
            .workspace(theme, window, header, meter, dimensions.inspector);
        let image_preview = self.image_preview.as_ref().map(|preview| {
            let events = Rc::clone(&self.preview_actions);
            preview.view.view(
                theme,
                aqua_core::Size::new(viewport.width, viewport.height),
                now,
                reduced,
                Rc::new(move |generation, action| {
                    events.borrow_mut().push_back((generation, action))
                }),
            )
        });
        Ok(view::workspace(
            workspace.selections.key,
            &theme,
            viewport,
            &screen,
            workspace.show_sidebar,
            workspace.show_inspector,
            self.context_reading.is_some(),
            view::WorkspaceElements {
                titlebar,
                sidebar,
                center,
                inspector,
                notices,
                settings_dialog: None,
                palette: None,
                resource_preview: None,
                copy_menu: None,
                image_preview,
            },
        ))
    }

    fn mounted(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) {
        self.source_dialogs
            .retain(|export| Self::source_export_is_current(shell, export));
        self.bind_row_sources(ui, ctx, window);
        (|| {
            let Some((panel, width)) = self.settings_panel.take() else {
                if let Some(dialog) = self.settings.take() {
                    dialog.dispose(ui);
                }
                return;
            };
            if let Some(dialog) = &self.settings {
                if dialog.is_open(ui) {
                    if let Err(error) = dialog.refresh_content(ui, panel) {
                        tracing::warn!(%error, "could not refresh settings dialog");
                    }
                    return;
                }
                self.settings = None;
            }
            let Some(owner) = ui.find_debug_selector("workspace-root") else {
                return;
            };
            let dismissed = Rc::clone(&self.settings_dismissed);
            let dialog = crate::ui::dialog::Dialog::new(
                "settings-dialog",
                shell.preferences.theme(),
                "Settings",
                panel,
            )
            .bare()
            .soft_backdrop()
            .max_width(width)
            .on_dismiss(move |_| dismissed.set(true));
            match crate::ui::dialog::DialogHost::open(ui, owner, dialog) {
                Ok(dialog) => self.settings = Some(dialog),
                Err(error) => tracing::warn!(%error, "could not open settings dialog"),
            }
        })();
        self.mount_palette(shell, ui);
        self.mount_resource(shell, ui);
        if self.sync_image_preview(shell, ui, ctx, window) {
            crate::shell::rebuild_stats::record_event("inv_preview");
            self.conversation_invalidated.set(true);
            ctx.request_frame(window, aqua::prelude::FrameReason::EXPLICIT);
        }
        self.sync_resource_jump(ui, ctx, window);
        {
            let _tm = crate::shell::application::Tm::new("input.views.context");
            self.sync_context(shell, ui, ctx, window);
        }
        if {
            let _tm = crate::shell::application::Tm::new("input.views.navigation");
            self.sync_navigation(ui, ctx, window, shell.preferences.theme())
        } {
            crate::shell::rebuild_stats::record_event("inv_nav_fc");
            self.navigation_invalidated.set(true);
        }
        if self.sync_sidebar(shell, ui, ctx, window) {
            crate::shell::rebuild_stats::record_event("inv_sidebar_a");
            self.conversation_invalidated.set(true);
            ctx.request_frame(window, aqua::prelude::FrameReason::EXPLICIT);
        }
        if self
            .resource
            .as_ref()
            .is_some_and(|(_, state)| state.pending_jump().is_some())
        {
            ctx.request_frame(window, aqua::prelude::FrameReason::EXPLICIT);
        }
    }

    fn frame_completed(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
        frame: u64,
    ) -> bool {
        let lap_start = std::time::Instant::now();
        let early_reveals = {
            let _tm = crate::shell::application::Tm::new("fc.reveals");
            match (&self.rows, shell.entry.workspace_mut()) {
                (Some(rows), Some(workspace)) => {
                    Some(rows.sync_reveals(ui, &mut workspace.conversation, &shell.core))
                }
                _ => None,
            }
        };
        aqua_core::checkpoint::reach();
        self.source_dialogs
            .retain(|export| Self::source_export_is_current(shell, export));
        self.bind_row_sources(ui, ctx, window);
        self.sync_resource_jump(ui, ctx, window);
        self.sync_context(shell, ui, ctx, window);
        if self.sync_navigation(ui, ctx, window, shell.preferences.theme()) {
            crate::shell::rebuild_stats::record_event("inv_nav_wake");
            self.navigation_invalidated.set(true);
        }
        if let Some(menu) = &mut self.color_menu {
            match menu.advance(ui, ctx.now().as_secs_f64()) {
                Ok(true) => ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION),
                Ok(false) => {}
                Err(error) => tracing::warn!(%error, "could not advance color copy menu"),
            }
        }
        let reduced = ui.host().motion().reduced_motion() == aqua_ui::motion::ReducedMotion::Reduce;
        let now = std::time::Instant::now();
        aqua_core::checkpoint::reach();
        self.sync_edges(shell, ui, ctx, window);
        let mut action_changed = crate::shell::rebuild_stats::note("wheel", self.sync_wheel(shell, ui, ctx, window))
            | crate::shell::rebuild_stats::note("dispatch", self.dispatch_conversation(shell, ui, ctx, window))
            | crate::shell::rebuild_stats::note("sidebar", self.sync_sidebar(shell, ui, ctx, window))
            | crate::shell::rebuild_stats::note("connection_anim", self.connection.animating(now, reduced))
            | crate::shell::rebuild_stats::note("loading_anim", self.thread_loading.animating(now, reduced))
            | crate::shell::rebuild_stats::note("landing_anim", self.thread_landing.animating(now, reduced));
        if let Some(height) = self.composer_measurement.take() {
            if self
                .composer_height
                .is_none_or(|previous| (previous - height).abs() >= 0.5)
            {
                self.composer_height = Some(height);
                action_changed = crate::shell::rebuild_stats::note("composer_height", true);
            }
        }
        let Some(rows) = &self.rows else {
            return action_changed;
        };
        let Some(workspace) = shell.entry.workspace_mut() else {
            return action_changed;
        };
        let Some(reveals) = early_reveals else {
            return action_changed;
        };
        let mut changed = crate::shell::rebuild_stats::note("reveals", reveals.frontier_changed);
        let reveal_active = reveals.changed || reveals.animating;
        let retry = self
            .pending_stall
            .observe(reveals.pending && !reveals.changed && !reveals.animating, ui.tree().invalidation_epoch());
        if changed || reveal_active || retry {
            ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION);
        }
        aqua_core::checkpoint::reach();
        let _tm_events = crate::shell::application::Tm::new("fc.events");
        let events = self.observations.drain();
        let painted_count = events.painted.len();
        #[cfg(feature = "qualification")]
        self.qualification_paint.begin_frame(&rows.scope, frame);
        #[cfg(not(feature = "qualification"))]
        let _ = frame;
        if let Some(clip) = crate::shell::wheel::keyed_node(ui, "conversation-transcript")
            .and_then(|node| ui.tree().visual_rect(node))
        {
            let active = ctx
                .window_state(window)
                .is_some_and(|window| window.focused);
            let markdown = workspace.markdown.borrow();
            let visible_count = events
                .painted
                .iter()
                .filter(|painted| {
                    painted.snapshot.active.get()
                        && painted.bounds.h > 0.0
                        && !painted.bounds.intersect(clip).is_empty()
                })
                .count();
            let tm_capture = crate::shell::application::Tm::new("fc.capture");
            let candidates: Vec<_> = events
                .painted
                .into_iter()
                .filter_map(|painted| {
                    #[cfg(feature = "qualification")]
                    let observation = painted.clone();
                    let candidate = painted.capture(
                        &workspace.conversation,
                        &shell.core,
                        &markdown,
                        &rows.scope,
                        clip,
                        active,
                    );
                    #[cfg(feature = "qualification")]
                    {
                        let painted = observation;
                        if painted.snapshot.active.get()
                            && painted.bounds.w > 0.0
                            && painted.bounds.h > 0.0
                            && !painted.bounds.intersect(clip).is_empty()
                        {
                            let item = match painted.snapshot.row.semantic() {
                                crate::conversation::fold::Row::Text { id, .. } => {
                                    Some(id.to_string())
                                }
                                _ => None,
                            };
                            self.qualification_paint.record(
                                painted.serial,
                                crate::qualification::observation::PaintedRow {
                                    key: format!("{:?}", painted.snapshot.row.key()),
                                    item,
                                    item_seq: painted.snapshot.item_seq,
                                    observed_seq: candidate
                                        .as_ref()
                                        .map(|read| read.observed_seq()),
                                    bounds_bits: crate::qualification::observation::rect_bits(
                                        painted.bounds,
                                    ),
                                    clip_bits: crate::qualification::observation::rect_bits(clip),
                                    revealed: painted.snapshot.revealed,
                                },
                            );
                        }
                    }
                    candidate
                })
                .collect();
            crate::conversation::open_timing::gate(
                reveals.reason,
                reveals.pending,
                reveals.changed,
                reveals.animating,
                active,
                painted_count,
                visible_count,
                candidates.len(),
                lap_start.elapsed(),
            );
            drop(tm_capture);
            let tm_ack = crate::shell::application::Tm::new("fc.ack");
            for candidate in candidates {
                candidate.acknowledge(&workspace.conversation, &mut shell.core, &markdown);
            }
            drop(tm_ack);
            let _tm_viewports = crate::shell::application::Tm::new("fc.viewports");
            let landing_before = workspace.conversation.landing.is_some();
            for event in events.viewports {
                if event.active.get() {
                    let rebuild = rail_refresh::Rail {
                        navigation: &mut self.navigation,
                        marker: &mut self.navigation_marker,
                        transcript_viewport: self.transcript_viewport,
                        composer_height: self.composer_height,
                    }
                    .refresh(
                        ui,
                        ctx,
                        window,
                        &workspace.conversation,
                        &shell.core,
                        event.viewport,
                        reduced,
                    );
                    changed |= crate::shell::rebuild_stats::note("viewport", rebuild);
                    self.transcript_viewport = Some(event.viewport);
                    let was_following = workspace
                        .conversation
                        .view
                        .as_ref()
                        .is_some_and(|view| view.viewport.state().is_following());
                    let previous = workspace.conversation.rows_epoch();
                    match self.observations.observe_viewport(
                        &mut workspace.conversation,
                        &shell.core,
                        ui,
                        &markdown,
                        &rows.scope,
                        event.viewport,
                        clip,
                    ) {
                        Ok(()) => {
                            changed |= crate::shell::rebuild_stats::note(
                                "rows",
                                previous != workspace.conversation.rows_epoch(),
                            );
                            changed |= was_following
                                != workspace
                                    .conversation
                                    .view
                                    .as_ref()
                                    .is_some_and(|view| view.viewport.state().is_following());
                        }
                        Err(error) => {
                            tracing::warn!(%error, "could not observe transcript viewport")
                        }
                    }
                }
            }
            if landing_before && workspace.conversation.landing.is_none() {
                changed |= crate::shell::rebuild_stats::note("landed", true);
            }
        }
        drop(_tm_events);
        aqua_core::checkpoint::reach();
        let _tm_after = crate::shell::application::Tm::new("fc.after");
        crate::conversation::view::transcript::apply_heights(
            &mut workspace.conversation,
            events.heights,
        );
        changed |= crate::shell::rebuild_stats::note(
            "projections",
            rows.publish_projections(&mut workspace.conversation, &shell.core),
        );
        let after_layout = rows.after_layout(ui);
        let motion_due = after_layout.rebuild
            && self
                .motion_rebuilt
                .get()
                .is_none_or(|at| at.elapsed() >= MOTION_REBUILD_INTERVAL);
        if motion_due {
            self.motion_rebuilt.set(Some(std::time::Instant::now()));
        }
        if after_layout.repaint || after_layout.rebuild {
            ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION);
        }
        changed |= crate::shell::rebuild_stats::note("after_layout", motion_due);
        let disclosure = workspace.conversation.advance_disclosure_frame(
            &shell.core,
            std::time::Instant::now(),
            reduced,
        );
        changed |= crate::shell::rebuild_stats::note(
            "disclosure",
            !disclosure.changed.is_empty() || disclosure.refold || disclosure.active,
        );
        if disclosure.active {
            ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION);
        }
        self.sync_scroll(&mut workspace.conversation, ui, ctx, window);
        changed | action_changed
    }

    fn input(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
        input: &InputEvent,
        consumed: bool,
    ) -> bool {
        self.sync_context(shell, ui, ctx, window);
        if self.sync_navigation(ui, ctx, window, shell.preferences.theme()) {
            crate::shell::rebuild_stats::record_event("inv_nav_input");
            self.navigation_invalidated.set(true);
        }
        if let InputEvent::Scroll { at, delta } = input {
            if (delta.x != 0.0 || delta.y != 0.0)
                && crate::shell::wheel::keyed_node(ui, "conversation-transcript")
                    .and_then(|node| ui.tree().visual_rect(node))
                    .is_some_and(|clip| clip.contains(*at))
            {
                self.observations.note_reader_move();
                if delta.y != 0.0 {
                    self.observations.note_reader_direction(delta.y > 0.0);
                }
                self.smooth_scroll.cancel();
                if delta.y < 0.0 {
                    crate::conversation::scroll::pause_tail(ui);
                }
            }
        }
        if let Some(dialog) = &self.palette_dialog {
            dialog.sync(ui);
        }
        if let Some(dialog) = &self.resource_dialog {
            dialog.sync(ui);
        }
        let wheel_changed = {
            let _tm = crate::shell::application::Tm::new("input.views.wheel");
            self.sync_wheel(shell, ui, ctx, window)
        };
        let palette_changed = crate::shell::rebuild_stats::note("in_wheel", wheel_changed)
            | {
                let _tm = crate::shell::application::Tm::new("input.views.palette");
                crate::shell::rebuild_stats::note(
                    "in_palette",
                    self.dispatch_palette(shell, ui) | self.dispatch_resource(shell, ui, ctx),
                )
            }
            | {
                let _tm = crate::shell::application::Tm::new("input.views.conversation");
                crate::shell::rebuild_stats::note(
                    "in_conversation",
                    self.dispatch_conversation(shell, ui, ctx, window),
                )
            }
            | {
                let _tm = crate::shell::application::Tm::new("input.views.sidebar");
                crate::shell::rebuild_stats::note(
                    "in_sidebar",
                    self.sync_sidebar(shell, ui, ctx, window),
                )
            };
        let failure_changed = if self.failure_dismissed.replace(false) {
            shell
                .entry
                .workspace_mut()
                .is_some_and(|workspace| workspace.failure.take().is_some())
        } else {
            false
        };
        let palette_changed = palette_changed | failure_changed;
        if let Some(dialog) = &self.settings {
            dialog.sync(ui);
        }
        if self.settings_dismissed.replace(false) {
            shell.core.close_settings();
            return true;
        }
        if consumed || self.image_preview.is_some() {
            return palette_changed;
        }
        let InputEvent::KeyDown(event) = input else {
            return palette_changed;
        };
        let modifiers = event.modifiers;
        let command = modifiers.contains(aqua_core::Modifiers::CONTROL)
            || modifiers.contains(aqua_core::Modifiers::LOGO);
        if command
            && !modifiers.contains(aqua_core::Modifiers::ALT)
            && !modifiers.contains(aqua_core::Modifiers::SHIFT)
            && matches!(event.key, aqua_core::Key::Char('q' | 'Q'))
        {
            ctx.quit();
            return false;
        }
        if shell.entry.workspace().is_none() {
            return palette_changed;
        }
        if modifiers == aqua_core::Modifiers::ALT
            && event.key == aqua_core::Key::Named(aqua_core::NamedKey::Left)
        {
            shell.go_back();
            return true;
        }
        if !command || modifiers.contains(aqua_core::Modifiers::SHIFT) {
            return palette_changed;
        }
        let alt = modifiers.contains(aqua_core::Modifiers::ALT);
        match event.key {
            aqua_core::Key::Char('b' | 'B') if alt => {
                if let Some(workspace) = shell.entry.workspace_mut() {
                    workspace.show_inspector = !workspace.show_inspector;
                    return true;
                }
            }
            aqua_core::Key::Char('b' | 'B') if !alt => {
                if let Some(workspace) = shell.entry.workspace_mut() {
                    workspace.show_sidebar = !workspace.show_sidebar;
                    return true;
                }
            }
            aqua_core::Key::Char(',') if !alt => {
                shell.open_settings();
                return true;
            }
            aqua_core::Key::Char('n' | 'N') if !alt => {
                shell.new_thread();
                return true;
            }
            aqua_core::Key::Char('k' | 'K') if !alt => {
                shell.open_palette(ctx, window);
                return true;
            }
            _ => {}
        }
        palette_changed
    }

    fn wake(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) -> bool {
        self.sync_context(shell, ui, ctx, window);
        if self.sync_navigation(ui, ctx, window, shell.preferences.theme()) {
            crate::shell::rebuild_stats::record_event("inv_nav_a");
            self.navigation_invalidated.set(true);
        }
        let palette_changed = self.dispatch_palette(shell, ui)
            | self.dispatch_resource(shell, ui, ctx)
            | self.dispatch_conversation(shell, ui, ctx, window)
            | self.sync_sidebar(shell, ui, ctx, window);
        if self.settings_dismissed.replace(false) {
            shell.core.close_settings();
            true
        } else {
            palette_changed
        }
    }
}

#[cfg(all(test, feature = "test-support"))]
#[path = "../../tests/shell/preview.rs"]
mod tests;

#[cfg(all(test, feature = "test-support"))]
#[path = "../../tests/shell/blob_images.rs"]
mod blob_tests;
