use editor_sources::SourceProviderExt;
use super::fold::{PlainSection, TextSection};
use artisan_api::{ApprovalRequest, Item, ItemKind, TextContent};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SourceField {
    Plain(PlainSection),
    Text(TextSection),
    Prose,
    Notice,
    Output,
    Question(usize),
    AnswerText,
    ApprovalCommand,
    ApprovalDescription,
    ApprovalReason,
}
impl SourceField {
    pub(crate) const fn of_slot(slot: super::fold::Slot) -> Self {
        match slot {
            super::fold::Slot::Prose => Self::Prose,
            super::fold::Slot::Plain(section) => Self::Plain(section),
            super::fold::Slot::Text(section) => Self::Text(section),
            super::fold::Slot::Output | super::fold::Slot::Preview => Self::Output,
            super::fold::Slot::Answer => Self::AnswerText,
        }
    }
    pub(crate) fn text<'a>(&self, item: &'a Item) -> Option<&'a TextContent> {
        match self {
            Self::Plain(section) => section.text(item),
            Self::Text(section) => section.text(item),
            Self::Prose => match &item.kind {
                ItemKind::AssistantMessage { text, .. } => Some(text),
                ItemKind::UserMessage { .. }
                | ItemKind::Tool { .. }
                | ItemKind::Agent { .. }
                | ItemKind::Question { .. }
                | ItemKind::Plan { .. }
                | ItemKind::Image { .. }
                | ItemKind::Compaction { .. }
                | ItemKind::Notice { .. }
                | ItemKind::Error { .. } => None,
            },
            Self::Notice => match &item.kind {
                ItemKind::Notice { text, .. } => Some(text),
                ItemKind::UserMessage { .. }
                | ItemKind::AssistantMessage { .. }
                | ItemKind::Tool { .. }
                | ItemKind::Agent { .. }
                | ItemKind::Question { .. }
                | ItemKind::Plan { .. }
                | ItemKind::Image { .. }
                | ItemKind::Compaction { .. }
                | ItemKind::Error { .. } => None,
            },
            Self::Output => match &item.kind {
                ItemKind::Tool {
                    output: Some(output),
                    ..
                } => Some(&output.text),
                ItemKind::UserMessage { .. }
                | ItemKind::AssistantMessage { .. }
                | ItemKind::Tool { .. }
                | ItemKind::Agent { .. }
                | ItemKind::Question { .. }
                | ItemKind::Plan { .. }
                | ItemKind::Image { .. }
                | ItemKind::Compaction { .. }
                | ItemKind::Notice { .. }
                | ItemKind::Error { .. } => None,
            },
            Self::Question(index) => match &item.kind {
                ItemKind::Question { questions, .. } => questions
                    .inline()?
                    .get(*index)
                    .map(|question| &question.text),
                ItemKind::UserMessage { .. }
                | ItemKind::AssistantMessage { .. }
                | ItemKind::Tool { .. }
                | ItemKind::Agent { .. }
                | ItemKind::Plan { .. }
                | ItemKind::Image { .. }
                | ItemKind::Compaction { .. }
                | ItemKind::Notice { .. }
                | ItemKind::Error { .. } => None,
            },
            Self::AnswerText => match &item.kind {
                ItemKind::Question { answer_text, .. } => answer_text.as_ref(),
                ItemKind::UserMessage { .. }
                | ItemKind::AssistantMessage { .. }
                | ItemKind::Tool { .. }
                | ItemKind::Agent { .. }
                | ItemKind::Plan { .. }
                | ItemKind::Image { .. }
                | ItemKind::Compaction { .. }
                | ItemKind::Notice { .. }
                | ItemKind::Error { .. } => None,
            },
            Self::ApprovalCommand | Self::ApprovalDescription | Self::ApprovalReason => {
                let approval = match &item.kind {
                    ItemKind::Tool { approval, .. } | ItemKind::Plan { approval, .. } => {
                        approval.as_ref()
                    }
                    ItemKind::UserMessage { .. }
                    | ItemKind::AssistantMessage { .. }
                    | ItemKind::Agent { .. }
                    | ItemKind::Question { .. }
                    | ItemKind::Image { .. }
                    | ItemKind::Compaction { .. }
                    | ItemKind::Notice { .. }
                    | ItemKind::Error { .. } => None,
                }?;
                match (self, &approval.request) {
                    (Self::ApprovalReason, _) => approval.reason.as_ref(),
                    (Self::ApprovalCommand, ApprovalRequest::Command { command, .. }) => {
                        Some(command)
                    }
                    (Self::ApprovalDescription, ApprovalRequest::Action { description }) => {
                        Some(description)
                    }
                    _ => None,
                }
            }
        }
    }
}

use editor_core::{Core, HostId, HostInstance};
use editor_sources::{ContentAction, ContentEffect, ContentText, ContentView};
use aqua_text::document::{SelectionDocumentId, SelectionSourceId};
use artisan_api::{ContentRef, ItemId};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

struct Owner {
    host: HostId,
    instance: HostInstance,
    text: ContentText,
}

#[derive(Default)]
pub struct SourceOwners {
    owners: HashMap<(ItemId, SourceField), Owner>,
}

pub struct SourceAction {
    host: HostId,
    instance: HostInstance,
    item: ItemId,
    field: SourceField,
    source: ContentRef,
    action: ContentAction,
}

impl SourceAction {
    pub fn matches_item(&self, item: &Item) -> bool {
        item.id == self.item
            && matches!(self.field.text(item), Some(TextContent::Stored(source)) if source == &self.source)
    }
}

pub struct SourceExport {
    host: HostId,
    instance: HostInstance,
    item: ItemId,
    field: SourceField,
    source: ContentRef,
    identity: u64,
    generation: u64,
}

impl SourceExport {
    pub fn item(&self) -> &ItemId {
        &self.item
    }
    pub fn matches_item(&self, item: &Item) -> bool {
        item.id == self.item
            && matches!(self.field.text(item), Some(TextContent::Stored(source)) if source == &self.source)
    }
}

pub enum SourceEffect {
    SavePath(SourceExport),
    Clipboard(editor_sources::CopiedSource),
}

impl SourceOwners {
    pub fn export_is_current(&self, export: &SourceExport) -> bool {
        self.owners
            .get(&(export.item.clone(), export.field.clone()))
            .is_some_and(|owner| {
                owner.host == export.host
                    && owner.instance == export.instance
                    && owner.text.identity() == export.identity
                    && owner.text.window().source() == &export.source
                    && owner.text.is_export_pending(export.generation)
            })
    }
    pub fn dispatch_changed(
        &mut self,
        core: &Core,
        action: SourceAction,
    ) -> (bool, Option<SourceEffect>) {
        let before = self
            .owners
            .get(&(action.item.clone(), action.field.clone()))
            .map(|owner| owner.text.view_state());
        let key = (action.item.clone(), action.field.clone());
        let effect = self.dispatch(core, action);
        let after = self.owners.get(&key).map(|owner| owner.text.view_state());
        (before != after, effect)
    }

    pub fn drain_all(&mut self, core: &Core) -> (bool, Vec<SourceEffect>) {
        let mut changed = false;
        let mut effects = Vec::new();
        for ((item, field), owner) in &mut self.owners {
            let (updated, effect) = editor_sources::drain_content_changed(&mut owner.text, core);
            changed |= updated;
            if let Some(effect) = effect {
                effects.push(match effect {
                    ContentEffect::Clipboard(copied) => SourceEffect::Clipboard(copied),
                    ContentEffect::SavePath(generation) => SourceEffect::SavePath(SourceExport {
                        host: owner.host.clone(),
                        instance: owner.instance,
                        item: item.clone(),
                        field: field.clone(),
                        source: owner.text.window().source().clone(),
                        identity: owner.text.identity(),
                        generation,
                    }),
                });
            }
        }
        (changed, effects)
    }
    pub fn open_provider(
        &mut self,
        provider: &editor_core::host::ScopedSourceProvider,
        item: &Item,
        field: SourceField,
    ) -> bool {
        let Some(TextContent::Stored(source)) = field.text(item) else {
            return false;
        };
        if !provider.is_available() {
            return false;
        }
        let key = (item.id.clone(), field);
        let replace = self.owners.get(&key).is_none_or(|owner| {
            &owner.host != provider.host() || owner.instance != provider.instance()
        });
        if replace {
            let Some(text) = provider.open_content(source.clone()) else {
                return false;
            };
            self.owners.insert(
                key,
                Owner {
                    host: provider.host().clone(),
                    instance: provider.instance(),
                    text,
                },
            );
        } else if let Some(owner) = self.owners.get_mut(&key)
            && owner.text.window().source() != source
        {
            owner.text.set_source(source.clone());
        }
        true
    }
    pub fn clear(&mut self) {
        self.owners.clear();
    }

    pub fn retain(
        &mut self,
        core: &Core,
        mut item: impl FnMut(&ItemId) -> Option<std::sync::Arc<Item>>,
    ) {
        self.owners.retain(|(id, field), owner| {
            if core.host_if_instance(&owner.host, owner.instance).is_none() {
                return false;
            }
            let Some(item) = item(id) else { return false };
            match field.text(&item) {
                Some(TextContent::Stored(source)) => {
                    if owner.text.window().source() != source {
                        owner.text.set_source(source.clone());
                    }
                    true
                }
                _ => false,
            }
        });
    }

    pub fn open(
        &mut self,
        core: &Core,
        host: &HostId,
        instance: HostInstance,
        item: &Item,
        field: SourceField,
    ) -> bool {
        let Some(TextContent::Stored(source)) = field.text(item) else {
            return false;
        };
        let Some(host_data) = core.host_if_instance(host, instance) else {
            return false;
        };
        let key = (item.id.clone(), field);
        let replace = self
            .owners
            .get(&key)
            .is_none_or(|owner| owner.host != *host || owner.instance != instance);
        if replace {
            self.owners.insert(
                key,
                Owner {
                    host: host.clone(),
                    instance,
                    text: ContentText::new(host_data, source.clone()),
                },
            );
        } else if let Some(owner) = self.owners.get_mut(&key) {
            owner.text.set_source(source.clone());
        }
        true
    }

    pub fn view(
        &self,
        id: &ItemId,
        field: &SourceField,
        theme: editor_ui::theme::ArtisanTheme,
        document: SelectionDocumentId,
        source_id: SelectionSourceId,
        emit: Rc<dyn Fn(SourceAction)>,
    ) -> Option<ContentView> {
        let owner = self.owners.get(&(id.clone(), field.clone()))?;
        let host = owner.host.clone();
        let instance = owner.instance;
        let item = id.clone();
        let field = field.clone();
        let source = owner.text.window().source().clone();
        let callback = Rc::new(move |action| {
            emit(SourceAction {
                host: host.clone(),
                instance,
                item: item.clone(),
                field: field.clone(),
                source: source.clone(),
                action,
            })
        });
        Some(editor_sources::content_view(
            &owner.text,
            theme,
            document,
            source_id,
            callback,
        ))
    }

    pub fn dispatch(&mut self, core: &Core, action: SourceAction) -> Option<SourceEffect> {
        let owner = self
            .owners
            .get_mut(&(action.item.clone(), action.field.clone()))?;
        if owner.host != action.host
            || owner.instance != action.instance
            || owner.text.window().source() != &action.source
            || core
                .host_if_instance(&action.host, action.instance)
                .is_none()
        {
            return None;
        }
        match editor_sources::dispatch_content(&mut owner.text, core, action.action)? {
            ContentEffect::SavePath(generation) => Some(SourceEffect::SavePath(SourceExport {
                host: action.host,
                instance: action.instance,
                item: action.item,
                field: action.field,
                source: action.source,
                identity: owner.text.identity(),
                generation,
            })),
            ContentEffect::Clipboard(copied) => Some(SourceEffect::Clipboard(copied)),
        }
    }

    pub fn finish_export_result(
        &mut self,
        core: &Core,
        request: SourceExport,
        result: Result<Option<PathBuf>, String>,
    ) {
        let Some(owner) = self.owners.get_mut(&(request.item, request.field)) else {
            return;
        };
        if owner.host == request.host
            && owner.instance == request.instance
            && owner.text.identity() == request.identity
            && owner.text.window().source() == &request.source
            && core
                .host_if_instance(&request.host, request.instance)
                .is_some()
        {
            owner
                .text
                .finish_export_result(core, request.generation, result);
        }
    }

    pub fn drain(&mut self, core: &Core, id: &ItemId, field: &SourceField) -> Option<SourceEffect> {
        let owner = self.owners.get_mut(&(id.clone(), field.clone()))?;
        match editor_sources::drain_content(&mut owner.text, core)? {
            ContentEffect::Clipboard(copied) => Some(SourceEffect::Clipboard(copied)),
            ContentEffect::SavePath(generation) => Some(SourceEffect::SavePath(SourceExport {
                host: owner.host.clone(),
                instance: owner.instance,
                item: id.clone(),
                field: field.clone(),
                source: owner.text.window().source().clone(),
                identity: owner.text.identity(),
                generation,
            })),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&ItemId, &SourceField, &ContentText)> {
        self.owners
            .iter()
            .map(|((id, field), owner)| (id, field, &owner.text))
    }

    pub fn get(&self, id: &ItemId, field: &SourceField) -> Option<&ContentText> {
        self.owners
            .get(&(id.clone(), field.clone()))
            .map(|owner| &owner.text)
    }
}
