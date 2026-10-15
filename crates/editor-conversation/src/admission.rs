//! The native list registers only rows admitted by current reveal/disclosure
//! frontiers. Canonical fold/source windows remain unchanged and are refolded
//! when a frontier releases; user steers keep their own immediate native rows.
use crate::{
    Conversation,
    fold::{PlainSection, Row},
};
use gpui::{App, Context};

fn immediate(row: &Row) -> bool {
    matches!(
        row.semantic(),
        Row::User(_)
            | Row::Work { .. }
            | Row::PlainBlock {
                section: PlainSection::User,
                ..
            }
    )
}
impl Conversation {
    fn speculative_reveal_visible(&self, key: &crate::fold::RowKey) -> bool {
        if self
            .view
            .as_ref()
            .is_some_and(|view| view.viewport.state().is_following())
            || self.list.is_following_tail()
        {
            return true;
        }
        self.row_context.key_rows.get(key).is_some_and(|index| {
            *index >= self.list.logical_scroll_top().item_ix
                && self.list.item_is_below_viewport(*index) == Some(false)
                && self.row_views.get(*index).is_some_and(Option::is_some)
        })
    }

    pub(crate) fn sync_reveal_admission(&mut self, cx: &mut Context<Self>) {
        let frontier = self.shared.reveal_queue.borrow().frontier_key();
        // The structural pass may have admitted a fresh leaf before its async
        // source is ready. Keep that one row registered until it can present;
        // otherwise idle source loading would repeatedly refold the transcript.
        if !cx.reduce_motion()
            && frontier.is_none()
            && self.admitted_frontier.as_ref().is_some_and(|key| {
                self.speculative_reveal_visible(key)
                    && self.row_context.key_rows.get(key).is_some_and(|index| {
                        self.row_views
                            .get(*index)
                            .and_then(Option::as_ref)
                            .is_none_or(|row| row.read(cx).awaiting_reveal_admission())
                    })
            })
        {
            return;
        }
        if frontier != self.admitted_frontier && (frontier.is_some() || self.reveal_rows_deferred) {
            self.rebuild(cx);
        }
    }
    pub(crate) fn admit_native_rows(&mut self, rows: Vec<(Row, u64)>, cx: &App) -> Vec<(Row, u64)> {
        let frontier = self
            .shared
            .reveal_queue
            .borrow()
            .frontier_key()
            .or_else(|| {
                if cx.reduce_motion() {
                    return None;
                }
                // Admit the first not-yet-presented live leaf BEFORE ListState's
                // layout pass. A zero-height first frame can then visit only that
                // frontier and immediate steers, never every future descriptor.
                rows.iter()
                    .find(|(row, _)| {
                        if immediate(row) {
                            return false;
                        }
                        let key = row.key();
                        if !self.speculative_reveal_visible(&key) {
                            return false;
                        }
                        if let Some(owner) = self
                            .row_context
                            .key_rows
                            .get(&key)
                            .and_then(|index| self.row_views.get(*index))
                            .and_then(Option::as_ref)
                        {
                            return owner.read(cx).awaiting_reveal_admission();
                        }
                        match row.semantic() {
                            Row::Reply(id) | Row::ProseBlock { id, .. } => {
                                let block = match row.semantic() {
                                    Row::ProseBlock { block, .. } => *block,
                                    _ => 0,
                                };
                                self.shared.prose_arrivals.borrow().get(id).map_or_else(
                                    || {
                                        self.view
                                            .as_ref()
                                            .and_then(|view| view.ui.get(id))
                                            .is_some_and(crate::thread::ItemUi::fresh)
                                    },
                                    |arrival| arrival.is_fresh(block),
                                )
                            }
                            Row::WorkEntry { .. }
                            | Row::Chain { .. }
                            | Row::Prompt(_)
                            | Row::Error(_)
                            | Row::Image(_) => row.anchor().is_some_and(|id| {
                                self.view
                                    .as_ref()
                                    .and_then(|view| view.ui.get(id))
                                    .is_some_and(crate::thread::ItemUi::fresh)
                            }),
                            _ => false,
                        }
                    })
                    .map(|(row, _)| row.key())
            });
        // Once per structural refold, not once per native row or animation frame.
        let limit = frontier
            .as_ref()
            .and_then(|key| rows.iter().position(|(row, _)| row.key() == *key));
        self.admitted_frontier = limit.and(frontier);
        self.reveal_rows_deferred = false;
        rows.into_iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                let queued = limit.is_some_and(|limit| index > limit) && !immediate(&entry.0);
                self.reveal_rows_deferred |= queued;
                let disclosed = self
                    .disclosures
                    .iter()
                    .all(|(header, flight)| flight.admits(header, &entry.0));
                (!queued && disclosed).then_some(entry)
            })
            .collect()
    }
}
