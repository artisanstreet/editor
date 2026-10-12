//! Navigation is an enum, not URL strings.

use crate::hosts::HostId;
use artisan_api::{ProjectId, ThreadId};

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
}

/// A settings page. The rail lists the Artisan pages, then each engine;
/// `Extensions` and `Hosts` open from the sidebar and the profile menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingsPage {
    Models,
    GlobalPrompt,
    Threads,
    Appearance,
    Notifications,
    Privacy,
    About,
    Engines,
    Extensions,
    Hosts,
}

impl SettingsPage {
    /// Every page, with its link name.
    pub const ALL: [(Self, &'static str); 10] = [
        (Self::Models, "models"),
        (Self::GlobalPrompt, "global-prompt"),
        (Self::Threads, "threads"),
        (Self::Appearance, "appearance"),
        (Self::Notifications, "notifications"),
        (Self::Privacy, "privacy"),
        (Self::About, "about"),
        (Self::Engines, "engines"),
        (Self::Extensions, "extensions"),
        (Self::Hosts, "hosts"),
    ];

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find_map(|(page, page_name)| (page_name == name).then_some(page))
    }
}

impl Screen {
    #[must_use]
    pub fn host(&self) -> Option<&HostId> {
        match self {
            Self::Thread { host, .. } | Self::NewThread { host, .. } => Some(host),
            Self::Home | Self::Settings(_) | Self::Marketplace => None,
        }
    }

    #[must_use]
    pub fn thread(&self) -> Option<&ThreadId> {
        match self {
            Self::Thread { thread, .. } => Some(thread),
            _ => None,
        }
    }

    /// `artisan://thread/<host>/<thread>`, `artisan://new/<host>[/<project>]`
    /// `artisan://settings[/<page>]` and `artisan://marketplace`.
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
            ["settings"] => Some(Self::Settings(SettingsPage::Models)),
            ["settings", page] => SettingsPage::from_name(page).map(Self::Settings),
            ["marketplace"] => Some(Self::Marketplace),
            _ => None,
        }
    }
}
