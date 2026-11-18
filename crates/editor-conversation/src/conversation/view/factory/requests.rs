use super::{Part, RealizeRowError, RowFactory, Service, Staleness, truncate};
use std::cell::RefCell;
use std::rc::Rc;

impl RowFactory {
    pub(super) fn approval_card(
        &self,
        owner: &Rc<super::super::row_cache::RowOwner>,
        item: &artisan_api::Item,
        answer: Option<&crate::conversation::thread::AnswerState>,
        dispatch: super::super::Dispatch,
        batches: &mut Vec<std::sync::Arc<aqua_ui::collection::SelectionSourceBatch>>,
    ) -> Result<Option<aqua_ui::element::Element>, RealizeRowError> {
        let collected = RefCell::new(Vec::new());
        let used = RefCell::new(std::collections::HashSet::new());
        let result = super::super::request::approval(
            item,
            answer.is_some_and(crate::conversation::thread::AnswerState::sending),
            answer.and_then(crate::conversation::thread::AnswerState::error),
            self.theme,
            |field| self.source_field(owner, item, field, &collected, &used),
            dispatch,
        )?;
        owner.retain_batches(&used.into_inner());
        batches.extend(collected.into_inner());
        Ok(result)
    }

    pub(super) fn activity_row(
        &self,
        owner: &Rc<super::super::row_cache::RowOwner>,
        item: &artisan_api::Item,
        expanded: bool,
        names: &std::collections::HashMap<artisan_api::ItemId, String>,
        dispatch: super::super::Dispatch,
        batches: &mut Vec<std::sync::Arc<aqua_ui::collection::SelectionSourceBatch>>,
    ) -> Result<Option<aqua_ui::element::Element>, RealizeRowError> {
        let collected = RefCell::new(Vec::new());
        let used = RefCell::new(std::collections::HashSet::new());
        let now = self.now;
        let disclosed = owner.motion.borrow_mut().disclosed(
            format!("details-{}", item.id),
            expanded,
            self.reduced_motion,
            now,
        );
        let invalidate = self.invalidate.clone();
        let result = super::super::work::activity_row(
            item,
            expanded,
            names,
            self.theme,
            |text, color, mono| truncate(text, color, mono, self.theme),
            dispatch,
            || {
                self.source_field(
                    owner,
                    item,
                    crate::conversation::content::SourceField::Output,
                    &collected,
                    &used,
                )
            },
            move |details| match disclosed {
                Some(frame) => frame.wrap(details, 0.0, invalidate),
                None => details,
            },
        )?;
        owner.retain_batches(&used.into_inner());
        batches.extend(collected.into_inner());
        Ok(result)
    }

    pub(super) fn notice_row(
        &self,
        owner: &Rc<super::super::row_cache::RowOwner>,
        item: &artisan_api::Item,
        batches: &mut Vec<std::sync::Arc<aqua_ui::collection::SelectionSourceBatch>>,
    ) -> Result<Option<aqua_ui::element::Element>, RealizeRowError> {
        let collected = RefCell::new(Vec::new());
        let used = RefCell::new(std::collections::HashSet::new());
        let result = super::super::notice::render(
            item,
            self.theme,
            |text, color| truncate(text, color, false, self.theme),
            |field| self.source_field(owner, item, field, &collected, &used),
        )?;
        owner.retain_batches(&used.into_inner());
        batches.extend(collected.into_inner());
        Ok(result)
    }

    pub(super) fn question_card(
        &self,
        owner: &Rc<super::super::row_cache::RowOwner>,
        item: &artisan_api::Item,
        batches: &mut Vec<std::sync::Arc<aqua_ui::collection::SelectionSourceBatch>>,
    ) -> Result<Option<aqua_ui::element::Element>, RealizeRowError> {
        use crate::conversation::content::SourceField;
        let collected = RefCell::new(Vec::new());
        let used = RefCell::new(std::collections::HashSet::new());
        let result = super::super::request::question(
            item,
            self.theme,
            |field| self.source_field(owner, item, field, &collected, &used),
            |source| self.question_collection(owner, item, source, &collected, &used),
            |item, questions| {
                super::super::request::answered(
                    questions,
                    self.theme,
                    |field| self.source_field(owner, item, field, &collected, &used),
                    |source| self.question_collection(owner, item, source, &collected, &used),
                    || self.source_field(owner, item, SourceField::AnswerText, &collected, &used),
                )
            },
        )?;
        owner.retain_batches(&used.into_inner());
        batches.extend(collected.into_inner());
        Ok(result)
    }

    pub(super) fn question_collection(
        &self,
        owner: &Rc<super::super::row_cache::RowOwner>,
        item: &artisan_api::Item,
        source: &artisan_api::CollectionRef,
        batches: &RefCell<Vec<std::sync::Arc<aqua_ui::collection::SelectionSourceBatch>>>,
        used: &RefCell<std::collections::HashSet<super::super::resources::ResourceKey>>,
    ) -> Result<aqua_ui::element::Element, RealizeRowError> {
        use crate::conversation::view::resources::ResourceKey;
        use crate::conversation::view::row_sources::{RowSourceAction, RowSourceActionKind};
        let theme = super::super::request::question_theme(item, self.theme);
        let services = self
            .source_resources
            .as_ref()
            .ok_or(RealizeRowError::Missing(Service::Sources))?;
        if !services.provider.is_available() {
            return Err(RealizeRowError::Missing(Service::SourceProvider));
        }
        let key = ResourceKey::Questions(item.id.clone());
        used.borrow_mut().insert(key.clone());
        let id = *owner
            .source_ids
            .borrow_mut()
            .entry(key.clone())
            .or_insert_with(|| (self.allocate)());
        if !owner
            .questions
            .borrow_mut()
            .open_provider(&services.provider, &item.id, source)
        {
            return Err(RealizeRowError::Unavailable(Part::QuestionCollection));
        }
        let stored_detail = if let Some(browser) = owner.questions.borrow().browser() {
            let browser = browser.borrow();
            if let Some(detail) = &browser.detail {
                let detail_key = ResourceKey::QuestionDetail(item.id.clone());
                used.borrow_mut().insert(detail_key.clone());
                let detail_id = *owner
                    .source_ids
                    .borrow_mut()
                    .entry(detail_key.clone())
                    .or_insert_with(|| (self.allocate)());
                let detail_source = detail.window().source().clone();
                let identity = detail.identity();
                let collection = source.clone();
                let scope = self.scope.clone();
                let provider = services.provider.clone();
                let emit = services.emit.clone();
                let weak = Rc::downgrade(owner);
                let item_id = item.id.clone();
                let seq = item.seq;
                let callback = Rc::new(move |action| {
                    emit(RowSourceAction {
                        item: item_id.clone(),
                        seq,
                        scope: scope.clone(),
                        provider: provider.clone(),
                        owner: weak.clone(),
                        action: RowSourceActionKind::QuestionDetail {
                            collection: collection.clone(),
                            source: detail_source.clone(),
                            identity,
                            action,
                        },
                    })
                });
                let view = editor_sources::content_view(
                    detail,
                    theme,
                    services.document,
                    detail_id,
                    callback,
                );
                let row = self.retained_row(
                    owner,
                    detail_key,
                    services.document,
                    view,
                    Staleness::DetailBatch,
                )?;
                batches.borrow_mut().extend(row.batches);
                Some(row.element)
            } else {
                None
            }
        } else {
            None
        };
        let scope = self.scope.clone();
        let provider = services.provider.clone();
        let emit = services.emit.clone();
        let weak = Rc::downgrade(owner);
        let item_id = item.id.clone();
        let seq = item.seq;
        let callback = Rc::new(move |action| {
            emit(RowSourceAction {
                item: item_id.clone(),
                seq,
                scope: scope.clone(),
                provider: provider.clone(),
                owner: weak.clone(),
                action: RowSourceActionKind::Questions(action),
            })
        });
        let view = owner
            .questions
            .borrow()
            .view(theme, services.document, id, stored_detail, callback)
            .ok_or(RealizeRowError::Unavailable(Part::QuestionView))?;
        let row = self.retained_row(
            owner,
            key,
            services.document,
            view,
            Staleness::QuestionBatch,
        )?;
        batches.borrow_mut().extend(row.batches);
        Ok(row.element)
    }
}
