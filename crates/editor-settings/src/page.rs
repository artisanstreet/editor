//! Each page's header copy. The sidebar lists the pages.

use artisan_api::EngineId;
use editor_core::SettingsPage;

pub const fn label(page: SettingsPage) -> &'static str {
    page.label()
}

pub const fn description(page: SettingsPage) -> &'static str {
    match page {
        SettingsPage::General => "How this Editor behaves on this device.",
        SettingsPage::Hosts => "The Forges this Editor is paired with.",
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
