use super::{RowFactory, RowFingerprint, RowSource};
use crate::conversation::fold::{Row, RowKey};
use crate::conversation::presentation::BodyKey;
use std::rc::Rc;
use std::sync::Arc;

impl RowFactory {
    pub(super) fn fingerprint(&self, index: usize, key: &RowKey) -> Option<u64> {
        if let Some((known, fingerprint)) = self.fingerprints.borrow().get(&index) {
            return fingerprint.filter(|_| known == key);
        }
        let row_key = self.row_key(index)?;
        let fingerprint = self.compute_fingerprint(index);
        self.fingerprints
            .borrow_mut()
            .insert(index, (row_key.clone(), fingerprint));
        fingerprint.filter(|_| &row_key == key)
    }

    fn compute_fingerprint(&self, index: usize) -> Option<u64> {
        let RowSource::Snapshots { snapshots, rows } = &self.rows else {
            return None;
        };
        let model = snapshots.model(index)?;
        let row = rows.get(index)?;
        let body_version = match model.row.semantic() {
            Row::Text { id, slot, .. } => {
                let key = BodyKey {
                    item: id.clone(),
                    slot: *slot,
                };
                if self.shape_counts.contains_key(&key) {
                    None
                } else {
                    self.projection_versions.get(&key).copied()
                }
            }
            Row::Work { .. } | Row::WorkHeader(_)
                if model
                    .run
                    .as_ref()
                    .is_some_and(crate::conversation::fold::RunSummary::live) =>
            {
                u64::try_from(self.work.timestamp / 1_000).ok()
            }
            Row::Projected { .. }
            | Row::Work { .. }
            | Row::WorkHeader(_)
            | Row::EarlierWork { .. }
            | Row::WorkEntry { .. }
            | Row::Chain { .. }
            | Row::Prompt(_)
            | Row::Compaction(_)
            | Row::Image(_)
            | Row::Gap
            | Row::Status(_)
            | Row::AgentResult(_)
            | Row::AgentStatus(..) => None,
        };
        Some(
            RowFingerprint {
                model: &model,
                disclosure: &row.disclosure,
                held: row.held,
                body_version,
                style_epoch: self.style_epoch,
            }
            .value(),
        )
    }

    pub fn revision(&self, index: usize, key: &RowKey) -> Option<u64> {
        let fingerprint = self.fingerprint(index, key)?;
        (self.settled.borrow().get(key) == Some(&fingerprint)).then_some(fingerprint)
    }

    fn row_key(&self, index: usize) -> Option<RowKey> {
        let RowSource::Snapshots { snapshots, .. } = &self.rows else {
            return None;
        };
        snapshots.key(index)
    }

    pub(super) fn live_text(&self, rope: &Arc<ropey::Rope>) -> Rc<str> {
        let at = Arc::as_ptr(rope) as usize;
        Rc::clone(
            &self
                .live_texts
                .borrow_mut()
                .entry(at)
                .or_insert_with(|| (Arc::clone(rope), Rc::from(rope.to_string())))
                .1,
        )
    }

    pub(super) fn inline_text(&self, item: &artisan_api::Item, text: &str) -> Rc<str> {
        Rc::clone(
            self.inline_texts
                .borrow_mut()
                .entry((item.id.clone(), item.seq))
                .or_insert_with(|| Rc::from(text)),
        )
    }
}
