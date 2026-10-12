//! Each page's rail label, header and anchors, in the previous Editor's
//! words (`native_settings/sections.rs`).

use artisan_api::EngineId;
use editor_core::SettingsPage;

/// One anchor under the open page's rail row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Anchor {
    pub hash: &'static str,
    pub label: &'static str,
}

const fn anchor(hash: &'static str, label: &'static str) -> Anchor {
    Anchor { hash, label }
}

const GLOBAL_PROMPT_ANCHORS: &[Anchor] =
    &[anchor("prompt", "Prompt"), anchor("engines", "Engines")];
const EXTENSIONS_ANCHORS: &[Anchor] = &[anchor("mcp", "MCP servers"), anchor("skills", "Skills")];

/// The `ARTISAN` group, in the rail's order.
pub(crate) const ARTISAN_PAGES: [SettingsPage; 9] = [
    SettingsPage::Models,
    SettingsPage::GlobalPrompt,
    SettingsPage::Extensions,
    SettingsPage::Threads,
    SettingsPage::Appearance,
    SettingsPage::Notifications,
    SettingsPage::Privacy,
    SettingsPage::Hosts,
    SettingsPage::About,
];

pub(crate) const fn label(page: SettingsPage) -> &'static str {
    match page {
        SettingsPage::Models => "Models",
        SettingsPage::GlobalPrompt => "Global prompt",
        SettingsPage::Threads => "Threads",
        SettingsPage::Appearance => "Appearance",
        SettingsPage::Notifications => "Notifications",
        SettingsPage::Privacy => "Privacy",
        SettingsPage::About => "About",
        SettingsPage::Engines => "Engines",
        SettingsPage::Extensions => "Extensions",
        SettingsPage::Hosts => "Hosts",
    }
}

pub(crate) const fn description(page: SettingsPage) -> &'static str {
    match page {
        SettingsPage::Models => "Forge-owned model defaults shared by every paired client.",
        SettingsPage::GlobalPrompt => {
            "Standing instructions Artisan keeps in every engine. Edit them once here; each engine reads them from its own global file."
        }
        SettingsPage::Threads => "Lifecycle rules the Forge applies to every thread.",
        SettingsPage::Appearance => "How Artisan's surfaces are drawn.",
        SettingsPage::Notifications => "When Artisan is allowed to interrupt you.",
        SettingsPage::Privacy => {
            "Two independent choices for anonymous product analytics and sanitized crash reports."
        }
        SettingsPage::About => "Which build of Artisan is running and where it is installed.",
        SettingsPage::Engines => {
            "Choose where an engine appears, manage its installation, and inspect its account and models."
        }
        SettingsPage::Extensions => {
            "The MCP servers every engine on this host gets, and the way to its skills."
        }
        SettingsPage::Hosts => "The Forges this Editor is paired with.",
    }
}

pub(crate) const fn anchors(page: SettingsPage) -> &'static [Anchor] {
    match page {
        SettingsPage::Models => &[
            Anchor {
                hash: "compaction",
                label: "Compaction",
            },
            Anchor {
                hash: "favorites",
                label: "Favorites",
            },
        ],
        SettingsPage::GlobalPrompt => GLOBAL_PROMPT_ANCHORS,
        SettingsPage::Threads => &[
            Anchor {
                hash: "archived",
                label: "Archived",
            },
            Anchor {
                hash: "retention",
                label: "Retention",
            },
            Anchor {
                hash: "usage-recovery",
                label: "Usage recovery",
            },
            Anchor {
                hash: "agents",
                label: "Agents",
            },
        ],
        SettingsPage::Appearance => &[
            Anchor {
                hash: "app-icon",
                label: "App icon",
            },
            Anchor {
                hash: "typography",
                label: "Typography",
            },
            Anchor {
                hash: "glass",
                label: "Glass",
            },
            Anchor {
                hash: "reading",
                label: "Reading",
            },
        ],
        SettingsPage::Notifications => &[Anchor {
            hash: "system",
            label: "System",
        }],
        SettingsPage::Privacy => &[
            Anchor {
                hash: "telemetry",
                label: "Observability",
            },
            Anchor {
                hash: "never-collected",
                label: "Never collected",
            },
        ],
        SettingsPage::About => &[Anchor {
            hash: "build",
            label: "Build",
        }],
        SettingsPage::Engines => &[
            Anchor {
                hash: "availability",
                label: "Availability",
            },
            Anchor {
                hash: "installation",
                label: "Installation",
            },
            Anchor {
                hash: "account",
                label: "Account",
            },
            Anchor {
                hash: "models",
                label: "Models",
            },
        ],
        SettingsPage::Extensions => EXTENSIONS_ANCHORS,
        SettingsPage::Hosts => &[],
    }
}

/// An engine's product name, as the previous Editor's catalog wrote it.
pub(crate) const fn engine_label(engine: EngineId) -> &'static str {
    engine.display_name()
}
