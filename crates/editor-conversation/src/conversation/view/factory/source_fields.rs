use super::{Part, RealizeRowError, RowFactory, Service, Staleness};
use crate::conversation::fold::{self, Slot};
use aqua_ui::collection::BatchedCollectionRow;
use artisan_api::TextContent;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

pub(super) fn answer_text(text: &str) -> &str {
    text.strip_prefix("You answered: ").unwrap_or(text)
}

impl RowFactory {
    pub(super) fn retained_row(
        &self,
        owner: &super::super::row_cache::RowOwner,
        key: super::super::resources::ResourceKey,
        document: aqua_text::document::SelectionDocumentId,
        view: editor_sources::ContentView,
        stale: Staleness,
    ) -> Result<BatchedCollectionRow, RealizeRowError> {
        let mut retained = owner.batches.borrow_mut();
        if !retained.set_scope(&self.scope) {
            return Err(RealizeRowError::Stale(stale));
        }
        Ok(retained.row(&self.scope, key, document, view)?)
    }

    pub(super) fn slot_source_field(
        &self,
        owner: &Rc<super::super::row_cache::RowOwner>,
        item: &artisan_api::Item,
        slot: Slot,
        batches: &mut Vec<Arc<aqua_ui::collection::SelectionSourceBatch>>,
    ) -> Result<aqua_ui::element::Element, RealizeRowError> {
        let collected = RefCell::new(Vec::new());
        let used = RefCell::new(std::collections::HashSet::new());
        let full = self.source_field(
            owner,
            item,
            crate::conversation::content::SourceField::of_slot(slot),
            &collected,
            &used,
        )?;
        owner.retain_batches(&used.into_inner());
        batches.extend(collected.into_inner());
        Ok(full)
    }

    pub(super) fn source_field(
        &self,
        owner: &Rc<super::super::row_cache::RowOwner>,
        item: &artisan_api::Item,
        field: crate::conversation::content::SourceField,
        batches: &RefCell<Vec<std::sync::Arc<aqua_ui::collection::SelectionSourceBatch>>>,
        used: &RefCell<std::collections::HashSet<super::super::resources::ResourceKey>>,
    ) -> Result<aqua_ui::element::Element, RealizeRowError> {
        use crate::conversation::view::resources::ResourceKey;
        use crate::conversation::view::row_sources::{RowSourceAction, RowSourceActionKind};
        use aqua_ui::element::Text;
        let theme = match &field {
            crate::conversation::content::SourceField::Question(_) => {
                super::super::request::question_theme(item, self.theme)
            }
            _ => self.theme,
        };
        let source = field
            .text(item)
            .ok_or(RealizeRowError::Unavailable(Part::SourceField))?;
        if let TextContent::Inline(text) = source {
            if field == crate::conversation::content::SourceField::AnswerText {
                let services = self
                    .source_resources
                    .as_ref()
                    .ok_or(RealizeRowError::Missing(Service::Sources))?;
                let key = ResourceKey::Source(item.id.clone(), field);
                used.borrow_mut().insert(key.clone());
                let id = *owner
                    .source_ids
                    .borrow_mut()
                    .entry(key.clone())
                    .or_insert_with(|| (self.allocate)());
                let text = answer_text(text).to_owned();
                let source = aqua_text::document::SelectionSource {
                    id,
                    order: fold::selection_order(
                        self.scope.agent.is_some(),
                        item.position,
                        Slot::Answer,
                    ),
                    text: std::sync::Arc::from(text.as_str()),
                    separator: std::sync::Arc::from("\n\n"),
                    copies: Vec::new(),
                };
                let element = Text::new(text.clone())
                    .wrap(true)
                    .style(
                        aqua_ui::style::Style::new()
                            .width(aqua_ui::style::Length::Pct(100.0))
                            .min_width(aqua_ui::style::Length::Pct(0.0)),
                    )
                    .gpui_source_color(theme.colors.text.to_reference_text_color())
                    .selection_source_in(
                        services.document,
                        editor_sources::source_slice(id, 0..text.len()),
                    )
                    .build();
                let view = editor_sources::ContentView {
                    element,
                    selection: Some(source),
                };
                let row =
                    self.retained_row(owner, key, services.document, view, Staleness::AnswerBatch)?;
                batches.borrow_mut().extend(row.batches);
                return Ok(row.element);
            }
            let mut element = super::super::request::field_role(
                field == crate::conversation::content::SourceField::ApprovalCommand,
            )
            .text(&theme, text.clone());
            if matches!(
                field,
                crate::conversation::content::SourceField::Question(_)
            ) {
                element = element.wrap(true).style(
                    aqua_ui::style::Style::new()
                        .width(aqua_ui::style::Length::Pct(100.0))
                        .min_width(aqua_ui::style::Length::Pct(0.0)),
                );
            }
            return Ok(element.build());
        }
        let services = self
            .source_resources
            .as_ref()
            .ok_or(RealizeRowError::Missing(Service::Sources))?;
        if !services.provider.is_available() {
            return Err(RealizeRowError::Missing(Service::SourceProvider));
        }
        let key = ResourceKey::Source(item.id.clone(), field.clone());
        used.borrow_mut().insert(key.clone());
        let id = *owner
            .source_ids
            .borrow_mut()
            .entry(key.clone())
            .or_insert_with(|| (self.allocate)());
        if !owner
            .sources
            .borrow_mut()
            .open_provider(&services.provider, item, field.clone())
        {
            return Err(RealizeRowError::Unavailable(Part::Source));
        }
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
                action: RowSourceActionKind::Content(action),
            })
        });
        let view = owner
            .sources
            .borrow()
            .view(&item.id, &field, theme, services.document, id, callback)
            .ok_or(RealizeRowError::Unavailable(Part::SourceView))?;
        let row = self.retained_row(owner, key, services.document, view, Staleness::SourceBatch)?;
        batches.borrow_mut().extend(row.batches);
        Ok(row.element)
    }
}
