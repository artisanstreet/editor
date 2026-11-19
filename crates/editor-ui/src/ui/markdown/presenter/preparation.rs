use super::super::{CodeFence, MarkdownEngine};
use super::{MarkdownPresenter, PreparationProof, PresenterError, RetainedRich, fence_key};
use crate::ui::rich::{self, RichFence};
use crate::ui::text::slice::slice;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::Arc;

impl MarkdownPresenter {
    pub fn prepare(
        &mut self,
        engine: &MarkdownEngine,
        source: &str,
    ) -> Result<PreparationProof, PresenterError> {
        self.prepare_with(engine, source, None)
    }

    pub fn prepare_message(
        &mut self,
        engine: &MarkdownEngine,
        source: &str,
    ) -> Result<PreparationProof, PresenterError> {
        let command = artisan_mentions::special_command(source)
            .and_then(|range| source.get(range))
            .map(str::to_owned);
        self.prepare_with(engine, source, command)
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one preparation pass rebuilding the document, projection, rich fences and proofs together"
    )]
    fn prepare_with(
        &mut self,
        engine: &MarkdownEngine,
        source: &str,
        command: Option<String>,
    ) -> Result<PreparationProof, PresenterError> {
        if self.projected.is_some() && self.source == source && self.command == command {
            return Ok(PreparationProof(Arc::clone(&self.proof)));
        }
        self.proof = Arc::new(());
        self.document = None;
        let extendable = self.command == command && source.starts_with(self.source.as_str());
        let (projected, kept) = match self.projected.take() {
            Some(previous) if extendable => previous.extend(
                engine,
                &self.source,
                source,
                command.as_deref(),
                &mut self.fence_tokens,
            )?,
            _ => (
                super::super::projected::ProjectedDocument::new(
                    engine,
                    source,
                    command.as_deref(),
                    &mut self.fence_tokens,
                )?,
                0,
            ),
        };
        if kept == 0 {
            self.fence_tokens.finish();
        }
        let settled = projected.code_fences_before(kept);
        let mut previous = std::mem::take(&mut self.rich);
        let mut next: Vec<Option<RetainedRich>> =
            previous.drain(..settled.min(previous.len())).collect();
        let fences: Vec<&CodeFence> = projected.code_fences().skip(next.len()).collect();
        let offset = next.len();
        next.extend((0..fences.len()).map(|_| None));
        let mut reusable: HashMap<u64, VecDeque<usize>> = HashMap::new();
        for (at, entry) in previous.iter().enumerate() {
            if let Some(entry) = entry {
                reusable
                    .entry(fence_key(&entry.source))
                    .or_default()
                    .push_back(at);
            }
        }
        for (index, fence) in fences.iter().enumerate() {
            let Some(candidates) = reusable.get_mut(&fence_key(fence)) else {
                continue;
            };
            while let Some(at) = candidates.pop_front() {
                if previous[at].as_ref().is_some_and(|entry| {
                    entry.source.language == fence.language
                        && entry.source.source == fence.source
                        && entry.source.closed == fence.closed
                }) {
                    next[offset + index] = previous[at].take();
                    break;
                }
            }
        }
        for (index, fence) in fences.iter().enumerate() {
            if next[offset + index].is_some() {
                continue;
            }
            let Some(prepared) =
                rich::prepare(fence.language.as_deref(), &fence.source, fence.closed)
            else {
                continue;
            };
            let old_entry = previous.get_mut(index).and_then(|entry| {
                if entry
                    .as_ref()
                    .is_some_and(|entry| entry.source.language == fence.language)
                {
                    entry.take()
                } else {
                    None
                }
            });
            let previous_id = old_entry.and_then(|entry| {
                if matches!(&entry.prepared, RichFence::Ready(_))
                    && !matches!(&prepared, RichFence::Ready(_))
                {
                    self.remove_rich(entry.id);
                    None
                } else {
                    Some(entry.id)
                }
            });
            let id = previous_id.unwrap_or_else(|| {
                let id = self.next_rich_id;
                self.next_rich_id = self.next_rich_id.wrapping_add(1);
                id
            });
            next[offset + index] = Some(RetainedRich {
                id,
                source: (*fence).clone(),
                prepared,
            });
        }
        for removed in previous.into_iter().flatten() {
            self.remove_rich(removed.id);
        }
        let mut projected = projected;
        projected.bind_rich(|fence| next.get(fence).is_some_and(Option::is_some));
        self.rich = next;
        let previous_proofs = std::mem::take(&mut self.leaf_proofs);
        self.leaf_proofs = (0..projected.len())
            .map(|row| {
                let signature = projected.signature(row).unwrap_or_default();
                match previous_proofs.get(row) {
                    Some((previous, proof)) if *previous == signature => {
                        (signature, Arc::clone(proof))
                    }
                    _ => (signature, Arc::new(())),
                }
            })
            .collect();
        if kept > 0 {
            self.source.push_str(slice(source, self.source.len()..));
        } else {
            self.source.clear();
            self.source.push_str(source);
        }
        self.command = command;
        self.projected = Some(projected);
        Ok(PreparationProof(Arc::clone(&self.proof)))
    }

    pub fn prepare_document(&mut self, engine: &MarkdownEngine) -> Result<(), PresenterError> {
        if self.projected.is_none() {
            return Err(PresenterError::Unavailable("unprepared"));
        }
        if self.document.is_none() {
            self.document =
                Some(engine.parse_document_with_fence_memo(&self.source, &mut self.fence_tokens)?);
        }
        Ok(())
    }

    pub(super) fn remove_rich(&self, rich_id: u64) {
        let remove = |ids: &RefCell<BTreeMap<String, BTreeSet<String>>>,
                      suffix: &str,
                      remove: &dyn Fn(&str)| {
            for (owner, keys) in ids.borrow_mut().iter_mut() {
                let key = format!("{owner}-rich-{rich_id}-{suffix}");
                if keys.remove(&key) {
                    remove(&key);
                }
            }
        };
        remove(&self.model_ids, "model", &|key| {
            self.models.release(key);
            self.model_hover.borrow_mut().remove(key);
        });
        remove(&self.image_ids, "image", &|key| {
            self.images.borrow_mut().remove(key);
        });
        remove(&self.audio_ids, "audio", &|key| self.audio.release(key));
        remove(&self.image_comparison_ids, "image-compare", &|key| {
            self.image_comparisons.borrow_mut().remove(key);
        });
        remove(&self.annotated_ids, "annotated", &|key| {
            self.annotated.borrow_mut().remove(key);
        });
        remove(&self.file_tree_ids, "file-tree", &|key| {
            self.remove_file_tree(key);
        });
        remove(&self.comparison_ids, "compare", &|key| {
            self.comparisons.remove(key);
        });
        remove(&self.stats_ids, "stats", &|key| self.stats.remove(key));
        remove(&self.callout_ids, "callout", &|key| {
            self.callouts.remove(key);
        });
        remove(&self.heatmap_ids, "heatmap", &|key| {
            self.heatmaps.remove(key);
        });
        remove(&self.funnel_ids, "funnel", &|key| self.funnels.remove(key));
        remove(&self.spans_ids, "spans", &|key| self.spans.remove(key));
        remove(&self.hierarchy_ids, "hierarchy", &|key| {
            self.hierarchies.remove(key);
        });
        remove(&self.sankey_ids, "sankey", &|key| self.sankeys.remove(key));
        remove(&self.chart_ids, "chart", &|key| self.charts.remove(key));
        remove(&self.distribution_ids, "distribution", &|key| {
            self.distributions.remove(key);
        });
        remove(&self.concentration_ids, "concentration", &|key| {
            self.concentrations.remove(key);
        });
    }
}
