use crate::conversation::content::{SourceAction, SourceField, SourceOwners};
use crate::conversation::presentation::PresentationScope;
use crate::conversation::questions::{QuestionCollection, QuestionCollectionAction};
use editor_core::Core;
use editor_ui::theme::ArtisanTheme;
use aqua_text::document::SelectionSource;
use aqua_text::document::{SelectionDocumentId, SelectionSourceId};
use aqua_ui::collection::{BatchedCollectionRow, SelectionBatchOwner, SelectionSourceBatch};
use aqua_ui::collection_runtime::PreparedCollectionRow;
use aqua_ui::element::Element;
use artisan_api::{CollectionRef, Item};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ResourceKey {
    Source(artisan_api::ItemId, SourceField),
    Questions(artisan_api::ItemId),
    QuestionDetail(artisan_api::ItemId),
}

struct ResourceBatch {
    owner: SelectionBatchOwner,
    revision: u64,
    source: Option<SelectionSource>,
    batch: Arc<SelectionSourceBatch>,
}

#[derive(Default)]
pub struct ResourceBatches {
    scope: Option<PresentationScope>,
    entries: HashMap<ResourceKey, ResourceBatch>,
}

pub struct BatchRelease {
    document: SelectionDocumentId,
    owner: SelectionBatchOwner,
    source: Option<SelectionSourceId>,
}

impl BatchRelease {
    pub fn apply(self, app: &mut aqua::UiApp) {
        app.release_selection_batch_owner(self.document, self.owner);
        if let Some(source) = self.source {
            app.unregister_selection_source_in(self.document, source);
        }
    }
}

fn same_source(left: &Option<SelectionSource>, right: &Option<SelectionSource>) -> bool {
    match (left, right) {
        (Some(a), Some(b)) => {
            a.id == b.id
                && a.order == b.order
                && a.text == b.text
                && a.separator == b.separator
                && a.copies == b.copies
        }
        (None, None) => true,
        _ => false,
    }
}

impl ResourceBatches {
    pub fn set_scope(&mut self, scope: &PresentationScope) -> bool {
        if self.scope.as_ref().is_some_and(|current| current != scope) {
            return false;
        }
        self.scope = Some(scope.clone());
        true
    }

    pub fn take_releases(&mut self) -> Vec<BatchRelease> {
        self.scope = None;
        self.entries
            .drain()
            .map(|(_, entry)| BatchRelease {
                document: entry.batch.document(),
                owner: entry.owner,
                source: entry.source.map(|source| source.id),
            })
            .collect()
    }

    pub fn retain_deferred(
        &mut self,
        keep: &std::collections::HashSet<ResourceKey>,
    ) -> Vec<BatchRelease> {
        let keys: Vec<_> = self
            .entries
            .keys()
            .filter(|key| !keep.contains(*key))
            .cloned()
            .collect();
        keys.into_iter()
            .filter_map(|key| self.entries.remove(&key))
            .map(|entry| BatchRelease {
                document: entry.batch.document(),
                owner: entry.owner,
                source: entry.source.map(|source| source.id),
            })
            .collect()
    }
    pub fn from_prepared(
        &mut self,
        scope: &PresentationScope,
        key: ResourceKey,
        document: SelectionDocumentId,
        prepared: PreparedCollectionRow,
    ) -> anyhow::Result<BatchedCollectionRow> {
        let mut sources = prepared.selection_sources.into_iter();
        let selection = match sources.next() {
            Some((actual, source)) => {
                anyhow::ensure!(actual == document, "conversation source document mismatch");
                Some(source)
            }
            None => None,
        };
        anyhow::ensure!(
            sources.next().is_none(),
            "conversation source has more than one selection source"
        );
        self.row(
            scope,
            key,
            document,
            editor_sources::ContentView {
                element: prepared.element,
                selection,
            },
        )
    }

    pub fn rebind(&mut self, app: &mut aqua::UiApp, scope: Option<PresentationScope>) {
        if self.scope != scope {
            self.clear(app);
            self.scope = scope;
        }
    }

    pub fn row(
        &mut self,
        scope: &PresentationScope,
        key: ResourceKey,
        document: SelectionDocumentId,
        view: editor_sources::ContentView,
    ) -> anyhow::Result<BatchedCollectionRow> {
        anyhow::ensure!(
            self.scope.as_ref() == Some(scope),
            "stale conversation presentation scope"
        );
        anyhow::ensure!(
            self.entries
                .get(&key)
                .is_none_or(|entry| entry.batch.document() == document),
            "conversation source document changed without releasing its owner"
        );
        let source = view.selection;
        let changed = self.entries.get(&key).is_none_or(|entry| {
            entry.batch.document() != document || !same_source(&entry.source, &source)
        });
        if changed {
            let (owner, revision) = self.entries.get(&key).map_or_else(
                || (SelectionBatchOwner::new(), 0),
                |entry| (entry.owner, entry.revision.wrapping_add(1)),
            );
            let sources: Arc<[SelectionSource]> =
                source.clone().into_iter().collect::<Vec<_>>().into();
            let batch = Arc::new(SelectionSourceBatch::new(
                document, owner, revision, sources,
            ));
            self.entries.insert(
                key.clone(),
                ResourceBatch {
                    owner,
                    revision,
                    source,
                    batch,
                },
            );
        }
        let entry = self
            .entries
            .get(&key)
            .ok_or_else(|| anyhow::anyhow!("conversation source batch is unavailable"))?;
        Ok(BatchedCollectionRow {
            element: view.element,
            batches: vec![Arc::clone(&entry.batch)],
        })
    }

    pub fn retain(
        &mut self,
        app: &mut aqua::UiApp,
        mut retained: impl FnMut(&ResourceKey) -> bool,
    ) {
        self.entries.retain(|key, entry| {
            if retained(key) {
                return true;
            }
            app.release_selection_batch_owner(entry.batch.document(), entry.owner);
            if let Some(source) = &entry.source {
                app.unregister_selection_source_in(entry.batch.document(), source.id);
            }
            false
        });
    }

    pub fn clear(&mut self, app: &mut aqua::UiApp) {
        self.retain(app, |_| false);
        self.scope = None;
    }
}

fn prepared(
    document: SelectionDocumentId,
    view: editor_sources::ContentView,
) -> PreparedCollectionRow {
    PreparedCollectionRow {
        element: view.element,
        selection_sources: view
            .selection
            .into_iter()
            .map(|source| (document, source))
            .collect(),
    }
}

pub fn stored_source(
    owners: &mut SourceOwners,
    current: &PresentationScope,
    core: &Core,
    scope: &PresentationScope,
    item: &Item,
    field: SourceField,
    theme: ArtisanTheme,
    document: SelectionDocumentId,
    source_id: SelectionSourceId,
    emit: Rc<dyn Fn(SourceAction)>,
) -> anyhow::Result<PreparedCollectionRow> {
    anyhow::ensure!(
        scope == current && core.host_if_instance(&scope.host, scope.instance).is_some(),
        "stale conversation presentation scope"
    );
    anyhow::ensure!(
        owners.open(core, &scope.host, scope.instance, item, field.clone()),
        "stored conversation source is unavailable"
    );
    let view = owners
        .view(&item.id, &field, theme, document, source_id, emit)
        .ok_or_else(|| anyhow::anyhow!("stored conversation source is unavailable"))?;
    Ok(prepared(document, view))
}

pub fn stored_questions(
    owner: &mut QuestionCollection,
    current: &PresentationScope,
    core: &Core,
    scope: &PresentationScope,
    item: &Item,
    source: &CollectionRef,
    theme: ArtisanTheme,
    document: SelectionDocumentId,
    source_id: SelectionSourceId,
    stored_detail: Option<Element>,
    emit: Rc<dyn Fn(QuestionCollectionAction)>,
) -> anyhow::Result<PreparedCollectionRow> {
    anyhow::ensure!(
        scope == current && core.host_if_instance(&scope.host, scope.instance).is_some(),
        "stale conversation presentation scope"
    );
    anyhow::ensure!(
        owner.open(core, &scope.host, scope.instance, &item.id, source),
        "stored questions are unavailable"
    );
    let view = owner
        .view(theme, document, source_id, stored_detail, emit)
        .map_err(anyhow::Error::msg)?
        .ok_or_else(|| anyhow::anyhow!("stored questions are unavailable"))?;
    Ok(prepared(document, view))
}
