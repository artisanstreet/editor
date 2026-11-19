#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Refresh {
    #[default]
    None,
    Sidebar,
    SidebarFrame,
    Transcript,
    Sections,
    FrameSections,
    Workspace,
}

impl Refresh {
    pub const fn merge(self, other: Self) -> Self {
        match (self, other) {
            (Self::Workspace, _) | (_, Self::Workspace) => Self::Workspace,
            (Self::None, next) | (next, Self::None) => next,
            (Self::Sidebar, Self::Sidebar) => Self::Sidebar,
            (Self::SidebarFrame, Self::Sidebar | Self::SidebarFrame)
            | (Self::Sidebar, Self::SidebarFrame) => Self::SidebarFrame,
            (Self::FrameSections, _) | (_, Self::FrameSections) => Self::FrameSections,
            (Self::SidebarFrame, _) | (_, Self::SidebarFrame) => Self::FrameSections,
            (Self::Transcript, Self::Transcript) => Self::Transcript,
            _ => Self::Sections,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cause {
    Sections,
    Titlebar,
    ViewEvents,
    CodeActions,
    QuestionSource,
    Inspector,
    MediaAssets,
    EntryActions,
    WorkspaceActions,
    LiveText,
    ThreadItems,
    ListedThreads,
    OpenThread,
    HostState,
    EntryState,
    ConversationState,
    ConversationImages,
    ComposerState,
    QuestionPages,
    SidebarState,
    ScreenState,
    MarkdownMedia,
    PaletteState,
    ReferenceState,
    ComposerImage,
    Unmounted,
    ViewsFrame,
    PopupAnimating,
    UsageClock,
    CodeFrame,
    InspectorTick,
    SidebarMotion,
    SidebarTick,
    LandingExpiry,
    RevealActed,
    SidebarEvents,
    WorkClock,
    SidebarAnimation,
    SidebarResize,
    SidebarSettled,
    Signing,
    AfterLayout,
    LayoutMotion,
    ComposerHeight,
    ConnectionAnimation,
    ImagePreview,
    ConversationInvalidated,
    RowSources,
    StoredRows,
    Disclosure,
    Dispatch,
    ConversationInput,
    PaletteInput,
    SidebarInput,
    WheelInput,
    Landed,
    LandingAnimation,
    LoadingAnimation,
    Navigation,
    Projections,
    Reveals,
    Rows,
    Sidebar,
    Viewport,
    Wheel,
}

impl Cause {
    #[must_use]
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Sections => "sections",
            Self::Titlebar => "titlebar",
            Self::ViewEvents => "view_events",
            Self::CodeActions => "code_actions",
            Self::QuestionSource => "question_source",
            Self::Inspector => "inspector",
            Self::MediaAssets => "media_assets",
            Self::EntryActions => "entry_actions",
            Self::WorkspaceActions => "workspace_actions",
            Self::LiveText => "live_text",
            Self::ThreadItems => "thread_items",
            Self::ListedThreads => "listed_threads",
            Self::OpenThread => "open_thread",
            Self::HostState => "host_state",
            Self::EntryState => "entry_state",
            Self::ConversationState => "conversation_state",
            Self::ConversationImages => "conversation_images",
            Self::ComposerState => "composer_state",
            Self::QuestionPages => "question_pages",
            Self::SidebarState => "sidebar_state",
            Self::ScreenState => "screen_state",
            Self::MarkdownMedia => "markdown_media",
            Self::PaletteState => "palette_state",
            Self::ReferenceState => "reference_state",
            Self::ComposerImage => "composer_image",
            Self::Unmounted => "unmounted",
            Self::ViewsFrame => "views_frame",
            Self::PopupAnimating => "popup_animating",
            Self::UsageClock => "usage_clock",
            Self::CodeFrame => "code_frame",
            Self::InspectorTick => "inspector_tick",
            Self::SidebarMotion => "sidebar_motion",
            Self::SidebarTick => "sidebar_tick",
            Self::LandingExpiry => "landing_expiry",
            Self::RevealActed => "anim_reveal",
            Self::SidebarEvents => "anim_events",
            Self::WorkClock => "anim_clock",
            Self::SidebarAnimation => "anim_sidebar",
            Self::SidebarResize => "anim_sidebar_resize",
            Self::SidebarSettled => "anim_sidebar_settled",
            Self::Signing => "anim_signing",
            Self::AfterLayout => "after_layout",
            Self::LayoutMotion => "layout_motion",
            Self::ComposerHeight => "composer_height",
            Self::ConnectionAnimation => "connection_anim",
            Self::ImagePreview => "conv_image_preview",
            Self::ConversationInvalidated => "conv_invalidated",
            Self::RowSources => "conv_row_sources",
            Self::StoredRows => "conv_stored",
            Self::Disclosure => "disclosure",
            Self::Dispatch => "dispatch",
            Self::ConversationInput => "in_conversation",
            Self::PaletteInput => "in_palette",
            Self::SidebarInput => "in_sidebar",
            Self::WheelInput => "in_wheel",
            Self::Landed => "landed",
            Self::LandingAnimation => "landing_anim",
            Self::LoadingAnimation => "loading_anim",
            Self::Navigation => "navigation",
            Self::Projections => "projections",
            Self::Reveals => "reveals",
            Self::Rows => "rows",
            Self::Sidebar => "sidebar",
            Self::Viewport => "viewport",
            Self::Wheel => "wheel",
        }
    }

    #[must_use]
    pub const fn refresh(self) -> Refresh {
        match self {
            Self::Sections
            | Self::Titlebar
            | Self::ViewEvents
            | Self::CodeActions
            | Self::QuestionSource
            | Self::Inspector
            | Self::MediaAssets
            | Self::EntryActions
            | Self::WorkspaceActions
            | Self::OpenThread
            | Self::HostState
            | Self::EntryState
            | Self::ConversationImages
            | Self::ComposerState
            | Self::QuestionPages
            | Self::ScreenState
            | Self::MarkdownMedia
            | Self::PaletteState
            | Self::ReferenceState
            | Self::ComposerImage
            | Self::Unmounted
            | Self::PopupAnimating
            | Self::UsageClock
            | Self::CodeFrame
            | Self::InspectorTick
            | Self::RevealActed
            | Self::SidebarEvents
            | Self::SidebarSettled
            | Self::Signing
            | Self::ComposerHeight
            | Self::ConnectionAnimation
            | Self::ImagePreview
            | Self::ConversationInvalidated
            | Self::RowSources
            | Self::StoredRows
            | Self::Dispatch
            | Self::ConversationInput
            | Self::PaletteInput
            | Self::SidebarInput
            | Self::WheelInput
            | Self::LandingAnimation
            | Self::LoadingAnimation
            | Self::Navigation
            | Self::Projections
            | Self::Sidebar
            | Self::Wheel => Refresh::Workspace,
            Self::ViewsFrame => Refresh::None,
            Self::SidebarMotion | Self::SidebarTick | Self::SidebarState => Refresh::Sidebar,
            Self::ListedThreads => Refresh::Sections,
            Self::LandingExpiry
            | Self::WorkClock
            | Self::AfterLayout
            | Self::LayoutMotion
            | Self::Disclosure
            | Self::Landed
            | Self::Reveals
            | Self::Rows
            | Self::LiveText
            | Self::ThreadItems
            | Self::ConversationState
            | Self::Viewport => Refresh::Transcript,
            Self::SidebarAnimation | Self::SidebarResize => Refresh::SidebarFrame,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Invalidation {
    causes: Vec<Cause>,
}

impl Invalidation {
    pub fn note(&mut self, cause: Cause, changed: bool) -> bool {
        if changed && !self.causes.contains(&cause) {
            self.causes.push(cause);
        }
        changed
    }

    pub(crate) fn extend(&mut self, other: Self) {
        for cause in other.causes {
            self.note(cause, true);
        }
    }

    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.causes.is_empty()
    }

    #[must_use]
    pub fn refresh(&self) -> Refresh {
        match self.causes.iter().fold(Refresh::None, |refresh, cause| {
            refresh.merge(cause.refresh())
        }) {
            Refresh::None => Refresh::Workspace,
            refresh @ Refresh::Sidebar
            | refresh @ Refresh::SidebarFrame
            | refresh @ Refresh::Transcript
            | refresh @ Refresh::Sections
            | refresh @ Refresh::FrameSections
            | refresh @ Refresh::Workspace => refresh,
        }
    }

    #[must_use]
    pub(crate) fn labels(&self) -> Vec<&'static str> {
        self.causes.iter().map(|cause| cause.label()).collect()
    }
}
