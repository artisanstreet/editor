//! Reveal never decides which rows exist. Fresh content reveals in order:
//! rows past the reveal frontier (the first fresh row still revealing) are
//! drawn hidden at zero height, without a view, so a burst of arrivals
//! does not prepare every later body at once. When the frontier moves, the
//! list measures only the rows it passed again; nothing refolds. User
//! steers and work headers never wait.
use crate::{
    Conversation,
    fold::{PlainSection, Row, RowKey},
};
use gpui::{App, Context};

fn immediate(row: &Row) -> bool {
    matches!(
        row.semantic(),
        Row::Work { .. }
            | Row::PlainBlock {
                section: PlainSection::User,
                ..
            }
    )
}

impl Conversation {
    fn speculative_reveal_visible(&self, key: &RowKey) -> bool {
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

    /// Whether the speculative frontier's row has yet to start revealing.
    fn awaiting(&self, key: &RowKey, cx: &App) -> bool {
        self.speculative_reveal_visible(key)
            && self.row_context.key_rows.get(key).is_some_and(|index| {
                self.row_views
                    .get(*index)
                    .and_then(Option::as_ref)
                    .is_none_or(|row| row.read(cx).awaiting_reveal_admission())
            })
    }

    /// The first fresh row that has not presented yet, chosen before the
    /// list lays out: a first frame then builds only that row and the
    /// immediate ones, never every later body.
    fn speculative_frontier(&self, cx: &App) -> Option<RowKey> {
        if cx.reduce_motion() {
            return None;
        }
        self.rows
            .iter()
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
                let fresh = |id| {
                    self.view
                        .as_ref()
                        .and_then(|view| view.ui.get(id))
                        .is_some_and(crate::thread::ItemUi::fresh)
                };
                match row.semantic() {
                    Row::ProseBlock { id, block } => {
                        let block = *block;
                        self.shared
                            .prose_arrivals
                            .borrow()
                            .get(id)
                            .map_or_else(|| fresh(id), |arrival| arrival.is_fresh(block))
                    }
                    Row::WorkEntry { .. } | Row::Chain { .. } | Row::Prompt(_) | Row::Image(_) => {
                        row.anchor().is_some_and(fresh)
                    }
                    _ => false,
                }
            })
            .map(|(row, _)| row.key())
    }

    /// Follows the reveal queue's frontier, once per frame before layout.
    pub(crate) fn sync_reveal(&mut self, cx: &mut Context<Self>) {
        let queued = self.shared.reveal_queue.borrow().frontier_key();
        if queued == self.reveal_frontier {
            return;
        }
        let frontier = match queued {
            Some(key) => Some(key),
            None => {
                // The queue drained, or a chosen row has yet to present.
                let Some(current) = self.reveal_frontier.clone() else {
                    return;
                };
                if !cx.reduce_motion() && self.awaiting(&current, cx) {
                    return;
                }
                self.speculative_frontier(cx)
            }
        };
        self.set_reveal_frontier(frontier);
    }

    /// After the rows are published: the queue's frontier, else the first
    /// fresh row.
    pub(crate) fn publish_reveal(&mut self, cx: &App) {
        let frontier = self
            .shared
            .reveal_queue
            .borrow()
            .frontier_key()
            .or_else(|| self.speculative_frontier(cx));
        self.set_reveal_frontier(frontier);
    }

    fn set_reveal_frontier(&mut self, frontier: Option<RowKey>) {
        if frontier == self.reveal_frontier {
            return;
        }
        let index = |key: &Option<RowKey>| {
            key.as_ref()
                .and_then(|key| self.row_context.key_rows.get(key).copied())
                .unwrap_or(self.rows.len())
        };
        let from = index(&self.reveal_frontier).min(index(&frontier)) + 1;
        self.reveal_frontier = frontier;
        if from < self.rows.len() {
            self.list.remeasure_items(from..self.rows.len());
        }
    }

    /// Whether the row at `index` waits, hidden, behind the reveal frontier.
    pub(crate) fn behind_reveal(&self, index: usize, row: &Row) -> bool {
        if immediate(row) {
            return false;
        }
        let queued = self.shared.reveal_queue.borrow().frontier_key();
        queued
            .as_ref()
            .or(self.reveal_frontier.as_ref())
            .and_then(|key| self.row_context.key_rows.get(key))
            .is_some_and(|frontier| index > *frontier)
    }

    /// The rows a reader can see now: up to the reveal frontier.
    pub(crate) fn revealed_rows(&self) -> &[(Row, u64)] {
        let end = self
            .reveal_frontier
            .as_ref()
            .and_then(|key| self.row_context.key_rows.get(key))
            .map_or(self.rows.len(), |index| index + 1);
        &self.rows[..end.min(self.rows.len())]
    }
}
