use super::realize::RowContext;
use super::{RealizeRowError, RowFactory, Service, Staleness, StoredResources, media_width};
use crate::conversation::fold::{self, Slot};
use crate::conversation::presentation::{BodyKey, ScopedCodeAction};
use crate::conversation::stored::{StoredLeafServices, StoredLeafTarget};
use crate::conversation::view::reveal::MountedReveal;
use aqua_ui::collection::BatchedCollectionRow;
use editor_ui::kit::{Button, ButtonTone, TextRole};
use editor_ui::markdown::presenter::SelectionPlace;
use editor_ui::markdown::stored::PagePreparation;
use editor_ui::markdown::visual::{BodyContext, BodyTone};
use editor_ui::rich::actions::ResourceScope;
use editor_ui::text::slice::slice;
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

#[derive(Clone, Copy)]
pub(super) struct StoredBlock<'a> {
    pub(super) item: &'a artisan_api::Item,
    pub(super) slot: Slot,
    pub(super) block: usize,
    pub(super) reference: &'a artisan_api::ContentRef,
}

pub(super) fn plain_block(text: &str, block: usize) -> Option<&str> {
    fold::plain_ranges(text)
        .get(block)
        .map(|range| slice(text, range.clone()))
}

impl RowFactory {
    #[expect(
        clippy::too_many_lines,
        reason = "one stored Markdown row from source, page and leaf preparation through reveal and render"
    )]
    pub(super) fn stored_row(
        &self,
        cx: &RowContext<'_>,
        target: StoredBlock<'_>,
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
        let RowContext {
            row,
            owner: row_owner,
            reveal,
            content_width,
            ..
        } = *cx;
        let StoredBlock {
            item,
            slot,
            block,
            reference,
        } = target;
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
        let preparation = services.owner.borrow_mut().leaf(&key, &source, block);
        let leaf = match preparation {
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
        let resource_scope = ResourceScope {
            host: self.scope.host.to_string(),
            thread: self.scope.thread.clone(),
        };
        let (rendered, lease) = stored.render_leaf(
            StoredLeafTarget {
                key: &key,
                source: &source,
                ordinal: block,
                leaf,
            },
            BodyContext {
                theme: &self.theme,
                tone: BodyTone::Foreground,
                on_link: Some(&self.on_link),
                on_color: Some(&self.on_color),
                scope: Some(&resource_scope),
            },
            SelectionPlace {
                document,
                order: &order,
            },
            self.allocate.as_ref(),
            StoredLeafServices {
                invalidate: &self.invalidate,
                reveal: reveal.map(|(owner, _, _)| Rc::clone(owner)),
                media,
                code_emit,
                instance: self.scope.instance,
                reduced_motion: self.reduced_motion,
            },
        )?;
        *row_owner.stored_leaf.borrow_mut() = Some(lease);
        let prepared = if self.reveals.is_some() && reveal.is_some() {
            MountedReveal::record_stored(
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
        editor_ui::kit::ProblemNotice::new(
            format!("stored-page-problem-{}-{}-{offset}", key.item, key.slot),
            self.theme,
            &editor_core::Problem::detailed("Couldn't load this part of the message.", cause),
        )
        .action(retry)
        .build()
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one literal stored row from source preparation through the retained batch"
    )]
    pub(super) fn literal_row(
        &self,
        cx: &RowContext<'_>,
        target: StoredBlock<'_>,
        section: fold::PlainSection,
        batches: &mut Vec<Arc<aqua_ui::collection::SelectionSourceBatch>>,
    ) -> Result<aqua_ui::element::Element, RealizeRowError> {
        use crate::conversation::view::resources::ResourceKey;
        use aqua_ui::element::Text;
        use editor_ui::markdown::literal::LiteralPreparation;
        use editor_ui::markdown::stored::StoredPreparation;
        let RowContext {
            row,
            owner: row_owner,
            ..
        } = *cx;
        let StoredBlock {
            item,
            slot,
            block,
            reference,
        } = target;
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
        let preparation = services
            .owner
            .borrow_mut()
            .literal_leaf(&key, &source, block);
        let leaf = match preparation {
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
