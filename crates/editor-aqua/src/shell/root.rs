use self::open_thread::PendingStall;
use crate::core::Screen;
use crate::shell::refresh::Invalidation;
use crate::shell::{
    application::{BuildError, SectionViews},
    host::Shell,
    layout,
};
use aqua::prelude::{AquaWindowId, InputEvent, ObservedWindowState};
use aqua::{AppCtx, UiApp};
use aqua_ui::element::Element;
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, VecDeque},
    rc::Rc,
};

mod account;
mod animation;
mod build;
mod dispatch;
mod frame;
mod image_preview;
mod input;
mod menus;
mod navigation;
mod open_thread;
mod palette;
#[cfg(feature = "qualification")]
mod qualification;
mod rail_refresh;
mod read_watch;
mod resource;
pub mod reveal;
mod sidebar;
mod sources;
mod thread_content;
mod transcript;
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
    Copy(
        crate::conversation::presentation::PresentationScope,
        crate::conversation::view::source_actions::CopySource,
    ),
    RetryControls(crate::conversation::presentation::PresentationScope),
    RetryThread(crate::conversation::presentation::PresentationScope),
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
        self.fetcher.close_resources();
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

type BrowserLookup = std::sync::mpsc::Receiver<(
    Option<crate::shell::browser::Browser>,
    Option<aqua::prelude::FileIcon>,
)>;

struct ResolvedBrowser {
    preferred: Option<String>,
    browser: Option<crate::shell::browser::Browser>,
    icon: Option<aqua::images::ImageHandle>,
}

enum PreviewOrigin {
    Conversation(crate::conversation::presentation::PresentationScope),
    Composer,
}

struct ImagePreview {
    origin: PreviewOrigin,
    owner: Option<std::sync::Arc<[u64]>>,
    view: crate::ui::media::preview::Preview,
    focus_pending: bool,
}

const MOTION_REBUILD_INTERVAL: std::time::Duration = std::time::Duration::from_millis(33);

#[derive(Default)]
pub struct RootViews {
    noted: Invalidation,
    motion_rebuilt: Cell<Option<std::time::Instant>>,
    #[cfg(feature = "qualification")]
    qualification_paint: crate::qualification::observation::PaintCollector,
    settings: Option<crate::ui::dialog::DialogHost>,
    link_failure: crate::shell::link_failure::LinkFailureDialog,
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
    sidebar: crate::frame::sidebar::Sidebar,
    seam: Rc<RefCell<crate::frame::seam::motion::Motion>>,
    avatar: crate::frame::avatar::Avatar,
    sign_in_dirty: bool,
    work_clock: Option<std::time::Instant>,
    account_popup: crate::frame::popup::Popup,
    sidebar_overlays: crate::sidebar::overlay::SidebarOverlays,
    sidebar_drag: Option<crate::sidebar::drag_preview::DragPreviewHost>,
    drag_position: Option<aqua_core::Point>,
    navigation: crate::shell::thread_navigation::ThreadNavigation,
    composer_glide: crate::shell::composer_glide::ComposerGlide,
    latest: crate::shell::thread_navigation::LatestPresence,
    transcript_viewport: Option<aqua_ui::collection_runtime::CollectionViewport>,
    navigation_marker: Option<usize>,
    context_reading: Option<crate::shell::context_meter::ContextReading>,
    connection: crate::shell::connection::ConnectionPresenter,
    thread_loading: crate::shell::connection::WordmarkLoader,
    thread_landing: crate::shell::connection::WordmarkLoader,
    smooth_scroll: crate::conversation::scroll::SmoothScroll,
    wheel: crate::shell::wheel::WheelGlides,
    pending_stall: PendingStall,
    read_watch: Option<read_watch::ReadWatch>,
    transcript_edges: crate::ui::scroll_edge::ScrollEdges,
    scroll_request: Option<crate::conversation::controller::ScrollAction>,
    composer_measurement: Rc<Cell<Option<f32>>>,
    composer_height: Option<f32>,
    conversation_actions: Rc<RefCell<VecDeque<ConversationAction>>>,
    color_menu: Option<crate::ui::color_copy_menu::ColorCopyMenuHost>,
    selection_menu: Option<(aqua_core::Point, String)>,
    browser: Option<ResolvedBrowser>,
    browser_task: Option<(Option<String>, BrowserLookup)>,
    reveal: Option<reveal::Reveal>,
    color_scope: Option<crate::conversation::presentation::PresentationScope>,
    color_effects: Rc<RefCell<VecDeque<crate::ui::color_copy_menu::ColorCopyEffect>>>,
    resize_key: Option<ResizeKey>,
    transcript_landing: Option<bool>,
}

impl std::fmt::Debug for RootViews {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("RootViews").finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ResizeKey {
    overview: bool,
    size: [u32; 2],
}

impl ResizeKey {
    fn of(screen: &Screen, card_width: f32, window: layout::Size) -> Self {
        Self {
            overview: overview_shown(screen, card_width),
            size: [window.width.to_bits(), window.height.to_bits()],
        }
    }
}

fn overview_shown(screen: &Screen, card_width: f32) -> bool {
    matches!(screen, Screen::Thread { .. }) && crate::frame::overview::fits(card_width)
}

impl RootViews {}

impl RootViews {}

impl RootViews {}

impl SectionViews for RootViews {
    fn refresh_transcript(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        window: &ObservedWindowState,
    ) -> bool {
        if !self.transcript_refreshable(shell, ui) {
            crate::core::diagnostics::rebuild_stats::record_event("tf_not_refreshable");
            return false;
        }
        let theme = shell.preferences.theme();
        let swift = shell.preferences.streaming == crate::ui::preferences::StreamingMode::Swift;
        let reduced = ui.host().motion().reduced_motion() == aqua_ui::motion::ReducedMotion::Reduce;
        let app_media = Self::app_media(shell, window);
        let Some(workspace) = shell.entry.workspace_mut() else {
            return false;
        };
        let width = (crate::measure::narrow(window.inner_size.width)
            - self.sidebar.width(std::time::Instant::now())
            - crate::frame::view::SHEET_PADDING)
            .max(0.0);
        let element = match self.thread_content(
            workspace,
            &shell.core,
            ui,
            theme,
            width,
            reduced,
            swift,
            app_media,
            shell.core.clock().now(),
            true,
        ) {
            Ok(element) => element,
            Err(error) => {
                tracing::warn!(%error, "could not build transcript");
                return false;
            }
        };
        let announcer = crate::conversation::announce::announcer(
            &crate::conversation::announce::current(&workspace.conversation),
        );
        if let Err(error) =
            ui.refresh_subtree(crate::conversation::announce::ANNOUNCER_KEY, announcer)
        {
            tracing::warn!(%error, "could not refresh the reply announcement");
        }
        match ui.refresh_subtree("conversation-transcript", element) {
            Ok(refreshed) => {
                if !refreshed {
                    crate::core::diagnostics::rebuild_stats::record_event("tf_subtree_false");
                }
                refreshed
            }
            Err(error) => {
                tracing::warn!(%error, "could not refresh transcript");
                false
            }
        }
    }

    fn refresh_sidebar_frame(
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
        let theme = shell.preferences.theme();
        let now = std::time::Instant::now();
        let reduced = Self::reduced(ui);
        let height = crate::shell::wheel::viewport_height(ui, crate::sidebar::view::ROOT_KEY)
            .unwrap_or((crate::measure::narrow(window.inner_size.height) - 260.0).max(0.0));
        let tree = workspace
            .sidebar
            .host
            .as_ref()
            .and_then(|host| shell.core.host(host))
            .map(|_| {
                crate::frame::inbox::view(
                    &workspace.sidebar,
                    &shell.core,
                    theme,
                    height,
                    reduced,
                    self.tree_emit(workspace),
                )
            });
        let (search, tree, overlay) = self.search_parts(workspace, &theme, tree, now);
        let account = self.account_snapshot(shell);
        let element = crate::frame::view::sidebar_view(
            &theme,
            self.sidebar_model(&theme, search, tree, account.as_ref(), now),
        );

        [
            (crate::frame::view::SIDEBAR, element),
            (crate::frame::search::OVERLAY, overlay),
        ]
        .into_iter()
        .all(|(key, element)| match ui.refresh_subtree(key, element) {
            Ok(refreshed) => refreshed,
            Err(error) => {
                tracing::warn!(%error, key, "could not refresh sidebar frame");
                false
            }
        })
    }

    fn refresh_sidebar(&self, shell: &Shell, ui: &mut UiApp, window: &ObservedWindowState) -> bool {
        let Some(workspace) = shell.entry.workspace() else {
            return false;
        };
        if matches!(shell.core.screen(), Screen::Settings(_)) {
            return false;
        }
        let reduced = ui.host().motion().reduced_motion() == aqua_ui::motion::ReducedMotion::Reduce;
        let height = crate::shell::wheel::viewport_height(ui, crate::sidebar::view::ROOT_KEY)
            .unwrap_or((crate::measure::narrow(window.inner_size.height) - 260.0).max(0.0));
        let element = crate::frame::inbox::view(
            &workspace.sidebar,
            &shell.core,
            shell.preferences.theme(),
            height,
            reduced,
            self.tree_emit(workspace),
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
        if shell.entry.workspace().is_none() {
            return true;
        }
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
        let viewport = layout::Size {
            width: crate::measure::narrow(window.inner_size.width),
            height: crate::measure::narrow(window.inner_size.height),
        };
        let card_width = (viewport.width
            - self.sidebar.width(shell.core.clock().now())
            - crate::frame::view::SHEET_PADDING)
            .max(0.0);
        let key = ResizeKey::of(shell.core.workspace_screen(), card_width, viewport);
        !(quiet && key == built)
    }

    fn advance_glides(&mut self, shell: &mut Shell, ui: &mut UiApp) -> bool {
        self.advance_wheel_glides(shell, ui)
    }

    fn glide_active(&self) -> bool {
        self.wheel.active()
    }

    fn open_composer_images(&mut self, shell: &mut Shell, ui: &mut UiApp) -> bool {
        self.open_composer_image(shell, ui)
    }

    fn take_events(
        &mut self,
        shell: &mut Shell,
        ui: &UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) -> bool {
        let changed = self
            .sidebar
            .take_events(std::time::Instant::now(), Self::reduced(ui));
        self.apply_account_effects(shell, ctx, window, Self::reduced(ui));
        let pictured = self.avatar.drain(shell.images(), std::time::Instant::now());
        self.schedule_sidebar(ctx, window);
        changed | pictured
    }

    fn take_invalidation(&mut self) -> Invalidation {
        std::mem::take(&mut self.noted)
    }

    fn animate(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) -> bool {
        self.step_animation(shell, ui, ctx, window)
    }

    fn pre_input(&mut self, ui: &mut UiApp, input: &InputEvent) -> Option<bool> {
        self.intercept_input(ui, input)
    }
    fn build(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        window: &ObservedWindowState,
    ) -> Result<Element, BuildError> {
        self.build_root(shell, ui, window)
    }

    fn mounted(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) {
        self.after_mount(shell, ui, ctx, window);
    }

    fn frame_completed(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
        frame: u64,
    ) -> bool {
        self.complete_frame(shell, ui, ctx, window, frame)
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
        self.route_input(shell, ui, ctx, window, input, consumed)
    }

    fn wake(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) -> bool {
        self.wake_views(shell, ui, ctx, window)
    }
}

#[cfg(all(test, feature = "test-support"))]
#[path = "../../tests/shell/preview.rs"]
mod tests;

#[cfg(all(test, feature = "test-support"))]
#[path = "../../tests/shell/blob_images.rs"]
mod blob_tests;
