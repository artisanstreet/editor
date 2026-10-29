//! Each page's header copy. The sidebar lists the pages.

use artisan_api::EngineId;
use editor_core::SettingsPage;

pub const fn label(page: SettingsPage) -> &'static str {
    page.label()
}

pub const fn description(page: SettingsPage) -> &'static str {
    match page {
        SettingsPage::Section(section) => match section {
            editor_core::SettingsSection::Appearance => {
                "Theme, fonts and text rendering on this device."
            }
            editor_core::SettingsSection::Streaming => {
                "How replies appear while an agent is working."
            }
            editor_core::SettingsSection::Notifications => {
                "When the Editor sends system notifications."
            }
            editor_core::SettingsSection::Performance => {
                "Animation and refresh rate on this device."
            }
            editor_core::SettingsSection::Automation => {
                "When threads retry and carry on automatically on this host."
            }
            editor_core::SettingsSection::Assistance => {
                "Suggestions, notes and titles for threads on this host."
            }
            editor_core::SettingsSection::Archive => {
                "Find and restore archived threads on this host."
            }
            editor_core::SettingsSection::ClaudeAi => {
                "Use claude.ai for Claude turns and manage its session on this host."
            }
            editor_core::SettingsSection::Installation(_) => {
                "Install, update and choose an engine version on this host."
            }
            editor_core::SettingsSection::Account(_) => {
                "Manage this engine’s account on this host."
            }
            editor_core::SettingsSection::McpServers => {
                "Tools shared with every engine on this host."
            }
            editor_core::SettingsSection::Skills => "Manage the skills available to your engines.",
        },
        SettingsPage::General => "How this Editor behaves on this device.",
        SettingsPage::Hosts => "The account's Forges this Editor has open.",
        SettingsPage::Account => {
            "Your AST account: sign in, sessions, and the Forges on the account."
        }
        SettingsPage::About => "Which build of Artisan is running and where it is installed.",
        SettingsPage::Threads => {
            "What the host does around your threads, and the ones you archived."
        }
        SettingsPage::GlobalPrompt => {
            "Standing instructions Artisan keeps in every engine. Edit them once here; each engine reads them from its own global file."
        }
        SettingsPage::Extensions => {
            "The MCP servers every engine on this host gets, and the way to its skills."
        }
        SettingsPage::Engine(_) => "Installation and sign-in on this host.",
    }
}

/// An engine's product name, as the previous Editor's catalog wrote it.
pub const fn engine_label(engine: EngineId) -> &'static str {
    engine.display_name()
}
