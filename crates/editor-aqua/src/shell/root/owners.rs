use super::{
    BrowserLookup, ConversationAction, ConversationMedia, ImagePreview, ResolvedBrowser, reveal,
};
use aqua::UiApp;
use aqua_ui::element::Element;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

#[derive(Default)]
pub(super) struct DialogHosts {
    pub(super) settings: Option<crate::ui::dialog::DialogHost>,
    pub(super) settings_panel: Option<(Element, f32)>,
    pub(super) settings_dismissed: Rc<Cell<bool>>,
    pub(super) palette: Option<crate::shell::palette::view::PaletteView>,
    pub(super) palette_dialog: Option<crate::ui::dialog::DialogHost>,
    pub(super) palette_content: Option<Element>,
    pub(super) palette_events: Rc<RefCell<VecDeque<crate::shell::palette::view::Event>>>,
    pub(super) resource: Option<(u64, crate::shell::resource_preview::view::ViewState)>,
    pub(super) resource_dialog: Option<crate::ui::dialog::DialogHost>,
    pub(super) resource_content: Option<(Element, String)>,
    pub(super) resource_events: Rc<RefCell<VecDeque<crate::shell::resource_preview::view::Action>>>,
    pub(super) source_dialogs: crate::shell::source::RowSourceDialogs,
}

#[derive(Default)]
pub(super) struct TranscriptViews {
    pub(super) rows: Option<Rc<crate::conversation::view::factory::RowFactory>>,
    pub(super) row_actions: Rc<RefCell<VecDeque<crate::conversation::adapter::ScopedAction>>>,
    pub(super) observations: crate::conversation::view::transcript::TranscriptObservations,
    pub(super) mounted_reveals: Rc<RefCell<crate::conversation::view::reveal::MountedReveals>>,
    pub(super) row_cache: Rc<RefCell<crate::conversation::view::row_cache::RowCache>>,
    pub(super) stored: Rc<RefCell<crate::conversation::stored::StoredConversation>>,
    pub(super) row_motion: Rc<RefCell<crate::conversation::view::motion::RowMotion>>,
    pub(super) markdown_engine: Rc<RefCell<Option<crate::ui::markdown::MarkdownEngine>>>,
    pub(super) conversation_invalidated: Rc<Cell<bool>>,
    pub(super) row_token: Option<Rc<Cell<bool>>>,
    pub(super) row_settled: Rc<RefCell<HashMap<crate::conversation::fold::RowKey, u64>>>,
    pub(super) row_style: Option<(crate::ui::theme::ArtisanTheme, bool, bool, u32)>,
    pub(super) row_style_epoch: u64,
    pub(super) motion_epoch: Option<std::time::Instant>,
    pub(super) conversation_actions: Rc<RefCell<VecDeque<ConversationAction>>>,
}

#[derive(Default)]
pub(super) struct MediaViews {
    pub(super) media: Option<ConversationMedia>,
    pub(super) image_preview: Option<ImagePreview>,
    pub(super) preview_generation: u64,
    pub(super) preview_actions: Rc<RefCell<VecDeque<(u64, crate::ui::media::preview::Action)>>>,
}

#[derive(Default)]
pub(super) struct SidebarViews {
    pub(super) sidebar: crate::frame::sidebar::Sidebar,
    pub(super) seam: Rc<RefCell<crate::frame::seam::motion::Motion>>,
    pub(super) avatar: crate::frame::avatar::Avatar,
    pub(super) account_popup: crate::frame::popup::Popup,
    pub(super) sidebar_overlays: crate::sidebar::overlay::SidebarOverlays,
    pub(super) sidebar_drag: Option<crate::sidebar::drag_preview::DragPreviewHost>,
    pub(super) drag_position: Option<aqua_core::Point>,
}

#[derive(Default)]
pub(super) struct ScrollState {
    pub(super) smooth_scroll: crate::conversation::scroll::SmoothScroll,
    pub(super) wheel: crate::shell::wheel::WheelGlides,
    pub(super) transcript_edges: crate::ui::scroll_edge::ScrollEdges,
    pub(super) overview_edges: crate::ui::scroll_edge::ScrollEdges,
    pub(super) transcript_viewport: Option<aqua_ui::collection_runtime::CollectionViewport>,
    pub(super) scroll_request: Option<crate::conversation::controller::ScrollAction>,
}

#[derive(Default)]
pub(super) struct Menus {
    pub(super) color_menu: Option<crate::ui::color_copy_menu::ColorCopyMenuHost>,
    pub(super) selection_menu: Option<(aqua_core::Point, String)>,
    pub(super) color_scope: Option<crate::conversation::presentation::PresentationScope>,
    pub(super) color_effects: Rc<RefCell<VecDeque<crate::ui::color_copy_menu::ColorCopyEffect>>>,
}

#[derive(Default)]
pub(super) struct BrowserResolver {
    pub(super) browser: Option<ResolvedBrowser>,
    pub(super) browser_task: Option<(Option<String>, BrowserLookup)>,
}

#[derive(Default)]
pub(super) struct NavigationViews {
    pub(super) navigation: crate::shell::thread_navigation::ThreadNavigation,
    pub(super) composer_glide: crate::shell::composer_glide::ComposerGlide,
    pub(super) latest: crate::shell::thread_navigation::LatestPresence,
    pub(super) navigation_marker: Option<usize>,
    pub(super) navigation_invalidated: Rc<Cell<bool>>,
    pub(super) reveal: Option<reveal::Reveal>,
    pub(super) transcript_landing: Option<bool>,
}

impl DialogHosts {
    pub(super) fn close_palette(&mut self, ui: &mut UiApp) {
        if let Some(dialog) = self.palette_dialog.take() {
            dialog.dispose(ui);
        }
        if let Some(palette) = self.palette.take() {
            palette.close();
        }
        self.palette_content = None;
        self.palette_events.borrow_mut().clear();
    }

    pub(super) fn close_resource(&mut self, ui: &mut UiApp) {
        if let Some(dialog) = self.resource_dialog.take() {
            dialog.dispose(ui);
        }
        if let Some((_, mut resource)) = self.resource.take() {
            resource.release(ui);
        }
        self.resource_content = None;
        self.resource_events.borrow_mut().clear();
    }

    pub(super) fn dispose(&mut self, ui: &mut UiApp) {
        if let Some(dialog) = self.settings.take() {
            dialog.dispose(ui);
        }
        self.close_palette(ui);
        self.close_resource(ui);
    }
}

impl TranscriptViews {
    pub(super) fn replace_rows(
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

    pub(super) fn unbind(&self) {
        self.mounted_reveals.borrow_mut().rebind(None);
        self.row_cache.borrow_mut().rebind(None);
    }

    pub(super) fn dispose(&mut self, ui: &mut UiApp) {
        self.replace_rows(None);
        self.stored.borrow_mut().drain_cleanup(ui);
        self.observations.clear();
    }
}

impl MediaViews {
    pub(super) fn dispose(&mut self) {
        drop(self.media.take());
    }
}

impl SidebarViews {
    pub(super) fn dispose(&mut self, ui: &mut UiApp) {
        self.sidebar_overlays.dispose(ui);
        if let Some(preview) = self.sidebar_drag.take() {
            preview.dispose(ui);
        }
        self.drag_position = None;
    }
}

impl Menus {
    pub(super) fn dispose(&mut self, ui: &mut UiApp) {
        if let Some(menu) = self.color_menu.take() {
            menu.dispose(ui);
        }
        self.color_effects.borrow_mut().clear();
        self.color_scope = None;
    }
}
