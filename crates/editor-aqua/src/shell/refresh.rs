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

    pub fn from_causes(causes: &[&str]) -> Self {
        let refresh = causes.iter().fold(Self::None, |refresh, cause| {
            refresh.merge(match *cause {
                "views_frame" => Self::None,
                "anim_clock" | "landing_expiry" | "reveals" | "after_layout" | "al_motion"
                | "al_compaction" | "al_reveal" | "al_stored" | "disclosure" | "viewport"
                | "viewport_rows" | "landed" => Self::Transcript,
                "sidebar_motion" | "sidebar_tick" => Self::Sidebar,
                "anim_sidebar" | "anim_sidebar_resize" | "anim_edges" => Self::SidebarFrame,
                _ => Self::Workspace,
            })
        });
        match refresh {
            Self::None => Self::Workspace,
            refresh => refresh,
        }
    }
}
