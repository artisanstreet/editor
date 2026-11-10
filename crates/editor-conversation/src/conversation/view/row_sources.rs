use super::row_cache::RowOwner;
use crate::conversation::content::{SourceAction, SourceEffect, SourceExport};
use crate::conversation::presentation::PresentationScope;
use crate::conversation::questions::QuestionCollectionAction;
use editor_core::Core;
use editor_core::host::ScopedSourceProvider;
use artisan_api::{Item, ItemId};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

pub enum RowSourceActionKind {
    Content(SourceAction),
    Questions(QuestionCollectionAction),
    QuestionDetail {
        collection: artisan_api::CollectionRef,
        source: artisan_api::ContentRef,
        identity: u64,
        action: editor_sources::ContentAction,
    },
}

pub struct RowSourceAction {
    pub item: ItemId,
    pub seq: artisan_api::Seq,
    pub scope: PresentationScope,
    pub provider: ScopedSourceProvider,
    pub owner: Weak<RowOwner>,
    pub action: RowSourceActionKind,
}

pub enum RowSourceEffect {
    SavePath(RowSourceExport),
    Clipboard(editor_sources::CopiedSource),
}

pub struct RowSourceExport {
    pub item: ItemId,
    pub scope: PresentationScope,
    pub provider: ScopedSourceProvider,
    owner: Weak<RowOwner>,
    export: ExportKind,
}

enum ExportKind {
    Content(SourceExport),
    QuestionDetail {
        collection: artisan_api::CollectionRef,
        source: artisan_api::ContentRef,
        identity: u64,
        generation: u64,
    },
}

fn question_matches(item: &Item, collection: &artisan_api::CollectionRef) -> bool {
    matches!(&item.kind, artisan_api::ItemKind::Question { questions: artisan_api::Collection::Stored(current), .. } if current == collection)
}

impl RowSourceAction {
    pub fn dispatch(
        self,
        core: &Core,
        scope: &PresentationScope,
        item: &Item,
    ) -> (bool, Option<RowSourceEffect>) {
        if &self.scope != scope
            || !self.provider.is_available()
            || self.provider.host() != &scope.host
            || self.provider.instance() != scope.instance
            || core.host_if_instance(&scope.host, scope.instance).is_none()
            || item.id != self.item
            || item.seq != self.seq
        {
            return (false, None);
        }
        let Some(owner) = self.owner.upgrade() else {
            return (false, None);
        };
        if owner.identity().is_none_or(|(_, current)| current != scope) {
            return (false, None);
        }
        match self.action {
            RowSourceActionKind::Content(action) if action.matches_item(item) => {
                let (changed, effect) = owner.sources.borrow_mut().dispatch_changed(core, action);
                let effect = effect.map(|effect| match effect {
                    SourceEffect::Clipboard(copied) => RowSourceEffect::Clipboard(copied),
                    SourceEffect::SavePath(export) => RowSourceEffect::SavePath(RowSourceExport {
                        item: self.item,
                        scope: self.scope,
                        provider: self.provider,
                        owner: Rc::downgrade(&owner),
                        export: ExportKind::Content(export),
                    }),
                });
                (changed, effect)
            }
            RowSourceActionKind::Questions(action) if action.matches_item(item) => (
                owner.questions.borrow_mut().dispatch_changed(core, action),
                None,
            ),
            RowSourceActionKind::QuestionDetail {
                collection,
                source,
                identity,
                action,
            } if question_matches(item, &collection) => {
                let Some(browser) = owner.questions.borrow().browser() else {
                    return (false, None);
                };
                let mut browser = browser.borrow_mut();
                if browser.window().source() != &collection {
                    return (false, None);
                }
                let Some(detail) = browser.detail.as_mut() else {
                    return (false, None);
                };
                if detail.identity() != identity || detail.window().source() != &source {
                    return (false, None);
                }
                let (changed, effect) =
                    editor_sources::dispatch_content_changed(detail, core, action);
                let effect = effect.map(|effect| match effect {
                    editor_sources::ContentEffect::Clipboard(copied) => {
                        RowSourceEffect::Clipboard(copied)
                    }
                    editor_sources::ContentEffect::SavePath(generation) => {
                        RowSourceEffect::SavePath(RowSourceExport {
                            item: self.item,
                            scope: self.scope,
                            provider: self.provider,
                            owner: Rc::downgrade(&owner),
                            export: ExportKind::QuestionDetail {
                                collection,
                                source,
                                identity,
                                generation,
                            },
                        })
                    }
                });
                (changed, effect)
            }
            _ => (false, None),
        }
    }
}

impl RowSourceExport {
    pub fn is_current(&self, core: &Core, scope: &PresentationScope, item: &Item) -> bool {
        &self.scope == scope
            && item.id == self.item
            && self.provider.is_available()
            && self.provider.host() == &scope.host
            && self.provider.instance() == scope.instance
            && core.host_if_instance(&scope.host, scope.instance).is_some()
            && self.owner.strong_count() > 0
            && self.owner.upgrade().is_some_and(|owner| {
                owner
                    .identity()
                    .is_some_and(|(_, current)| current == scope)
                    && match &self.export {
                        ExportKind::Content(export) => {
                            export.matches_item(item)
                                && owner.sources.borrow().export_is_current(export)
                        }
                        ExportKind::QuestionDetail {
                            collection,
                            source,
                            identity,
                            generation,
                        } => {
                            question_matches(item, collection)
                                && owner.questions.borrow().browser().is_some_and(|browser| {
                                    let browser = browser.borrow();
                                    browser.window().source() == collection
                                        && browser.detail.as_ref().is_some_and(|detail| {
                                            detail.identity() == *identity
                                                && detail.window().source() == source
                                                && detail.is_export_pending(*generation)
                                        })
                                })
                        }
                    }
            })
    }

    pub fn finish(
        self,
        core: &Core,
        scope: &PresentationScope,
        item: &Item,
        result: Result<Option<PathBuf>, String>,
    ) {
        if !self.is_current(core, scope, item) {
            return;
        }
        if let Some(owner) = self.owner.upgrade() {
            match self.export {
                ExportKind::Content(export) => owner
                    .sources
                    .borrow_mut()
                    .finish_export_result(core, export, result),
                ExportKind::QuestionDetail { generation, .. } => {
                    if let Some(browser) = owner.questions.borrow().browser()
                        && let Some(detail) = browser.borrow_mut().detail.as_mut()
                    {
                        detail.finish_export_result(core, generation, result);
                    }
                }
            }
        }
    }
}

impl RowOwner {
    pub fn bind_sources(&self, ctx: &aqua::AppCtx<'_>, window: aqua::prelude::AquaWindowId) {
        for (_, _, content) in self.sources.borrow().iter() {
            ctx.bind_mailbox(window, content.action_wake());
            ctx.bind_mailbox(window, content.window().wake_mailbox());
        }
        if let Some(browser) = self.questions.borrow().browser() {
            let browser = browser.borrow();
            ctx.bind_mailbox(window, browser.window().wake_mailbox());
            if let Some(content) = &browser.detail {
                ctx.bind_mailbox(window, content.action_wake());
                ctx.bind_mailbox(window, content.window().wake_mailbox());
            }
        }
    }

    pub fn drain_sources(
        self: &Rc<Self>,
        core: &Core,
        scope: &PresentationScope,
        provider: &ScopedSourceProvider,
        resolve: impl Fn(&ItemId) -> Option<std::sync::Arc<Item>>,
    ) -> (bool, Vec<RowSourceEffect>) {
        if !provider.is_available()
            || provider.host() != &scope.host
            || provider.instance() != scope.instance
            || core.host_if_instance(&scope.host, scope.instance).is_none()
            || self.identity().is_none_or(|(_, current)| current != scope)
        {
            return (false, Vec::new());
        }
        self.sources.borrow_mut().retain(core, |id| resolve(id));
        self.questions.borrow_mut().retain(core, |id| resolve(id));
        let (sources, effects) = self.sources.borrow_mut().drain_all(core);
        let questions = self.questions.borrow_mut().drain_changed(core);
        let mut effects: Vec<_> = effects
            .into_iter()
            .map(|effect| match effect {
                SourceEffect::Clipboard(copied) => RowSourceEffect::Clipboard(copied),
                SourceEffect::SavePath(export) => RowSourceEffect::SavePath(RowSourceExport {
                    item: export.item().clone(),
                    scope: scope.clone(),
                    provider: provider.clone(),
                    owner: Rc::downgrade(self),
                    export: ExportKind::Content(export),
                }),
            })
            .collect();
        let mut detail_changed = false;
        let (identity, browser) = {
            let questions = self.questions.borrow();
            (questions.identity(), questions.browser())
        };
        if let (Some((item, collection)), Some(browser)) = (identity, browser) {
            let mut browser = browser.borrow_mut();
            if let Some(detail) = browser.detail.as_mut() {
                let source = detail.window().source().clone();
                let identity = detail.identity();
                let (changed, effect) = editor_sources::drain_content_changed(detail, core);
                detail_changed = changed;
                if let Some(effect) = effect {
                    effects.push(match effect {
                        editor_sources::ContentEffect::Clipboard(copied) => {
                            RowSourceEffect::Clipboard(copied)
                        }
                        editor_sources::ContentEffect::SavePath(generation) => {
                            RowSourceEffect::SavePath(RowSourceExport {
                                item,
                                scope: scope.clone(),
                                provider: provider.clone(),
                                owner: Rc::downgrade(self),
                                export: ExportKind::QuestionDetail {
                                    collection,
                                    source,
                                    identity,
                                    generation,
                                },
                            })
                        }
                    });
                }
            }
        }
        (sources || questions || detail_changed, effects)
    }
}
