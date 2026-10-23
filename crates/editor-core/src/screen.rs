//! Navigation is an enum, not URL strings.

use crate::hosts::HostId;
use artisan_api::{EngineId, ProjectId, ThreadId};

/// A sidebar thread being dragged: the composer it lands on mentions it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraggedThread {
    pub host: HostId,
    pub thread: ThreadId,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Screen {
    /// Nothing open yet, or no host paired.
    Home,
    Thread {
        host: HostId,
        thread: ThreadId,
    },
    NewThread {
        host: HostId,
        project: Option<ProjectId>,
    },
    Settings(SettingsPage),
    /// The active host's skills and global prompt.
    Marketplace,
    /// Every paired host's engine accounts and their limits.
    Usage,
}

/// A settings page. The sidebar groups the Editor's own pages, the active
/// host's pages, then each engine on that host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingsPage {
    General,
    Hosts,
    About,
    Threads,
    GlobalPrompt,
    Extensions,
    /// One engine's installation and sign-in on the active host.
    Engine(EngineId),
}

impl SettingsPage {
    /// Every page, with its link name.
    pub const ALL: [(Self, &'static str); 7] = [
        (Self::General, "general"),
        (Self::Hosts, "hosts"),
        (Self::About, "about"),
        (Self::Threads, "threads"),
        (Self::GlobalPrompt, "global-prompt"),
        (Self::Extensions, "extensions"),
        (Self::Engine(EngineId::ALL[0]), "engines"),
    ];

    /// The page's name in the sidebar and its header.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Hosts => "Hosts",
            Self::About => "About",
            Self::Threads => "Threads",
            Self::GlobalPrompt => "Global prompt",
            Self::Extensions => "Extensions",
            Self::Engine(engine) => engine.display_name(),
        }
    }

    /// The page a link name opens; `engines/<engine>` opens that engine.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        if let Some(engine) = name.strip_prefix("engines/") {
            return engine.parse().ok().map(Self::Engine);
        }
        Self::ALL
            .into_iter()
            .find_map(|(page, page_name)| (page_name == name).then_some(page))
    }
}

impl Screen {
    #[must_use]
    pub const fn host(&self) -> Option<&HostId> {
        match self {
            Self::Thread { host, .. } | Self::NewThread { host, .. } => Some(host),
            Self::Home | Self::Settings(_) | Self::Marketplace | Self::Usage => None,
        }
    }

    #[must_use]
    pub const fn thread(&self) -> Option<&ThreadId> {
        match self {
            Self::Thread { thread, .. } => Some(thread),
            Self::Home
            | Self::NewThread { .. }
            | Self::Settings(_)
            | Self::Marketplace
            | Self::Usage => None,
        }
    }

    /// `artisan://thread/<host>/<thread>`, `artisan://new/<host>[/<project>]`
    /// `artisan://settings[/<page>]`, `artisan://marketplace` and
    /// `artisan://usage`.
    #[must_use]
    pub fn from_link(link: &str) -> Option<Self> {
        let rest = link.strip_prefix("artisan://")?;
        let parts: Vec<&str> = rest.trim_end_matches('/').split('/').collect();
        match parts.as_slice() {
            ["thread", host, thread] => Some(Self::Thread {
                host: HostId((*host).to_owned()),
                thread: ThreadId((*thread).to_owned()),
            }),
            ["new", host] => Some(Self::NewThread {
                host: HostId((*host).to_owned()),
                project: None,
            }),
            ["new", host, project] => Some(Self::NewThread {
                host: HostId((*host).to_owned()),
                project: Some(ProjectId((*project).to_owned())),
            }),
            ["settings"] => Some(Self::Settings(SettingsPage::General)),
            ["settings", page] => SettingsPage::from_name(page).map(Self::Settings),
            ["settings", "engines", engine] => engine
                .parse()
                .ok()
                .map(|engine| Self::Settings(SettingsPage::Engine(engine))),
            ["marketplace"] => Some(Self::Marketplace),
            ["usage"] => Some(Self::Usage),
            _ => None,
        }
    }
}
