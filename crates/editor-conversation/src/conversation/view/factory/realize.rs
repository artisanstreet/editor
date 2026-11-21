use super::stored_rows::StoredBlock;
use super::{
    HeldByRow, Part, RealizeRowError, RealizedRow, RowFactory, RowSource, Staleness, authored,
    bubble_sender, image_caption, message_author, plain_block, row_settles_statically, truncate,
    user_bubble,
};
use crate::conversation::disclosure::RowDisclosure;
use crate::conversation::fold::{self, NonEmpty, PlainSection, Row, RowKey, Slot};
use crate::conversation::presentation::BodyKey;
use crate::conversation::stored::StoredProof;
use crate::conversation::view::reveal::{MountedReveal, RevealHandle};
use crate::conversation::view::row_cache::RowOwner;
use crate::conversation::view::{row_frame, status, work};
use aqua_ui::collection::{BatchedCollectionRow, SelectionSourceBatch};
use aqua_ui::element::Element;
use artisan_api::{ItemId, ItemKind, RunId, Status, TextContent};
use editor_ui::kit::{Space, Spaced};
use std::rc::Rc;
use std::sync::Arc;

#[derive(Clone, Copy)]
pub(super) struct RowContext<'a> {
    pub(super) row: &'a RealizedRow,
    pub(super) key: &'a RowKey,
    pub(super) owner: &'a Rc<RowOwner>,
    pub(super) reveal: Option<&'a RevealHandle>,
    pub(super) content_width: Option<f32>,
}

#[derive(Default)]
pub(super) struct RowOutput {
    pub(super) batches: Vec<Arc<SelectionSourceBatch>>,
    pub(super) stored_proof: Option<StoredProof>,
}

impl RowFactory {
    pub(super) fn realize_inner(
        &self,
        index: usize,
        key: &RowKey,
        content_width: Option<f32>,
    ) -> Result<BatchedCollectionRow, RealizeRowError> {
        let _realize_span = editor_core::diagnostics::rebuild_stats::span("f_realize");
        if !self.live.get() {
            return Err(RealizeRowError::Stale(Staleness::Factory));
        }
        if !self.row_cache.borrow().scope_is(&self.scope) {
            return Err(RealizeRowError::Stale(Staleness::RowCache));
        }
        if !self.work.motion.borrow().scope_is(&self.scope) {
            return Err(RealizeRowError::Stale(Staleness::MotionScope));
        }
        let generated;
        let row = match &self.rows {
            RowSource::Snapshots { snapshots, rows } => {
                let state = rows
                    .get(index)
                    .ok_or(RealizeRowError::Unavailable(Part::Row))?;
                generated = RealizedRow {
                    model: snapshots
                        .model(index)
                        .ok_or(RealizeRowError::Unavailable(Part::Row))?,
                    disclosure: state.disclosure,
                    held: state.held,
                };
                &generated
            }
            RowSource::Realized(rows) => rows
                .get(index)
                .ok_or(RealizeRowError::Unavailable(Part::Row))?,
        };
        if &row.model.row.key() != key {
            return Err(RealizeRowError::Stale(Staleness::RowIdentity));
        }
        editor_core::diagnostics::rebuild_stats::record_event(
            if self.row_cache.borrow().get(key).is_some() {
                "row_rerealized"
            } else {
                "row_realized"
            },
        );
        self.unsettled.set(false);
        self.content_keyed.set(false);
        self.settled.borrow_mut().remove(key);
        let row_owner = self.row_cache.borrow().candidate(key);
        *row_owner.stored_leaf.borrow_mut() = None;
        row_owner.motion.borrow_mut().begin_frame();
        row_owner
            .motion
            .borrow_mut()
            .on_refit(Rc::clone(&self.invalidate));

        let reveal = self.acquire_reveal(row, &row_owner);
        let queue_held = self.queue_holds(row, key);
        let deferred = row.disclosure.clip.is_none()
            && row.disclosure.fraction.is_none()
            && row.disclosure.hidden;
        if row.held || queue_held || deferred {
            if let Some((reveal, _, ready)) = &reveal {
                reveal.set_paused(true);
                ready.set(false);
            }
            if let Some(queue) = &self.admission {
                queue.borrow_mut().finish(key, row.held || queue_held);
            }
            return Ok(BatchedCollectionRow {
                element: row_frame(
                    &row.model,
                    row.disclosure,
                    row.held || queue_held,
                    None,
                    &self.theme,
                ),
                batches: Vec::new(),
            });
        }
        let cx = RowContext {
            row,
            key,
            owner: &row_owner,
            reveal: reveal.as_ref(),
            content_width,
        };
        let mut out = RowOutput::default();
        let content = self.row_content(&cx, &mut out)?;
        let (content, work_settling) = self.entered(&cx, content);
        self.settle(&cx, index, work_settling);
        let element = self.observed_frame(&cx, content, out.stored_proof);
        Ok(self.commit_row(key, row_owner, element, out.batches))
    }

    fn acquire_reveal(&self, row: &RealizedRow, row_owner: &Rc<RowOwner>) -> Option<RevealHandle> {
        match (&self.reveals, row.model.row.semantic()) {
            (
                Some(resources),
                Row::Text {
                    id,
                    slot: Slot::Prose,
                    block,
                    ..
                },
            ) => {
                let initial = row.model.ui.get(id).is_some_and(|ui| ui.fresh(self.now));
                Some(MountedReveal::acquire(row_owner, || {
                    resources
                        .arrivals
                        .borrow_mut()
                        .get_mut(id)
                        .map_or(initial, |arrival| arrival.claim(*block))
                }))
            }
            _ => None,
        }
    }

    fn queue_holds(&self, row: &RealizedRow, key: &RowKey) -> bool {
        self.admission.as_ref().is_some_and(|queue| {
            let semantic = row.model.row.semantic();
            let position = row
                .model
                .row
                .anchor()
                .and_then(|id| row.model.items.get(id))
                .map_or(u64::MAX, |item| item.position);
            let (part, ordinal) = match semantic {
                Row::Chain { .. } => (0, 0),
                Row::Text {
                    slot: Slot::Prose | Slot::Text(_),
                    block,
                    ..
                } => (2, *block),
                Row::Projected { .. }
                | Row::Work { .. }
                | Row::WorkHeader(_)
                | Row::EarlierWork { .. }
                | Row::WorkEntry { .. }
                | Row::Text { .. }
                | Row::Prompt(_)
                | Row::Compaction(_)
                | Row::Image(_)
                | Row::Gap
                | Row::Status(_)
                | Row::AgentResult(_)
                | Row::AgentStatus(..) => (1, 0),
            };
            let mut queue = queue.borrow_mut();
            let enabled = !crate::conversation::admission::immediate(semantic)
                && !queue.rushed(position)
                && !self.reduced_motion;
            queue.begin(key.clone(), (position, part, ordinal), enabled, self.now)
        })
    }

    fn row_content(
        &self,
        cx: &RowContext<'_>,
        out: &mut RowOutput,
    ) -> Result<Option<Element>, RealizeRowError> {
        let row = cx.row;
        match &row.model.row {
            Row::Gap | Row::EarlierWork { .. } => Ok(super::super::load_row(
                &row.model.row,
                &self.theme,
                Rc::clone(&self.dispatch),
            )),
            Row::Work {
                run,
                segment,
                members,
            } => self.work_header(
                cx,
                run.as_ref(),
                *segment,
                !members.entries().is_empty() || !members.spans().is_empty(),
            ),
            Row::WorkHeader(run) => self.work_header(cx, Some(run), 0, false),
            Row::Chain { ids, .. } => Ok(self.chain_row(cx, ids)),
            Row::WorkEntry { id, .. } => self.work_entry_row(cx, out, id),
            Row::Text {
                id,
                slot: slot @ Slot::Plain(section),
                block,
                ..
            } if has_stored_text(row, id, *slot) => {
                self.stored_literal_row(cx, out, id, *slot, *section, *block)
            }
            Row::Text {
                id, slot, block, ..
            } if matches!(slot, Slot::Prose | Slot::Text(_))
                && shows_stored_text(row, id, *slot) =>
            {
                self.stored_text_row(cx, out, id, *slot, *block)
            }
            Row::Text {
                id, slot, block: 0, ..
            } if matches!(slot, Slot::Text(_)) && has_empty_inline_text(row, id, *slot) => {
                self.attachments_row(cx, id)
            }
            Row::Text {
                id,
                slot: slot @ (Slot::Prose | Slot::Text(_)),
                block,
                ..
            } => self.prose_row(cx, out, id, *slot, *block),
            Row::Prompt(id) => self.prompt_row(cx, out, id),
            Row::Image(id) => self.image_row(cx, id),
            Row::Compaction(id) => self.compaction_row(cx, id),
            Row::AgentResult(_) => Ok(None),
            Row::Status(run) => Ok(self.status_row(cx, run)),
            Row::AgentStatus(_, doing) => Ok(status::agent(
                doing.as_deref(),
                &self.theme,
                |text, theme| {
                    self.statuses
                        .borrow_mut()
                        .agent(cx.key, text, self.reduced_motion, &theme)
                },
            )),
            Row::Text {
                id,
                slot: Slot::Plain(section),
                block,
                ..
            } => self.plain_row(cx, id, *section, *block),
            Row::Projected { .. } | Row::Text { .. } => Err(RealizeRowError::Unrenderable {
                key: cx.key.clone(),
            }),
        }
    }

    fn work_header(
        &self,
        cx: &RowContext<'_>,
        run: Option<&RunId>,
        segment: usize,
        has_members: bool,
    ) -> Result<Option<Element>, RealizeRowError> {
        let RowContext {
            row,
            owner: row_owner,
            ..
        } = *cx;
        Ok({
            let work = &self.work;
            let dispatch = &self.dispatch;
            if segment != 0 || row.model.run.is_none() {
                None
            } else {
                let summary = row
                    .model
                    .run
                    .as_ref()
                    .ok_or(RealizeRowError::Unavailable(Part::Run))?;
                let key = format!("work-{}", run.map_or_else(String::new, ToString::to_string));
                let label = row_owner.motion.borrow_mut().phrase(
                    &format!("{key}-label"),
                    work::section_title(summary, work.timestamp),
                    self.theme.colors.text_muted.to_reference_text_color(),
                    &self.theme,
                    work.seconds,
                    self.reduced_motion,
                );
                work::header(
                    &self.theme,
                    run,
                    segment,
                    has_members,
                    Some(summary),
                    label,
                    |open| {
                        row_owner.motion.borrow_mut().chevron(
                            &key,
                            open,
                            row.disclosure.fraction,
                            editor_ui::kit::IconSize::Md.px(),
                            self.theme.colors.text_muted.to_reference_paint(),
                            self.now,
                            self.reduced_motion,
                        )
                    },
                    Rc::clone(dispatch),
                )
            }
        })
    }

    fn chain_row(&self, cx: &RowContext<'_>, ids: &NonEmpty<ItemId>) -> Option<Element> {
        let RowContext {
            row,
            owner: row_owner,
            ..
        } = *cx;
        let work = &self.work;
        let dispatch = &self.dispatch;
        let open = row
            .model
            .ui
            .get(ids.first())
            .and_then(|ui| ui.chain_open)
            .unwrap_or(false);
        let key = format!("chain-{}", ids.first());
        work::chain(
            ids.as_slice(),
            &row.model.items,
            open,
            &self.theme,
            |text, color| {
                row_owner.motion.borrow_mut().phrase(
                    &format!("{key}-clause"),
                    text,
                    color,
                    &self.theme,
                    work.seconds,
                    self.reduced_motion,
                )
            },
            |open, color| {
                row_owner.motion.borrow_mut().chevron(
                    &key,
                    open,
                    row.disclosure.fraction,
                    editor_ui::kit::IconSize::Md.px(),
                    color,
                    self.now,
                    self.reduced_motion,
                )
            },
            Rc::clone(dispatch),
        )
    }

    fn work_entry_row(
        &self,
        cx: &RowContext<'_>,
        out: &mut RowOutput,
        id: &ItemId,
    ) -> Result<Option<Element>, RealizeRowError> {
        let RowContext {
            row,
            key,
            owner: row_owner,
            ..
        } = *cx;
        Ok({
            let work = &self.work;
            let dispatch = &self.dispatch;
            let item = row
                .model
                .items
                .get(id)
                .ok_or(RealizeRowError::Unavailable(Part::Item))?;
            let expanded = row
                .model
                .ui
                .get(id)
                .and_then(|ui| ui.expanded)
                .unwrap_or(false);
            match &item.kind {
                ItemKind::Agent { .. } => work::agent(
                    item,
                    &self.theme,
                    |text, color| truncate(text, color, false, &self.theme),
                    Rc::clone(dispatch),
                ),
                ItemKind::Tool { .. }
                | ItemKind::UserMessage {
                    from_agent: Some(_),
                    ..
                } => self.activity_row(
                    row_owner,
                    item,
                    expanded,
                    &work.names,
                    Rc::clone(dispatch),
                    &mut out.batches,
                )?,
                ItemKind::Plan { .. } => work::plan_entry(item, &self.theme),
                ItemKind::Notice { .. } => self.notice_row(row_owner, item, &mut out.batches)?,
                ItemKind::Question { .. } => {
                    self.question_card(row_owner, item, &mut out.batches)?
                }
                ItemKind::UserMessage { .. }
                | ItemKind::AssistantMessage { .. }
                | ItemKind::Image { .. }
                | ItemKind::Compaction { .. }
                | ItemKind::Error { .. } => {
                    return Err(RealizeRowError::Unrenderable { key: key.clone() });
                }
            }
        })
    }

    fn stored_literal_row(
        &self,
        cx: &RowContext<'_>,
        out: &mut RowOutput,
        id: &ItemId,
        slot: Slot,
        section: PlainSection,
        block: usize,
    ) -> Result<Option<Element>, RealizeRowError> {
        let row = cx.row;
        Ok({
            let item = row
                .model
                .items
                .get(id)
                .ok_or(RealizeRowError::Unavailable(Part::Item))?;
            let Some(TextContent::Stored(reference)) = slot.text(item) else {
                return Err(RealizeRowError::Changed(Part::StoredSource));
            };
            Some(self.literal_row(
                cx,
                StoredBlock {
                    item,
                    slot,
                    block,
                    reference,
                },
                section,
                &mut out.batches,
            )?)
        })
    }

    fn stored_text_row(
        &self,
        cx: &RowContext<'_>,
        out: &mut RowOutput,
        id: &ItemId,
        slot: Slot,
        block: usize,
    ) -> Result<Option<Element>, RealizeRowError> {
        let RowContext {
            row,
            key,
            owner: row_owner,
            ..
        } = *cx;
        Ok({
            let item = row
                .model
                .items
                .get(id)
                .ok_or(RealizeRowError::Unavailable(Part::Item))?;
            let Some(TextContent::Stored(reference)) = slot.text(item) else {
                return Err(RealizeRowError::Changed(Part::StoredSource));
            };
            let (element, proof) = self.stored_row(
                cx,
                StoredBlock {
                    item,
                    slot,
                    block,
                    reference,
                },
                &mut out.batches,
            )?;
            out.stored_proof = proof;
            Some(if slot.is_bubble() {
                let count = self
                    .stored
                    .owner
                    .borrow()
                    .block_count(&BodyKey {
                        item: id.clone(),
                        slot,
                    })
                    .unwrap_or(1);
                let sticker = self.bubble_sticker(
                    row_owner,
                    key,
                    row.model.ui.get(id).is_some_and(|ui| ui.fresh(self.now)),
                );
                user_bubble(
                    element,
                    block,
                    count,
                    false,
                    if block == 0 {
                        self.user_attachments(row_owner, item)
                    } else {
                        None
                    },
                    bubble_sender(item, row.model.author),
                    sticker,
                    &self.theme,
                )
            } else {
                element
            })
        })
    }

    fn attachments_row(
        &self,
        cx: &RowContext<'_>,
        id: &ItemId,
    ) -> Result<Option<Element>, RealizeRowError> {
        let RowContext {
            row,
            owner: row_owner,
            ..
        } = *cx;
        Ok({
            use aqua_ui::element::Box as UiBox;
            use aqua_ui::style::{Length, Style};
            let item = row
                .model
                .items
                .get(id)
                .ok_or(RealizeRowError::Unavailable(Part::Item))?;
            let mut root = UiBox::new()
                .layout("column")
                .width(Length::Pct(100.0))
                .spaced(Space::S8)
                .style(Style::new().align(aqua_ui::AlignItems::END));
            if let Some(attachments) = self.user_attachments(row_owner, item) {
                root = root.child(attachments);
            }
            Some(root.build())
        })
    }

    fn prompt_row(
        &self,
        cx: &RowContext<'_>,
        out: &mut RowOutput,
        id: &ItemId,
    ) -> Result<Option<Element>, RealizeRowError> {
        let RowContext {
            row,
            owner: row_owner,
            ..
        } = *cx;
        Ok({
            let item = row
                .model
                .items
                .get(id)
                .ok_or(RealizeRowError::Unavailable(Part::Item))?;
            if matches!(&item.kind, ItemKind::Question { .. }) {
                self.question_card(row_owner, item, &mut out.batches)?
            } else {
                let dispatch = &self.dispatch;
                let approval = self.approval_card(
                    row_owner,
                    item,
                    row.model.answers.get(id),
                    dispatch,
                    &mut out.batches,
                )?;
                if approval.is_some() {
                    approval
                } else {
                    let work = &self.work;
                    match &item.kind {
                        ItemKind::Tool { .. }
                        | ItemKind::UserMessage {
                            from_agent: Some(_),
                            ..
                        } => self.activity_row(
                            row_owner,
                            item,
                            row.model
                                .ui
                                .get(id)
                                .and_then(|ui| ui.expanded)
                                .unwrap_or(false),
                            &work.names,
                            Rc::clone(dispatch),
                            &mut out.batches,
                        )?,
                        ItemKind::Agent { .. } => work::agent(
                            item,
                            &self.theme,
                            |text, color| truncate(text, color, false, &self.theme),
                            Rc::clone(dispatch),
                        ),
                        ItemKind::Plan { .. } => work::plan_entry(item, &self.theme),
                        ItemKind::Notice { .. } => {
                            self.notice_row(row_owner, item, &mut out.batches)?
                        }
                        ItemKind::UserMessage { .. }
                        | ItemKind::AssistantMessage { .. }
                        | ItemKind::Question { .. }
                        | ItemKind::Image { .. }
                        | ItemKind::Compaction { .. }
                        | ItemKind::Error { .. } => None,
                    }
                }
            }
        })
    }

    fn image_row(
        &self,
        cx: &RowContext<'_>,
        id: &ItemId,
    ) -> Result<Option<Element>, RealizeRowError> {
        let RowContext {
            row,
            owner: row_owner,
            content_width,
            ..
        } = *cx;
        Ok({
            let item = row
                .model
                .items
                .get(id)
                .ok_or(RealizeRowError::Unavailable(Part::Item))?;
            let ItemKind::Image { blob, .. } = &item.kind else {
                return Err(RealizeRowError::Changed(Part::Item));
            };
            let width =
                super::super::reading_width(content_width.ok_or(RealizeRowError::Unmeasured)?);
            if width.is_nan() || width <= 0.0 {
                return Err(RealizeRowError::Unmeasured);
            }
            let image = self.blob_frame(row_owner, item, blob, None, width);
            Some(
                aqua_ui::element::Box::new()
                    .layout("column")
                    .spaced(Space::S6)
                    .child(image)
                    .build(),
            )
        })
    }

    fn compaction_row(
        &self,
        cx: &RowContext<'_>,
        id: &ItemId,
    ) -> Result<Option<Element>, RealizeRowError> {
        let RowContext {
            key,
            row,
            owner: row_owner,
            ..
        } = *cx;
        Ok({
            use crate::conversation::compaction::CompactionPhase;
            let item = row
                .model
                .items
                .get(id)
                .ok_or(RealizeRowError::Unavailable(Part::Item))?;
            let ItemKind::Compaction { .. } = &item.kind else {
                return Err(RealizeRowError::Changed(Part::Item));
            };
            let phase = match item.status {
                Status::Running => CompactionPhase::Running,
                Status::Done => CompactionPhase::Done,
                Status::AwaitingUser | Status::Failed | Status::Declined | Status::Cancelled => {
                    CompactionPhase::Stopped
                }
            };
            let fresh = row.model.ui.get(id).is_some_and(|ui| ui.fresh(self.now));
            let frame = row_owner.motion.borrow_mut().compaction(
                id,
                phase,
                fresh,
                self.now,
                self.reduced_motion,
            );
            let shimmer_key = (!self.reduced_motion)
                .then(|| aqua_ui::tree::hash_key(("compaction-shimmer", format!("{key:?}"))));
            super::super::compaction::row(
                item,
                frame,
                &self.theme,
                |phase, switch, frame, theme| {
                    super::super::compaction::label(phase, switch, frame, &theme, shimmer_key)
                },
            )
        })
    }

    fn plain_row(
        &self,
        cx: &RowContext<'_>,
        id: &ItemId,
        section: PlainSection,
        block: usize,
    ) -> Result<Option<Element>, RealizeRowError> {
        let RowContext { row, .. } = *cx;
        Ok({
            let item = row
                .model
                .items
                .get(id)
                .ok_or(RealizeRowError::Unavailable(Part::Item))?;
            let Some(text) = section
                .inline_text(item)
                .and_then(|text| plain_block(text, block))
            else {
                return Err(RealizeRowError::Changed(Part::PlainText));
            };
            Some(match section {
                PlainSection::ImageCaption => image_caption(text, &self.theme),
                PlainSection::Error => aqua_ui::element::Text::new(text.to_owned()).build(),
            })
        })
    }

    fn status_row(&self, cx: &RowContext<'_>, run: &RunId) -> Option<Element> {
        let RowContext {
            row,
            key,
            owner: row_owner,
            ..
        } = *cx;
        status::run(&row.model, run, &self.theme, |text, live, theme| {
            let label = match &text {
                status::StatusText::Status(label) => Some(label.clone()),
                status::StatusText::Reasoning(_) => None,
            };
            let element =
                self.statuses
                    .borrow_mut()
                    .render(key, text, live, self.reduced_motion, &theme);
            let now = self.now;
            let swap = row_owner.motion.borrow_mut().swap(
                format!("status-{key:?}"),
                label,
                self.reduced_motion,
                now,
            );
            match swap {
                Some(progress) => super::super::motion::swapped(element, progress),
                None => element,
            }
        })
    }

    fn entered(&self, cx: &RowContext<'_>, content: Option<Element>) -> (Option<Element>, bool) {
        let RowContext {
            row,
            key,
            owner: row_owner,
            ..
        } = *cx;
        let semantic = row.model.row.semantic();
        let author = message_author(
            semantic,
            semantic
                .anchor()
                .and_then(|id| row.model.items.get(id))
                .map(Arc::as_ref),
            row.model.author,
        );
        let content = match (content, author) {
            (Some(element), Some(author)) => Some(authored(element, author)),
            (content, _) => content,
        };
        let mut work_settling = false;
        let content = match content {
            Some(element) => {
                let (id, standalone) = match row.model.row.semantic() {
                    Row::WorkEntry { id, .. } => (Some(id), false),
                    Row::Chain { ids, .. } => (Some(ids.first()), false),
                    Row::Prompt(id) | Row::Image(id) | Row::AgentResult(id) => (Some(id), true),
                    Row::Text {
                        id, slot, block: 0, ..
                    } if slot.is_card() => (Some(id), true),
                    Row::Projected { .. }
                    | Row::Work { .. }
                    | Row::WorkHeader(_)
                    | Row::EarlierWork { .. }
                    | Row::Text { .. }
                    | Row::Compaction(_)
                    | Row::Gap
                    | Row::Status(_)
                    | Row::AgentStatus(..) => (None, false),
                };
                if let Some(id) = id {
                    let fresh = row.disclosure.clip.is_none()
                        && row.model.ui.get(id).is_some_and(|ui| ui.fresh(self.now));
                    let live =
                        standalone || row.model.run.as_ref().is_some_and(fold::RunSummary::live);
                    let entrance = row_owner.motion.borrow_mut().entrance(
                        key.clone(),
                        fresh,
                        live,
                        self.reduced_motion,
                        self.now,
                    );
                    if row.disclosure.clip.is_some() {
                        Some(element)
                    } else if let Some(entrance) = entrance {
                        work_settling = entrance.settling();
                        Some(entrance.wrap(
                            element,
                            row.disclosure.gap.unwrap_or(row.model.gap),
                            Rc::clone(&self.invalidate),
                        ))
                    } else {
                        Some(element)
                    }
                } else {
                    Some(element)
                }
            }
            None => None,
        };
        (content, work_settling)
    }

    fn settle(&self, cx: &RowContext<'_>, index: usize, work_settling: bool) {
        let RowContext {
            row,
            key,
            owner: row_owner,
            reveal,
            ..
        } = *cx;
        if let Some(queue) = &self.admission {
            let position = row
                .model
                .row
                .anchor()
                .and_then(|id| row.model.items.get(id))
                .map_or(u64::MAX, |item| item.position);
            let settling = !queue.borrow().rushed(position)
                && (work_settling
                    || reveal
                        .as_ref()
                        .is_some_and(|(owner, _, _)| owner.is_settling()));
            queue.borrow_mut().finish(key, settling);
        }
        let reveal_done = reveal.is_none_or(|(owner, _, _)| {
            owner.fully_visible() && !owner.is_settling() && !owner.is_animating()
        });
        let quiescent = self.content_keyed.get() || row_settles_statically(&row.model, self.now);
        let motionless = {
            let motion = row_owner.motion.borrow();
            let now = self.now;
            !motion.animating() && !motion.sticking(now)
        };
        let disclosure_rest = row.disclosure == RowDisclosure::default();
        let settled = !self.unsettled.get()
            && !work_settling
            && !row.held
            && disclosure_rest
            && quiescent
            && reveal_done
            && motionless;
        editor_core::diagnostics::rebuild_stats::record_event(match row.model.row.semantic() {
            Row::Text { .. } => "rz_kind_text",
            Row::Chain { .. } | Row::WorkEntry { .. } => "rz_kind_chain",
            Row::Work { .. } | Row::WorkHeader(_) => "rz_kind_work",
            Row::Status(_) => "rz_kind_status",
            Row::Projected { .. }
            | Row::EarlierWork { .. }
            | Row::Prompt(_)
            | Row::Compaction(_)
            | Row::Image(_)
            | Row::Gap
            | Row::AgentResult(_)
            | Row::AgentStatus(..) => "rz_kind_other",
        });
        editor_core::diagnostics::rebuild_stats::record_event(match () {
            () if settled => "rz_settled",
            () if self.unsettled.get() => "rz_why_pending",
            () if work_settling => "rz_why_entrance",
            () if row.held => "rz_why_held",
            () if !disclosure_rest => "rz_why_disclosure",
            () if !quiescent => "rz_why_live",
            () if !reveal_done => "rz_why_reveal",
            () => "rz_why_motion",
        });
        if settled && let Some(fingerprint) = self.fingerprint(index, key) {
            self.settled.borrow_mut().insert(key.clone(), fingerprint);
        }
    }

    fn observed_frame(
        &self,
        cx: &RowContext<'_>,
        content: Option<Element>,
        stored_proof: Option<StoredProof>,
    ) -> Element {
        let RowContext { row, reveal, .. } = *cx;
        if let Some(observations) = &self.observations {
            let item = row
                .model
                .row
                .anchor()
                .and_then(|id| row.model.items.get(id));
            let snapshot_live = row
                .model
                .row
                .anchor()
                .and_then(|id| self.reveals.as_ref()?.live.get(id).cloned());
            let snapshot = super::super::transcript::RowSnapshot {
                row: row.model.row.clone(),
                version: row.model.version,
                item_seq: item.map(|item| item.seq),
                source: snapshot_live
                    .as_ref()
                    .map(|live| self.live_text(&live.text))
                    .or_else(|| {
                        item.and_then(|item| match &item.kind {
                            ItemKind::AssistantMessage {
                                text: TextContent::Inline(source),
                                ..
                            } => Some(self.inline_text(item, source)),
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
                        })
                    }),
                active: Rc::clone(&self.active),
                revealed: !row.held && !row.disclosure.hidden,
                reveal_ready: reveal.map(|(_, _, ready)| Rc::clone(ready)),
                live: snapshot_live,
                stored: stored_proof.map(|proof| (Rc::downgrade(&self.stored.owner), proof)),
            };
            let (height, painted) = observations.callbacks(snapshot, Rc::clone(&self.markdown));
            let height_active = Rc::clone(&self.active);
            let paint_active = Rc::clone(&self.active);
            let height = Rc::new(move |bounds| {
                if height_active.get() {
                    height(bounds);
                }
            });
            let painted = Rc::new(move |bounds| {
                if paint_active.get() {
                    painted(bounds);
                }
            });
            super::super::measured_row_frame(
                &row.model,
                row.disclosure,
                row.held,
                content,
                &self.theme,
                height,
                painted,
            )
        } else {
            row_frame(&row.model, row.disclosure, row.held, content, &self.theme)
        }
    }

    fn commit_row(
        &self,
        key: &RowKey,
        row_owner: Rc<RowOwner>,
        element: Element,
        batches: Vec<Arc<SelectionSourceBatch>>,
    ) -> BatchedCollectionRow {
        if let Some(resources) = &self.reveals {
            resources
                .owners
                .borrow_mut()
                .activate(key.clone(), &row_owner);
        }
        self.row_cache
            .borrow_mut()
            .commit(key.clone(), Rc::clone(&row_owner));
        self.realized_owners
            .borrow_mut()
            .insert(key.clone(), Rc::downgrade(&row_owner));
        let stored_lease = row_owner.stored_leaf.borrow().clone();
        let held: HeldByRow = (row_owner, stored_lease);
        let lease = aqua_ui::element::Canvas::new()
            .style(aqua_ui::style::Style::new().absolute_at(
                Some(0.0),
                Some(0.0),
                Some(0.0),
                Some(0.0),
            ))
            .on_prepaint(move |_, _| {
                let _: &HeldByRow = &held;
            })
            .build()
            .exclude_from_hit_testing();
        let element = aqua_ui::element::Box::new()
            .width(aqua_ui::style::Length::Pct(100.0))
            .child(element)
            .child(lease)
            .build();
        BatchedCollectionRow { element, batches }
    }
}

fn item_text_is_stored(row: &RealizedRow, id: &ItemId, slot: Slot) -> bool {
    row.model
        .items
        .get(id)
        .is_some_and(|item| matches!(slot.text(item), Some(TextContent::Stored(_))))
}

fn has_stored_text(row: &RealizedRow, id: &ItemId, slot: Slot) -> bool {
    item_text_is_stored(row, id, slot)
}

fn shows_stored_text(row: &RealizedRow, id: &ItemId, slot: Slot) -> bool {
    let streaming = || {
        row.model.live.contains_key(&artisan_api::LiveTarget::Text {
            item_id: id.clone(),
        })
    };
    row.model.items.get(id).is_some_and(|item| {
        (slot != Slot::Prose || item.status != Status::Running || !streaming())
            && matches!(slot.text(item), Some(TextContent::Stored(_)))
    })
}

fn has_empty_inline_text(row: &RealizedRow, id: &ItemId, slot: Slot) -> bool {
    row.model
        .items
        .get(id)
        .is_some_and(|item| slot.inline_text(item).is_some_and(str::is_empty))
}
