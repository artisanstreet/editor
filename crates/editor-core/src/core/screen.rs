use crate::core::hosts::HostId;
use artisan_api::{EngineId, ProjectId, ThreadId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraggedThread {
    pub host: HostId,
    pub thread: ThreadId,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Screen {
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
    Marketplace,
    Usage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SettingsPage {
    General,
    Section(SettingsSection),
    Hosts,
    About,
    Threads,
    GlobalPrompt,
    Extensions,
    Account,
    Engine(EngineId),
}

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
    const fn parent(self) -> SettingsPage {
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
    const fn label(self) -> &'static str {
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
            Self::McpServers => "MCPs",
            Self::Skills => "Skills",
        }
    }
}

impl SettingsPage {
    #[must_use]
    pub const fn parent(self) -> Self {
        match self {
            Self::Section(section) => section.parent(),
            Self::GlobalPrompt => Self::Threads,
            page @ (Self::General
            | Self::Hosts
            | Self::About
            | Self::Threads
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
            Self::Section(_) | Self::Hosts | Self::About | Self::GlobalPrompt | Self::Account => {
                Vec::new()
            }
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

    pub(crate) const ALL: [(Self, &'static str); 8] = [
        (Self::General, "general"),
        (Self::Account, "account"),
        (Self::Hosts, "hosts"),
        (Self::About, "about"),
        (Self::Threads, "threads"),
        (Self::GlobalPrompt, "global-prompt"),
        (Self::Extensions, "extensions"),
        (Self::Engine(EngineId::ALL[0]), "engines"),
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::General => "General",
            Self::Section(section) => section.label(),
            Self::Hosts => "Computers",
            Self::About => "About",
            Self::Account => "Account",
            Self::Threads => "Threads",
            Self::GlobalPrompt => "Global Prompt",
            Self::Extensions => "Extensions",
            Self::Engine(engine) => engine.display_name(),
        }
    }

    #[must_use]
    pub(crate) fn from_name(name: &str) -> Option<Self> {
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
        if parent == Self::Engine(EngineId::Claude) && section == Some("claude-ai") {
            return Some(parent);
        }
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

    pub fn from_link(link: &str) -> Result<Self, DeepLinkError> {
        if !link.starts_with("artisan:") {
            return Err(DeepLinkError::NotALink);
        }
        let url = url::Url::parse(link).map_err(|source| DeepLinkError::Url {
            link: link.to_owned(),
            source,
        })?;
        let refused = |reason| DeepLinkError::Refused {
            link: link.to_owned(),
            reason,
        };
        if url.scheme() != "artisan" {
            return Err(DeepLinkError::NotALink);
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err(refused("it carries a query or fragment"));
        }
        if !url.username().is_empty() || url.password().is_some() || url.port().is_some() {
            return Err(refused("it names a user or port"));
        }
        let kind = url
            .host_str()
            .ok_or_else(|| refused("it names no screen"))?;
        let path = url.path();
        let path = path.strip_prefix('/').unwrap_or(path);
        let path = path.strip_suffix('/').unwrap_or(path);
        let parts: Vec<&str> = if path.is_empty() {
            Vec::new()
        } else {
            path.split('/').collect()
        };
        let id = |part: &str| {
            let valid = !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
            if valid {
                Ok(part.to_owned())
            } else {
                Err(refused(
                    "an identifier is empty or holds characters ids never use",
                ))
            }
        };
        let settings = |name: &str| {
            SettingsPage::from_name(name)
                .map(Self::Settings)
                .ok_or_else(|| refused("there is no such settings page"))
        };
        match (kind, parts.as_slice()) {
            ("thread", [host, thread]) => Ok(Self::Thread {
                host: HostId(id(host)?),
                thread: ThreadId(id(thread)?),
            }),
            ("new", [host]) => Ok(Self::NewThread {
                host: HostId(id(host)?),
                project: None,
            }),
            ("new", [host, project]) => Ok(Self::NewThread {
                host: HostId(id(host)?),
                project: Some(ProjectId(id(project)?)),
            }),
            ("settings", []) => Ok(Self::Settings(SettingsPage::General)),
            ("settings", rest) => settings(&rest.join("/")),
            ("marketplace", []) => Ok(Self::Marketplace),
            ("usage", []) => Ok(Self::Usage),
            _ => Err(refused("it names no screen the Editor has")),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DeepLinkError {
    #[error("not an artisan:// link")]
    NotALink,
    #[error("read the link {link}")]
    Url {
        link: String,
        #[source]
        source: url::ParseError,
    },
    #[error("the link {link} opens nothing because {reason}")]
    Refused { link: String, reason: &'static str },
}
