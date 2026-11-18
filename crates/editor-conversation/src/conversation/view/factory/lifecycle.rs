use super::super::transcript::BatchedRowFactory;
use super::{AfterLayout, ConfigureRowsError, RealizeRowError, RowFactory};
use crate::conversation::fold::RowKey;
use aqua_ui::collection::BatchedCollectionRow;
use aqua_ui::collection_runtime::CollectionError;
use std::rc::Rc;
use std::time::Instant;

impl RowFactory {
    pub fn viewport_callback(
        &self,
    ) -> Result<Rc<dyn Fn(aqua_ui::collection_runtime::CollectionViewport)>, ConfigureRowsError>
    {
        let observations = self
            .observations
            .as_ref()
            .ok_or(ConfigureRowsError::NoObservations)?;
        Ok(observations.viewport_callback(self.active.clone()))
    }

    pub fn publish_projections(
        &self,
        conversation: &mut crate::conversation::controller::Conversation,
        core: &editor_core::Core,
    ) -> bool {
        if !self.live.get() {
            self.pending_projections.borrow_mut().clear();
            self.pending_live.borrow_mut().clear();
            return false;
        }
        for (binding, count) in std::mem::take(&mut *self.pending_live.borrow_mut()) {
            conversation.observe_live_preparation(core, &binding, count);
        }
        let pending = std::mem::take(&mut *self.pending_projections.borrow_mut());
        conversation.publish_projections(core, &self.scope, pending)
    }

    pub fn dispose(&self) {
        self.live.set(false);
        self.realized_owners.borrow_mut().clear();
        self.pending_projections.borrow_mut().clear();
        self.pending_live.borrow_mut().clear();
    }

    pub fn sync_after_layout(&self, app: &mut aqua::UiApp) -> bool {
        self.after_layout(app).rebuild
    }

    pub fn after_layout(&self, app: &mut aqua::UiApp) -> AfterLayout {
        if !self.live.get() || !self.row_cache.borrow().scope_is(&self.scope) {
            return AfterLayout::default();
        }
        if !self.work.motion.borrow().scope_is(&self.scope) {
            return AfterLayout::default();
        }
        let reveal = self.markdown.borrow().reveal_heatmaps(app);
        let stored_reveal = self
            .stored
            .as_ref()
            .is_some_and(|stored| stored.owner.borrow().sync_after_layout(app));
        let mounted: std::collections::HashSet<_> = app
            .tree()
            .traverse()
            .into_iter()
            .filter_map(|node| app.tree().get(node)?.key)
            .collect();
        let note = editor_core::diagnostics::rebuild_stats::note;
        let rebuild = note("al_reveal", reveal)
            | note("al_stored", stored_reveal)
            | note(
                "al_motion",
                self.realized_owners.borrow().iter().any(|(key, owner)| {
                    mounted.contains(&aqua_ui::tree::hash_key(aqua_ui::element::Key::from(
                        format!("{key:?}"),
                    ))) && owner
                        .upgrade()
                        .is_some_and(|owner| owner.motion.borrow().animating())
                }),
            );
        AfterLayout { rebuild }
    }

    pub fn sync_reveals(
        &self,
        app: &mut aqua::UiApp,
        conversation: &mut crate::conversation::controller::Conversation,
        core: &editor_core::Core,
    ) -> super::super::reveal::RevealSync {
        let Some(resources) = &self.reveals else {
            return super::super::reveal::RevealSync::default();
        };
        if !self.live.get()
            || !self.row_cache.borrow().scope_is(&self.scope)
            || !self.scope.current(conversation, core)
        {
            return super::super::reveal::RevealSync {
                reason: "factory_scope",
                pending: true,
                ..Default::default()
            };
        }
        let stored = self.stored.as_ref().map(|stored| stored.owner.borrow());
        let mut result = resources.owners.borrow_mut().sync(
            app,
            conversation,
            core,
            &mut self.markdown.borrow_mut(),
            stored.as_deref(),
        );
        if let Some(queue) = &self.admission {
            let frontier = queue.borrow().frontier_key(self.now);
            if conversation.reveal_frontier != frontier {
                conversation.reveal_frontier = frontier;
                result.changed = true;
                result.frontier_changed = true;
            }
        }
        result
    }

    pub fn callback(self: &Rc<Self>) -> BatchedRowFactory {
        let owner = Rc::clone(self);
        Rc::new(move |index, key| {
            owner.realize(index, key).map_err(|error| {
                let error = diagnostics::chain(&error);
                tracing::error!(?key, "conversation row failed: {error}");
                CollectionError::Validation(error)
            })
        })
    }

    pub fn layout_callback(self: &Rc<Self>) -> super::super::transcript::LayoutBatchedRowFactory {
        let owner = Rc::clone(self);
        Rc::new(move |index, key, layout| {
            owner
                .realize_with_layout(index, key, layout)
                .map_err(|error| {
                    let error = diagnostics::chain(&error);
                    tracing::error!(?key, "conversation row failed: {error}");
                    CollectionError::Validation(error)
                })
        })
    }

    pub fn realize(
        &self,
        index: usize,
        key: &RowKey,
    ) -> Result<BatchedCollectionRow, RealizeRowError> {
        let started = Instant::now();
        let realized = self.realize_inner(index, key, None);
        editor_core::diagnostics::open_timing::realized(started.elapsed());
        realized
    }

    fn realize_with_layout(
        &self,
        index: usize,
        key: &RowKey,
        layout: &aqua_ui::collection_runtime::CollectionLayout,
    ) -> Result<BatchedCollectionRow, RealizeRowError> {
        if !(layout.content_width.is_finite() && layout.content_width > 0.0) {
            return Err(RealizeRowError::Unmeasured);
        }
        let started = Instant::now();
        let realized = self.realize_inner(index, key, Some(layout.content_width));
        editor_core::diagnostics::open_timing::realized(started.elapsed());
        realized
    }
}
