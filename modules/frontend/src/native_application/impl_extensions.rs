//! Agent extensions and the global prompt as the Editor sees them: the
//! catalog and prompt the Forge serves and pushes, the Marketplace and the
//! global prompt page that render them, and the user's changes sent back.
//!
//! The Editor decides nothing about what an engine holds; it renders Forge
//! data and forwards the user's choices. A change in flight disables its
//! control until the Forge answers; a refusal is worded from what the user
//! attempted, because the transport carries no Forge detail text.

use std::collections::{BTreeMap, BTreeSet};

use artisan_domain::{
    AdoptExtension, ExtensionCatalog, GlobalPrompt, GlobalPromptResolution, GlobalPromptRevision,
    RequestId, ResolveGlobalPrompt, SaveGlobalPrompt, SetExtensionEnabled,
};

use crate::marketplace_screen::{MarketplaceEvent, MarketplaceScreen};
use crate::native_transport_service::{ExtensionsCommand, ExtensionsEvent, NativeTransportCommand};

use super::state::mint_request_id;
use super::{Context, Entity, NativeApplication, NativeRoute, SettingsRoute};

/// What the Editor holds of the Forge's extension state; dropped with the
/// view on a host switch.
#[derive(Debug, Default)]
pub(super) struct ExtensionsState {
    catalog: Option<ExtensionCatalog>,
    prompt: Option<GlobalPrompt>,
    catalog_requested: bool,
    prompt_requested: bool,
    /// Enable and adopt requests in flight, by request, with their
    /// extension.
    pending: BTreeMap<RequestId, String>,
    /// The global prompt save or resolution in flight.
    pending_prompt: Option<RequestId>,
    /// The last refused extension change.
    catalog_notice: Option<String>,
    /// The last refused global prompt change.
    prompt_notice: Option<String>,
}

impl NativeApplication {
    /// Reads the catalog and the global prompt once per connection; later
    /// changes are pushed by the Forge.
    pub(super) fn ensure_extensions_read(&mut self) {
        if !self.extensions.catalog_requested
            && self
                .submit_command(NativeTransportCommand::Extensions(
                    ExtensionsCommand::ReadCatalog,
                ))
                .is_ok()
        {
            self.extensions.catalog_requested = true;
        }
        if !self.extensions.prompt_requested
            && self
                .submit_command(NativeTransportCommand::Extensions(
                    ExtensionsCommand::ReadPrompt,
                ))
                .is_ok()
        {
            self.extensions.prompt_requested = true;
        }
    }

    /// The catalog as last served, if any.
    pub(super) fn extensions_catalog(&self) -> Option<&ExtensionCatalog> {
        self.extensions.catalog.as_ref()
    }

    /// Applies a pushed or served catalog.
    pub(super) fn apply_extension_catalog(
        &mut self,
        catalog: ExtensionCatalog,
        cx: &mut Context<Self>,
    ) {
        self.extensions.catalog = Some(catalog);
        self.refresh_marketplace_screen(cx);
    }

    /// Applies a pushed or served global prompt.
    pub(super) fn apply_global_prompt(&mut self, prompt: GlobalPrompt, cx: &mut Context<Self>) {
        self.extensions.prompt = Some(prompt);
        self.refresh_marketplace_screen(cx);
        self.refresh_settings_global_prompt(cx);
    }

    /// Receives one extensions transport answer.
    pub(super) fn receive_extensions(&mut self, event: ExtensionsEvent, cx: &mut Context<Self>) {
        match event {
            ExtensionsEvent::Catalog(Ok(catalog)) => self.apply_extension_catalog(*catalog, cx),
            ExtensionsEvent::Catalog(Err(_)) => {
                self.extensions.catalog_requested = false;
                self.extensions.catalog_notice = Some(
                    "The Forge could not list your skills. Reopen the Marketplace to retry."
                        .to_owned(),
                );
                self.refresh_marketplace_screen(cx);
            }
            ExtensionsEvent::Prompt(Ok(prompt)) => self.apply_global_prompt(*prompt, cx),
            ExtensionsEvent::Prompt(Err(_)) => {
                self.extensions.prompt_requested = false;
                self.extensions.prompt_notice = Some(
                    "The Forge could not read the global prompt. Reopen this page to retry."
                        .to_owned(),
                );
                self.refresh_settings_global_prompt(cx);
            }
            ExtensionsEvent::CatalogChanged { request_id, result } => {
                let Some(extension_id) = self.extensions.pending.remove(&request_id) else {
                    return;
                };
                if let Ok(catalog) = result {
                    self.extensions.catalog_notice = None;
                    self.apply_extension_catalog(*catalog, cx);
                } else {
                    let name = self.extension_name(&extension_id);
                    self.extensions.catalog_notice = Some(format!(
                        "Artisan could not update {name}. Its engines are unchanged; try again."
                    ));
                    self.refresh_marketplace_screen(cx);
                }
            }
            ExtensionsEvent::PromptChanged { request_id, result } => {
                if self.extensions.pending_prompt.as_ref() != Some(&request_id) {
                    return;
                }
                self.extensions.pending_prompt = None;
                if let Ok(prompt) = result {
                    self.extensions.prompt_notice = None;
                    self.apply_global_prompt(*prompt, cx);
                } else {
                    self.extensions.prompt_notice = Some(
                        "The global prompt was not saved. If it changed elsewhere, load the latest version and apply your edit again."
                            .to_owned(),
                    );
                    self.refresh_settings_global_prompt(cx);
                }
            }
        }
    }

    fn extension_name(&self, extension_id: &str) -> String {
        self.extensions
            .catalog
            .as_ref()
            .and_then(|catalog| catalog.extension(extension_id))
            .map_or_else(|| "the skill".to_owned(), |entry| entry.name.clone())
    }

    /// Serves one Marketplace event.
    pub(super) fn handle_marketplace_event(
        &mut self,
        _screen: Entity<MarketplaceScreen>,
        event: &MarketplaceEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            MarketplaceEvent::SetEnabled {
                extension_id,
                enabled,
            } => {
                let Ok(request_id) = mint_request_id("extension-enable") else {
                    return;
                };
                let command = ExtensionsCommand::SetEnabled(SetExtensionEnabled {
                    request_id: request_id.clone(),
                    extension_id: extension_id.clone(),
                    enabled: *enabled,
                });
                self.send_extension_change(request_id, extension_id, command, cx);
            }
            MarketplaceEvent::Adopt { extension_id } => {
                let Ok(request_id) = mint_request_id("extension-adopt") else {
                    return;
                };
                let command = ExtensionsCommand::Adopt(AdoptExtension {
                    request_id: request_id.clone(),
                    extension_id: extension_id.clone(),
                });
                self.send_extension_change(request_id, extension_id, command, cx);
            }
            MarketplaceEvent::OpenGlobalPrompt => {
                self.navigate(
                    NativeRoute::Settings {
                        section: SettingsRoute::GlobalPrompt,
                        engine: None,
                    },
                    cx,
                );
            }
            MarketplaceEvent::OpenDeveloperMode => cx.open_url("ms-settings:developers"),
        }
    }

    fn send_extension_change(
        &mut self,
        request_id: RequestId,
        extension_id: &str,
        command: ExtensionsCommand,
        cx: &mut Context<Self>,
    ) {
        if self
            .submit_command(NativeTransportCommand::Extensions(command))
            .is_ok()
        {
            self.extensions.catalog_notice = None;
            self.extensions
                .pending
                .insert(request_id, extension_id.to_owned());
        } else {
            self.extensions.catalog_notice =
                Some("The Editor is not connected to a Forge.".to_owned());
        }
        self.refresh_marketplace_screen(cx);
    }

    /// Saves the global prompt the user edited from `base`.
    pub(super) fn save_global_prompt(
        &mut self,
        base: GlobalPromptRevision,
        text: String,
        cx: &mut Context<Self>,
    ) {
        let Ok(request_id) = mint_request_id("global-prompt-save") else {
            return;
        };
        let command = ExtensionsCommand::SavePrompt(SaveGlobalPrompt {
            request_id: request_id.clone(),
            base_revision: base,
            text,
        });
        self.send_prompt_change(request_id, command, cx);
    }

    /// Settles global prompt setup or a drifted engine.
    pub(super) fn resolve_global_prompt(
        &mut self,
        resolution: GlobalPromptResolution,
        cx: &mut Context<Self>,
    ) {
        let Ok(request_id) = mint_request_id("global-prompt-resolve") else {
            return;
        };
        let command = ExtensionsCommand::ResolvePrompt(ResolveGlobalPrompt {
            request_id: request_id.clone(),
            resolution,
        });
        self.send_prompt_change(request_id, command, cx);
    }

    fn send_prompt_change(
        &mut self,
        request_id: RequestId,
        command: ExtensionsCommand,
        cx: &mut Context<Self>,
    ) {
        if self
            .submit_command(NativeTransportCommand::Extensions(command))
            .is_ok()
        {
            self.extensions.prompt_notice = None;
            self.extensions.pending_prompt = Some(request_id);
        } else {
            self.extensions.prompt_notice =
                Some("The Editor is not connected to a Forge.".to_owned());
        }
        self.refresh_settings_global_prompt(cx);
    }

    /// Pushes the current state into a mounted Marketplace.
    pub(super) fn refresh_marketplace_screen(&mut self, cx: &mut Context<Self>) {
        let Some(screen) = self.marketplace_screen.clone() else {
            return;
        };
        let catalog = self.extensions.catalog.clone();
        let prompt = self.extensions.prompt.clone();
        let pending: BTreeSet<String> = self.extensions.pending.values().cloned().collect();
        let notice = self.extensions.catalog_notice.clone();
        screen.update(cx, |screen, screen_cx| {
            screen.set_catalog(catalog, screen_cx);
            screen.set_prompt(prompt, screen_cx);
            screen.set_pending(pending, screen_cx);
            screen.set_notice(notice, screen_cx);
        });
    }

    /// Pushes the current state into a mounted global prompt page.
    pub(super) fn refresh_settings_global_prompt(&mut self, cx: &mut Context<Self>) {
        if !matches!(
            self.settings_screen_key,
            Some((SettingsRoute::GlobalPrompt, _))
        ) {
            return;
        }
        let Some(screen) = self.settings_screen.clone() else {
            return;
        };
        let prompt = self.extensions.prompt.clone();
        let pending = self.extensions.pending_prompt.is_some();
        let notice = self.extensions.prompt_notice.clone();
        screen.update(cx, |screen, screen_cx| {
            screen.set_global_prompt(prompt, pending, notice, screen_cx);
        });
    }
}
