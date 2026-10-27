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

/// A settings destination: categories expand to focused subsections in the sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingsPage {
    General,
    Section(SettingsSection),
    Hosts,
    About,
    Threads,
    GlobalPrompt,
    Extensions,
    /// The AST account: sign-in, sessions, and machines. Needs no paired
    /// host; works signed out.
    Account,
    /// One engine's installation and sign-in on the active host.
    Engine(EngineId),
}

/// A focused settings destination beneath a category.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingsSection {
    Appearance,
    Streaming,
    Notifications,
    Performance,
    Automation,
    Assistance,
    Archive,
    Installation(EngineId),
    Account(EngineId),
    ClaudeAi,
    McpServers,
    Skills,
}

impl SettingsSection {
    #[must_use]
    pub const fn parent(self) -> SettingsPage {
        match self {
            Self::Appearance | Self::Streaming | Self::Notifications | Self::Performance => {
                SettingsPage::General
            }
            Self::Automation | Self::Assistance | Self::Archive => SettingsPage::Threads,
            Self::Installation(engine) | Self::Account(engine) => SettingsPage::Engine(engine),
            Self::ClaudeAi => SettingsPage::Engine(EngineId::Claude),
            Self::McpServers | Self::Skills => SettingsPage::Extensions,
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Appearance => "appearance",
            Self::Streaming => "streaming",
            Self::Notifications => "notifications",
            Self::Performance => "performance",
            Self::Automation => "automation",
            Self::Assistance => "assistance",
            Self::Archive => "archive",
            Self::Installation(_) => "installation",
            Self::Account(_) => "account",
            Self::ClaudeAi => "claude-ai",
            Self::McpServers => "mcp-servers",
            Self::Skills => "skills",
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Streaming => "Text Streaming",
            Self::Notifications => "Notifications",
            Self::Performance => "Performance",
            Self::Automation => "Recovery & continuation",
            Self::Assistance => "Assistance",
            Self::Archive => "Archive",
            Self::Installation(_) => "Installation",
            Self::Account(_) => "Account",
            Self::ClaudeAi => "claude.ai mode",
            Self::McpServers => "MCP servers",
            Self::Skills => "Skills",
        }
    }
}

impl SettingsPage {
    /// The category owning this page or subsection.
    #[must_use]
    pub const fn parent(self) -> Self {
        match self {
            Self::Section(section) => section.parent(),
            page @ (Self::General
            | Self::Hosts
            | Self::About
            | Self::Threads
            | Self::GlobalPrompt
            | Self::Extensions
            | Self::Account
            | Self::Engine(_)) => page,
        }
    }

    #[must_use]
    pub fn sections(self) -> Vec<SettingsSection> {
        use SettingsSection as S;
        match self.parent() {
            Self::General => vec![
                S::Appearance,
                S::Streaming,
                S::Notifications,
                S::Performance,
            ],
            Self::Threads => vec![S::Automation, S::Assistance, S::Archive],
            Self::Engine(EngineId::Claude) => vec![
                S::Installation(EngineId::Claude),
                S::Account(EngineId::Claude),
                S::ClaudeAi,
            ],
            Self::Engine(engine) => vec![S::Installation(engine), S::Account(engine)],
            Self::Extensions => vec![S::McpServers, S::Skills],
            Self::Section(_)
            | Self::Hosts
            | Self::About
            | Self::GlobalPrompt
            | Self::Account => Vec::new(),
        }
    }

    #[must_use]
    pub fn shows(self, section: SettingsSection) -> bool {
        match self {
            Self::Section(selected) => selected == section,
            Self::General
            | Self::Hosts
            | Self::About
            | Self::Threads
            | Self::GlobalPrompt
            | Self::Extensions
            | Self::Account
            | Self::Engine(_) => true,
        }
    }

    /// Every page, with its link name.
    pub const ALL: [(Self, &'static str); 8] = [
        (Self::General, "general"),
        (Self::Account, "account"),
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
            Self::Section(section) => section.label(),
            Self::Hosts => "Hosts",
            Self::About => "About",
            Self::Account => "Account",
            Self::Threads => "Threads",
            Self::GlobalPrompt => "Global prompt",
            Self::Extensions => "Extensions",
            Self::Engine(engine) => engine.display_name(),
        }
    }

    /// The page a link name opens; `engines/<engine>` opens that engine.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        let (parent, section) = if let Some(rest) = name.strip_prefix("engines/") {
            match rest.split_once('/') {
                Some((engine, section)) => (Self::Engine(engine.parse().ok()?), Some(section)),
                None => return rest.parse().ok().map(Self::Engine),
            }
        } else {
            let (parent, section) = name
                .split_once('/')
                .map_or((name, None), |(parent, section)| (parent, Some(section)));
            let page = Self::ALL
                .into_iter()
                .find_map(|(page, page_name)| (page_name == parent).then_some(page))?;
            (page, section)
        };
        section.map_or(Some(parent), |name| {
            parent
                .sections()
                .into_iter()
                .find(|section| section.name() == name)
                .map(Self::Section)
        })
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
            ["settings", rest @ ..] => SettingsPage::from_name(&rest.join("/")).map(Self::Settings),
            ["marketplace"] => Some(Self::Marketplace),
            ["usage"] => Some(Self::Usage),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_sections_have_unambiguous_links_and_keep_their_parent() {
        for (parent, name) in SettingsPage::ALL {
            let name = if let SettingsPage::Engine(engine) = parent {
                format!("engines/{engine}")
            } else {
                name.to_owned()
            };
            for section in parent.sections() {
                let path = format!("{name}/{}", section.name());
                assert_eq!(
                    SettingsPage::from_name(&path),
                    Some(SettingsPage::Section(section))
                );
                assert_eq!(
                    Screen::from_link(&format!("artisan://settings/{path}")),
                    Some(Screen::Settings(SettingsPage::Section(section)))
                );
                assert_eq!(section.parent(), parent);
            }
        }
        assert_eq!(SettingsPage::from_name("general/claude-ai"), None);
        assert_eq!(
            SettingsPage::from_name("general/performance/appearance"),
            None
        );
        assert_eq!(SettingsPage::from_name("engines/codex/claude-ai"), None);
        assert_eq!(
            SettingsPage::from_name("engines/claude/claude-ai"),
            Some(SettingsPage::Section(SettingsSection::ClaudeAi))
        );
        assert_eq!(
            Screen::from_link("artisan://settings"),
            Some(Screen::Settings(SettingsPage::General))
        );
    }
}
