use super::{
    RealizeRowError, RealizedRow, RowFactory, Service, Staleness, StoredResources, media_width,
};
use crate::conversation::fold::{self, Slot};
use crate::conversation::presentation::{BodyKey, ScopedCodeAction};
use aqua_ui::collection::BatchedCollectionRow;
use editor_ui::kit::{Button, ButtonTone, Space, Spaced, TextRole};
use editor_ui::markdown::stored::PagePreparation;
use editor_ui::markdown::visual::BodyTone;
use editor_ui::rich::actions::ResourceScope;
use std::cell::Cell;
use std::rc::Rc;
use std::rc::Weak;
use std::sync::Arc;

fn pending_above(
    pending: aqua_ui::element::Element,
    full: aqua_ui::element::Element,
) -> aqua_ui::element::Element {
    aqua_ui::element::Box::new()
        .layout("column")
        .child(pending)
        .child(full)
        .build()
}

pub(super) fn plain_block(text: &str, block: usize) -> Option<&str> {
    fold::plain_ranges(text)
        .get(block)
        .map(|range| &text[range.clone()])
}

impl RowFactory {
    pub(super) fn stored_row(
        &self,
        row: &RealizedRow,
        row_owner: &Rc<super::super::row_cache::RowOwner>,
        item: &artisan_api::Item,
        slot: Slot,
        block: usize,
        reference: &artisan_api::ContentRef,
        reveal: Option<&(
            Rc<editor_ui::markdown::reveal::PreparedReveal>,
            bool,
            Rc<Cell<bool>>,
        )>,
        content_width: Option<f32>,
        batches: &mut Vec<Arc<aqua_ui::collection::SelectionSourceBatch>>,
    ) -> Result<
        (
            aqua_ui::element::Element,
            Option<crate::conversation::stored::StoredProof>,
        ),
        RealizeRowError,
    > {
        use aqua_ui::element::Text;
        use editor_ui::markdown::stored::{LeafPreparation, StoredPreparation};
        let services = self
            .stored
            .as_ref()
            .ok_or(RealizeRowError::Missing(Service::Stored))?;
        let key = BodyKey {
            item: item.id.clone(),
            slot,
        };
        let version = self
            .projection_versions
            .get(&key)
            .copied()
            .unwrap_or(row.model.version);
        let pending = |text: &str| {
            self.unsettled.set(true);
            Text::new(text.to_owned()).build()
        };
        let state = services.owner.borrow_mut().ensure(
            &self.scope,
            &services.provider,
            key.clone(),
            reference.clone(),
            version,
        )?;
        let source = match state {
            StoredPreparation::Pending => return Ok((pending("Loading formatted content…"), None)),
            StoredPreparation::Failed(error) => {
                let full = self.slot_source_field(row_owner, item, slot, batches)?;
                return Ok((pending_above(pending(&error), full), None));
            }
            StoredPreparation::Ready(source) => source,
        };
        if block >= source.block_count() {
            return Ok((pending("Content layout is updating…"), None));
        }
        match self.stored_page(services, &key, &source, block, version) {
            PagePreparation::Ready(_) => {}
            PagePreparation::Pending => return Ok((pending("Loading content…"), None)),
            PagePreparation::Failed { offset, cause } => {
                return Ok((self.page_failure(&key, offset, &cause), None));
            }
        }
        let leaf = match services.owner.borrow_mut().leaf(&key, &source, block) {
            LeafPreparation::Pending => return Ok((pending("Loading content…"), None)),
            LeafPreparation::Failed(error) => {
                let full = self.slot_source_field(row_owner, item, slot, batches)?;
                return Ok((pending_above(pending(&error), full), None));
            }
            LeafPreparation::Ready(leaf) => leaf,
        };
        let document = self
            .source_resources
            .as_ref()
            .ok_or(RealizeRowError::Missing(Service::Sources))?
            .document;
        if let (Some((owner, fresh, ready)), Some(resources)) = (reveal, &self.reveals) {
            ready.set(false);
            owner.begin_frame(
                self.now,
                false,
                *fresh,
                self.reduced_motion,
                resources.swift,
            );
            owner.set_paused(false);
            owner.set_rushed(
                self.admission
                    .as_ref()
                    .is_some_and(|queue| queue.borrow().rushed(item.position)),
            );
        }
        let mut order = fold::selection_order(self.scope.agent.is_some(), item.position, slot);
        order.push(block as u64);
        let media = if let (Some(width), Some(factory)) = (content_width, &self.media) {
            let width = media_width(
                super::super::reading_width(width),
                slot,
                leaf.context().lists.len(),
            );
            if width.is_nan() || width <= 0.0 {
                return Err(RealizeRowError::Unmeasured);
            }
            factory(&key, width)
        } else {
            None
        };
        let proof_leaf = leaf.proof().clone();
        let mut stored = services.owner.borrow_mut();
        let proof = stored
            .proof(&self.scope, &key, &source, block, item.seq, &proof_leaf)
            .ok_or(RealizeRowError::Stale(Staleness::StoredProof))?;
        let emit = Rc::clone(&self.on_code);
        let code_scope = self.scope.clone();
        let code_key = key.clone();
        let code_proof = proof.clone();
        let code_owner = Rc::downgrade(&services.owner);
        let code_emit = Rc::new(move |action| {
            emit(ScopedCodeAction::stored(
                code_scope.clone(),
                code_key.clone(),
                Weak::clone(&code_owner),
                code_proof.clone(),
                action,
            ));
        });
        let (rendered, lease) = stored.render_leaf(
            &key,
            &source,
            block,
            leaf,
            document,
            &order,
            &self.theme,
            BodyTone::Foreground,
            self.allocate.as_ref(),
            Rc::clone(&self.invalidate),
            Rc::clone(&self.on_link),
            Rc::clone(&self.on_color),
            ResourceScope {
                host: self.scope.host.to_string(),
                thread: self.scope.thread.clone(),
            },
            reveal.as_ref().map(|(owner, _, _)| Rc::clone(owner)),
            media,
            code_emit,
            self.scope.instance,
            self.reduced_motion,
        )?;
        *row_owner.stored_leaf.borrow_mut() = Some(lease);
        let prepared = if let (Some(resources), Some(_)) = (&self.reveals, reveal) {
            resources.owners.borrow_mut().record_stored(
                row_owner,
                row.model.version,
                block,
                proof.clone(),
                rendered,
            )
        } else {
            BatchedCollectionRow {
                element: rendered.element,
                batches: vec![rendered.selection_batch],
            }
        };
        batches.extend(prepared.batches);
        Ok((prepared.element, Some(proof)))
    }

    fn stored_page(
        &self,
        services: &StoredResources,
        key: &BodyKey,
        source: &editor_ui::markdown::stored::StoredSource,
        block: usize,
        version: u64,
    ) -> PagePreparation {
        let (page, moved) = services.owner.borrow_mut().page(key, source, block);
        if let Some(leaves) = moved {
            self.pending_projections.borrow_mut().push((
                key.clone(),
                crate::conversation::projection::BodyProjection {
                    version,
                    revision: source.revision(),
                    leaves,
                },
            ));
        }
        page
    }

    fn page_failure(&self, key: &BodyKey, offset: u64, cause: &str) -> aqua_ui::element::Element {
        let dispatch = Rc::clone(&self.dispatch);
        let active = Rc::clone(&self.active);
        let body = key.clone();
        let retry = Button::text(
            format!("retry-stored-page-{}-{}-{offset}", key.item, key.slot),
            self.theme,
            ButtonTone::Neutral,
            "Try again",
        )
        .on_activate(move |_| {
            if active.get() {
                dispatch(crate::conversation::adapter::Action::RetryStoredPage {
                    body: body.clone(),
                    offset,
                });
            }
        })
        .build();
        let text = TextRole::Label
            .text(
                &self.theme,
                format!("Couldn't load this part of the message: {cause}"),
            )
            .gpui_source_color(self.theme.colors.text_muted.to_reference_text_color())
            .build();
        aqua_ui::element::Box::new()
            .layout("column")
            .width(aqua_ui::style::Length::Pct(100.0))
            .spaced(Space::S8)
            .child(text)
            .child(retry)
            .build()
    }

    pub(super) fn literal_row(
        &self,
        row_owner: &Rc<super::super::row_cache::RowOwner>,
        item: &artisan_api::Item,
        slot: Slot,
        section: fold::PlainSection,
        block: usize,
        reference: &artisan_api::ContentRef,
        version: u64,
        batches: &mut Vec<Arc<aqua_ui::collection::SelectionSourceBatch>>,
    ) -> Result<aqua_ui::element::Element, RealizeRowError> {
        use crate::conversation::view::resources::ResourceKey;
        use aqua_ui::element::Text;
        use editor_ui::markdown::literal::LiteralPreparation;
        use editor_ui::markdown::stored::StoredPreparation;
        let services = self
            .stored
            .as_ref()
            .ok_or(RealizeRowError::Missing(Service::Stored))?;
        let key = BodyKey {
            item: item.id.clone(),
            slot,
        };
        let version = self
            .projection_versions
            .get(&key)
            .copied()
            .unwrap_or(version);
        let pending = |text: &str| {
            self.unsettled.set(true);
            Text::new(text.to_owned()).build()
        };
        let state = services.owner.borrow_mut().ensure(
            &self.scope,
            &services.provider,
            key.clone(),
            reference.clone(),
            version,
        )?;
        let source = match state {
            StoredPreparation::Pending => return Ok(pending("Loading content…")),
            StoredPreparation::Failed(error) => return Ok(pending(&error)),
            StoredPreparation::Ready(source) => source,
        };
        if block >= source.block_count() {
            return Ok(pending("Content layout is updating…"));
        }
        match self.stored_page(services, &key, &source, block, version) {
            PagePreparation::Ready(_) => {}
            PagePreparation::Pending => return Ok(pending("Loading content…")),
            PagePreparation::Failed { offset, cause } => {
                return Ok(self.page_failure(&key, offset, &cause));
            }
        }
        let leaf = match services
            .owner
            .borrow_mut()
            .literal_leaf(&key, &source, block)
        {
            LiteralPreparation::Pending => return Ok(pending("Loading content…")),
            LiteralPreparation::Failed(error) => {
                let full = self.slot_source_field(row_owner, item, slot, batches)?;
                return Ok(pending_above(pending(&error), full));
            }
            LiteralPreparation::Ready(leaf) => leaf,
        };
        if leaf.needs_native_window() {
            let full = self.slot_source_field(row_owner, item, slot, batches)?;
            return Ok(full);
        }
        let document = self
            .source_resources
            .as_ref()
            .ok_or(RealizeRowError::Missing(Service::Sources))?
            .document;
        let field = crate::conversation::content::SourceField::of_slot(slot);
        let resource = ResourceKey::Source(item.id.clone(), field);
        let id = *row_owner
            .source_ids
            .borrow_mut()
            .entry(resource.clone())
            .or_insert_with(|| (self.allocate)());
        let mut order = fold::selection_order(self.scope.agent.is_some(), item.position, slot);
        order.push(section.order());
        order.push(block as u64);
        let separator = if block == 0 { "\n\n" } else { "" };
        let selection = leaf.selection(id, order, separator);
        let visible = leaf.visible_text().to_owned();
        let color = match section {
            fold::PlainSection::ImageCaption => self.theme.colors.text_muted,
            fold::PlainSection::Error => self.theme.colors.text,
        };
        let element = TextRole::Label
            .text(&self.theme, visible.clone())
            .gpui_source_color(color.to_reference_text_color())
            .selection_source_in(document, editor_sources::source_slice(id, 0..visible.len()))
            .build();
        let view = editor_sources::ContentView {
            element,
            selection: Some(selection.source().clone()),
        };
        let prepared =
            self.retained_row(row_owner, resource, document, view, Staleness::LiteralBatch)?;
        batches.extend(prepared.batches);
        Ok(prepared.element)
    }
}
