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
    ShellUpdates,
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
    pub const fn label(self) -> &'static str {
        match self {
            Self::Sections => "sections",
            Self::Titlebar => "titlebar",
            Self::ViewEvents => "view_events",
            Self::CodeActions => "code_actions",
            Self::QuestionSource => "question_source",
            Self::Inspector => "inspector",
            Self::ShellUpdates => "shell_updates",
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
            | Self::ShellUpdates
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
            Self::SidebarMotion | Self::SidebarTick => Refresh::Sidebar,
            Self::LandingExpiry
            | Self::WorkClock
            | Self::AfterLayout
            | Self::LayoutMotion
            | Self::Disclosure
            | Self::Landed
            | Self::Reveals
            | Self::Rows
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

    pub fn extend(&mut self, other: Self) {
        for cause in other.causes {
            self.note(cause, true);
        }
    }

    #[must_use]
    pub fn is_only(&self, cause: Cause) -> bool {
        self.causes == [cause]
    }

    #[must_use]
    pub fn refresh(&self) -> Refresh {
        match self.causes.iter().fold(Refresh::None, |refresh, cause| {
            refresh.merge(cause.refresh())
        }) {
            Refresh::None => Refresh::Workspace,
            refresh => refresh,
        }
    }

    #[must_use]
    pub fn labels(&self) -> Vec<&'static str> {
        self.causes.iter().map(|cause| cause.label()).collect()
    }
}
