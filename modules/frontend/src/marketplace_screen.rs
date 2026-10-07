//! The Marketplace: every skill Artisan keeps in the user's engines, the
//! skills engines have on their own, and the global prompt.
//!
//! The screen renders Forge data pushed into it and emits the user's
//! choices as [`MarketplaceEvent`]s; the application sends them to the Forge.
//! Nothing here decides what an engine holds.

#![forbid(unsafe_code)]

mod render;

use std::collections::BTreeSet;

use artisan_domain::{ExtensionCatalog, GlobalPrompt};
use artisan_ui::theme::ArtisanTheme;
use gpui::{Context, EventEmitter, FocusHandle};

/// Root selector of the Marketplace surface.
pub const MARKETPLACE_SCREEN_SELECTOR: &str = "route-marketplace";

/// What the user asked for on the Marketplace.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MarketplaceEvent {
    /// Turn a managed skill on or off in every engine.
    SetEnabled {
        /// The managed extension.
        extension_id: String,
        /// Whether engines get it.
        enabled: bool,
    },
    /// Adopt a skill an engine has on its own.
    Adopt {
        /// The discovered extension.
        extension_id: String,
    },
    /// Open the global prompt page in Settings.
    OpenGlobalPrompt,
    /// Open the Windows setting that allows symbolic links.
    OpenDeveloperMode,
}

/// The Marketplace surface.
pub struct MarketplaceScreen {
    theme: ArtisanTheme,
    focus: FocusHandle,
    catalog: Option<ExtensionCatalog>,
    prompt: Option<GlobalPrompt>,
    /// Extensions with a change in flight; their controls are disabled.
    pending: BTreeSet<String>,
    /// The last refused change, shown above the sections.
    notice: Option<String>,
}

impl EventEmitter<MarketplaceEvent> for MarketplaceScreen {}

impl MarketplaceScreen {
    /// Creates an empty Marketplace waiting for the Forge's catalog.
    pub fn new(theme: ArtisanTheme, cx: &mut Context<Self>) -> Self {
        Self {
            theme,
            focus: cx.focus_handle(),
            catalog: None,
            prompt: None,
            pending: BTreeSet::new(),
            notice: None,
        }
    }

    /// Replaces the catalog; notifies only on change.
    pub fn set_catalog(&mut self, catalog: Option<ExtensionCatalog>, cx: &mut Context<Self>) {
        if self.catalog != catalog {
            self.catalog = catalog;
            cx.notify();
        }
    }

    /// Replaces the global prompt; notifies only on change.
    pub fn set_prompt(&mut self, prompt: Option<GlobalPrompt>, cx: &mut Context<Self>) {
        if self.prompt != prompt {
            self.prompt = prompt;
            cx.notify();
        }
    }

    /// Replaces the extensions with a change in flight.
    pub fn set_pending(&mut self, pending: BTreeSet<String>, cx: &mut Context<Self>) {
        if self.pending != pending {
            self.pending = pending;
            cx.notify();
        }
    }

    /// Shows or clears the last refusal.
    pub fn set_notice(&mut self, notice: Option<String>, cx: &mut Context<Self>) {
        if self.notice != notice {
            self.notice = notice;
            cx.notify();
        }
    }

    /// The catalog on screen.
    #[must_use]
    pub const fn catalog(&self) -> Option<&ExtensionCatalog> {
        self.catalog.as_ref()
    }

    /// The refusal on screen.
    #[must_use]
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }
}
