use crate::conversation::content::SourceField;
use crate::conversation::presentation::PresentationScope;
use aqua_text::document::SelectionSource;
use aqua_text::document::{SelectionDocumentId, SelectionSourceId};
use aqua_ui::collection::{BatchedCollectionRow, SelectionBatchOwner, SelectionSourceBatch};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ResourceKey {
    Source(artisan_api::ItemId, SourceField),
    Questions(artisan_api::ItemId),
    QuestionDetail(artisan_api::ItemId),
}

#[derive(Debug, thiserror::Error)]
pub enum BatchRowError {
    #[error("the conversation presentation belongs to another thread or host")]
    StaleScope,
    #[error("the selection document of {key:?} changed without releasing its owner")]
    DocumentChanged { key: ResourceKey },
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
    pub(crate) fn apply(self, app: &mut aqua::UiApp) {
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
    pub(crate) fn set_scope(&mut self, scope: &PresentationScope) -> bool {
        if self.scope.as_ref().is_some_and(|current| current != scope) {
            return false;
        }
        self.scope = Some(scope.clone());
        true
    }

    pub(crate) fn take_releases(&mut self) -> Vec<BatchRelease> {
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

    pub(crate) fn retain_deferred(
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
    ) -> Result<BatchedCollectionRow, BatchRowError> {
        if self.scope.as_ref() != Some(scope) {
            return Err(BatchRowError::StaleScope);
        }
        if self
            .entries
            .get(&key)
            .is_some_and(|entry| entry.batch.document() != document)
        {
            return Err(BatchRowError::DocumentChanged { key });
        }
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
            .expect("invariant: a source batch is recorded before its row is returned");
        Ok(BatchedCollectionRow {
            element: view.element,
            batches: vec![Arc::clone(&entry.batch)],
        })
    }

    fn retain(&mut self, app: &mut aqua::UiApp, mut retained: impl FnMut(&ResourceKey) -> bool) {
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
