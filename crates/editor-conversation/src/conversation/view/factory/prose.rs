use super::realize::{RowContext, RowOutput};
use super::{
    Part, RealizeRowError, RowFactory, Staleness, bubble_sender, media_width, user_bubble,
};
use crate::conversation::fold::{self, Slot};
use crate::conversation::live::LiveBinding;
use crate::conversation::presentation::{
    BodyKey, BodyText, MarkdownOwners, MarkdownRender, PreparedProjectedRow,
};
use crate::conversation::view::reveal::{MountedReveal, RevealHandle};
use aqua_ui::element::Element;
use artisan_api::{Item, ItemId, Status};
use editor_ui::markdown::presenter::HeatmapServices;
use editor_ui::markdown::visual::BodyTone;
use editor_ui::rich::actions::ResourceScope;
use std::rc::Rc;
use std::sync::Arc;

struct ProseBody<'a> {
    key: BodyKey,
    item: &'a Item,
    source: BodyText<'a>,
    live: Option<LiveBinding>,
    block: usize,
    sealed: bool,
}

enum Preparation {
    Empty,
    Ready { count: usize, revision: u64 },
    Failed(Element),
}

impl RowFactory {
    pub(super) fn prose_row(
        &self,
        cx: &RowContext<'_>,
        out: &mut RowOutput,
        id: &ItemId,
        slot: Slot,
        block: usize,
    ) -> Result<Option<Element>, RealizeRowError> {
        let RowContext { row, reveal, .. } = *cx;
        let item = row
            .model
            .items
            .get(id)
            .ok_or(RealizeRowError::Unavailable(Part::Item))?;
        let running = item.status == Status::Running && slot == Slot::Prose;
        let key = BodyKey {
            item: id.clone(),
            slot,
        };
        let shape_count = self.shape_counts.get(&key).copied();
        let sealed = shape_count.is_some_and(|count| block + 1 < count);
        let fresh =
            slot == Slot::Prose && row.model.ui.get(id).is_some_and(|ui| ui.fresh(self.now));
        if !(reveal.is_some() || (!running && !fresh)) {
            return Err(RealizeRowError::Unavailable(Part::Reveal));
        }
        let saved = match slot.inline_text(item) {
            Some(source) => Some(source),
            None if running => None,
            None => return Err(RealizeRowError::Unavailable(Part::StoredText)),
        };
        let live = self.prose_live(cx, id, running, saved)?;
        self.content_keyed
            .set(shape_count.is_some() && (live.is_none() || sealed));
        let live_source = live.as_ref().map(|binding| self.live_text(&binding.text));
        let source = live_source
            .as_ref()
            .map(BodyText::Shared)
            .or_else(|| saved.map(BodyText::Borrowed))
            .ok_or(RealizeRowError::Unavailable(Part::LiveSource))?;
        self.begin_prose_reveal(reveal, item, running && !sealed);
        let body = ProseBody {
            key,
            item,
            source,
            live,
            block,
            sealed,
        };
        self.render_prose(cx, out, &body)
    }

    fn render_prose(
        &self,
        cx: &RowContext<'_>,
        out: &mut RowOutput,
        body: &ProseBody<'_>,
    ) -> Result<Option<Element>, RealizeRowError> {
        let RowContext {
            row,
            owner: row_owner,
            reveal,
            content_width,
            ..
        } = *cx;
        let block = body.block;
        let mut owners = self.markdown.borrow_mut();
        let mut engine = self.engine.borrow_mut();
        let engine = match &mut *engine {
            Some(engine) => engine,
            absent @ None => absent.insert(editor_ui::markdown::MarkdownEngine::new()?),
        };
        let shaped = self.shape_counts.contains_key(&body.key);
        let (count, revision) = match self.prepare_prose(&mut owners, engine, body, reveal)? {
            Preparation::Empty => {
                drop(owners);
                return self.empty_prose(cx, body, shaped);
            }
            Preparation::Ready { count, revision } => (count, revision),
            Preparation::Failed(element) => return Ok(Some(element)),
        };
        if (if shaped {
            block + 1 == count
        } else {
            block == 0
        }) && let Some(binding) = &body.live
        {
            self.pending_live
                .borrow_mut()
                .push((binding.clone(), count));
        }
        if let Some(width) = content_width {
            self.bind_prose_media(&owners, &body.key, block, width)?;
        }
        let prepared = self.project_prose(&mut owners, body, reveal)?;
        let entrance = if prepared.geometry.block_entrance {
            reveal.and_then(|(owner, _, _)| {
                owner
                    .block_entrance(block)
                    .map(|(alpha, rise)| (alpha, rise, owner.block_panel_height(block)))
            })
        } else {
            None
        };
        if block == 0 && !shaped {
            self.queue_projection(cx, body.item, body.key.slot, revision, count);
        }
        let prepared = if self.reveals.is_some() && reveal.is_some() {
            MountedReveal::record(
                row_owner,
                row.model.version,
                block,
                prepared,
                body.live.clone().filter(|_| !body.sealed),
            )
        } else {
            prepared.row
        };
        out.batches = prepared.batches;
        let element = self.reveal_frame(prepared.element, entrance, reveal);
        Ok(Some(self.prose_frame(cx, body, element, count)))
    }

    fn empty_prose(
        &self,
        cx: &RowContext<'_>,
        body: &ProseBody<'_>,
        shaped: bool,
    ) -> Result<Option<Element>, RealizeRowError> {
        if !shaped
            && body.block == 0
            && let Some(binding) = &body.live
        {
            self.pending_live.borrow_mut().push((binding.clone(), 0));
        }
        if cx.reveal.is_some() {
            MountedReveal::clear(cx.owner);
        }
        if body.key.slot.is_bubble() && body.block == 0 {
            self.attachments_row(cx, &body.key.item)
        } else {
            Ok(None)
        }
    }

    fn prose_frame(
        &self,
        cx: &RowContext<'_>,
        body: &ProseBody<'_>,
        element: Element,
        count: usize,
    ) -> Element {
        let RowContext {
            row,
            key,
            owner: row_owner,
            ..
        } = *cx;
        let (item, block) = (body.item, body.block);
        let preview_notice = (block + 1 == count)
            .then(|| body.live.as_ref().and_then(LiveBinding::preview_notice))
            .flatten();
        if body.key.slot.is_bubble() {
            let fresh = row
                .model
                .ui
                .get(&item.id)
                .is_some_and(|ui| ui.fresh(self.now));
            user_bubble(
                element,
                block,
                count,
                body.source.as_str().is_empty(),
                if block == 0 {
                    self.user_attachments(row_owner, item)
                } else {
                    None
                },
                bubble_sender(item, row.model.author),
                self.bubble_sticker(row_owner, key, fresh),
                item.status == Status::Cancelled,
                &self.theme,
            )
        } else if let Some(notice) = preview_notice {
            aqua_ui::element::Box::new()
                .width(aqua_ui::style::Length::Pct(100.0))
                .layout("column")
                .child(element)
                .child(editor_ui::page::note(&self.theme, notice).build())
                .build()
        } else {
            element
        }
    }

    fn prose_live(
        &self,
        cx: &RowContext<'_>,
        id: &ItemId,
        running: bool,
        saved: Option<&str>,
    ) -> Result<Option<LiveBinding>, RealizeRowError> {
        let row = cx.row;
        if !running {
            return Ok(None);
        }
        let bound = self
            .reveals
            .as_ref()
            .and_then(|resources| resources.live.get(id));
        Ok(match (bound, saved) {
            (Some(binding), _) => {
                let target = artisan_api::LiveTarget::Text {
                    item_id: id.clone(),
                };
                if !row
                    .model
                    .live
                    .get(&target)
                    .is_some_and(|source| Arc::ptr_eq(source, &binding.text))
                {
                    return Err(RealizeRowError::Stale(Staleness::LiveSnapshot));
                }
                Some(binding.clone())
            }
            (None, Some(_)) => None,
            (None, None) => return Err(RealizeRowError::Unavailable(Part::LiveSource)),
        })
    }

    fn begin_prose_reveal(&self, reveal: Option<&RevealHandle>, item: &Item, streaming: bool) {
        if let (Some((owner, is_fresh, ready)), Some(resources)) = (reveal, &self.reveals) {
            ready.set(false);
            owner.begin_frame(
                self.now,
                streaming,
                *is_fresh,
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
    }

    fn prepare_prose(
        &self,
        owners: &mut MarkdownOwners,
        engine: &editor_ui::markdown::MarkdownEngine,
        body: &ProseBody<'_>,
        reveal: Option<&RevealHandle>,
    ) -> Result<Preparation, RealizeRowError> {
        let _prepare_span = editor_core::diagnostics::rebuild_stats::span("f_md_prepare");
        let (id, source) = (&body.key.item, body.source);
        match owners.prepare_on_realization(
            &self.scope,
            body.key.clone(),
            &id.to_string(),
            engine,
            source,
            body.item.seq,
        ) {
            Ok((0, _)) => Ok(Preparation::Empty),
            Ok((count, revision)) => Ok(Preparation::Ready { count, revision }),
            Err(error) => match self.copy.clone() {
                None => Err(error.into()),
                Some(copy) => {
                    if let Some((owner, _, _)) = reveal {
                        owner.mark_presented();
                    }
                    let copy_source = super::super::source_actions::CopySource::from_parts(
                        body.live.as_ref().map(|binding| Arc::clone(&binding.text)),
                        Arc::from(source.as_str()),
                    );
                    Ok(Preparation::Failed(
                        super::super::source_actions::failed_body(
                            &self.theme,
                            error.to_string(),
                            format!("copy-full-{id}"),
                            copy_source,
                            copy,
                        ),
                    ))
                }
            },
        }
    }

    fn bind_prose_media(
        &self,
        owners: &MarkdownOwners,
        body_key: &BodyKey,
        block: usize,
        width: f32,
    ) -> Result<(), RealizeRowError> {
        let (id, slot) = (&body_key.item, body_key.slot);
        let Some(media) = &self.media else {
            return Ok(());
        };
        let info = owners
            .projected_leaf_info(&self.scope, body_key, block)
            .ok_or(RealizeRowError::Unavailable(Part::LeafGeometry))?;
        if !owners.projected_leaf_is_current(&self.scope, body_key, block, &info) {
            return Err(RealizeRowError::Stale(Staleness::LeafGeometry));
        }
        let width = media_width(super::super::reading_width(width), slot, info.list_depth);
        if width.is_nan() || width <= 0.0 {
            return Err(RealizeRowError::Unmeasured);
        }
        if let Some(services) = media(body_key, width) {
            if services.width.to_bits() != width.to_bits() {
                return Err(RealizeRowError::MediaWidth);
            }
            owners.set_realization_media(&self.scope, body_key, &id.to_string(), services)?;
        }
        Ok(())
    }

    fn project_prose(
        &self,
        owners: &mut MarkdownOwners,
        body: &ProseBody<'_>,
        reveal: Option<&RevealHandle>,
    ) -> Result<PreparedProjectedRow, RealizeRowError> {
        let (body_key, item, source, block) = (
            body.key.clone(),
            body.item,
            body.source.as_str(),
            body.block,
        );
        let slot = body_key.slot;
        let code = owners.code_services(
            &self.scope,
            &body_key,
            &self.theme,
            self.reduced_motion,
            true,
            Rc::clone(&self.on_code),
        )?;
        let order = fold::selection_order(self.scope.agent.is_some(), item.position, slot);
        let id = item.id.to_string();
        let services = HeatmapServices::new(self.theme);
        let projected_span = editor_core::diagnostics::rebuild_stats::span("f_md_project");
        let prepared = owners.prepare_projected_row_with_geometry(
            &self.scope,
            &MarkdownRender {
                key: body_key,
                id: &id,
                source,
                theme: &self.theme,
                tone: BodyTone::Foreground,
                order: &order,
                glyph: editor_ui::markdown::glyph_painter(),
                invalidate: Rc::clone(&self.invalidate),
                on_link: Rc::clone(&self.on_link),
                on_color: Rc::clone(&self.on_color),
                reference_scope: Some(ResourceScope {
                    host: self.scope.host.to_string(),
                    thread: self.scope.thread.clone(),
                }),
                heatmap: &services,
            },
            block,
            Some(&code),
            reveal.map(|(owner, _, _)| owner),
            || (self.allocate)(),
        )?;
        drop(projected_span);
        Ok(prepared)
    }

    fn queue_projection(
        &self,
        cx: &RowContext<'_>,
        item: &Item,
        slot: Slot,
        revision: u64,
        count: usize,
    ) {
        let row = cx.row;
        let version = self
            .projection_versions
            .get(&BodyKey {
                item: item.id.clone(),
                slot,
            })
            .copied()
            .unwrap_or(row.model.version);
        self.pending_projections.borrow_mut().push((
            BodyKey {
                item: item.id.clone(),
                slot,
            },
            crate::conversation::projection::BodyProjection {
                version,
                revision,
                leaves: 0..count,
            },
        ));
    }

    fn reveal_frame(
        &self,
        element: Element,
        entrance: Option<(f32, f32, f32)>,
        reveal: Option<&RevealHandle>,
    ) -> Element {
        let element = if let Some((alpha, rise, height)) = entrance {
            use aqua_ui::element::Box as UiBox;
            use aqua_ui::style::{Length, Style};
            let inner = UiBox::new()
                .width(Length::Pct(100.0))
                .style(
                    Style::new()
                        .flex_shrink(0.0)
                        .opacity(alpha)
                        .transform(aqua_core::Transform2D::translate(0.0, rise)),
                )
                .child(element)
                .build();
            UiBox::new()
                .layout("column")
                .width(Length::Pct(100.0))
                .style(Style::new().height(Length::Px(height)).clip(true))
                .child(inner)
                .build()
        } else {
            element
        };
        if let Some((owner, _, _)) = reveal {
            owner.mark_presented();
            if !owner.fully_visible() && !self.reduced_motion {
                use aqua_ui::element::Box as UiBox;
                use aqua_ui::style::{Length, Style};
                UiBox::new()
                    .layout("column")
                    .width(Length::Pct(100.0))
                    .style(
                        Style::new()
                            .height(Length::Px(owner.visible_height()))
                            .clip(true),
                    )
                    .child(
                        UiBox::new()
                            .width(Length::Pct(100.0))
                            .style(Style::new().flex_shrink(0.0))
                            .child(element)
                            .build(),
                    )
                    .build()
            } else {
                element
            }
        } else {
            element
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/conversation/view/factory/prose.rs"]
mod tests;
