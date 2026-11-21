use super::fold::{Row, Slot};
use super::presentation::BodyKey;
use artisan_api::ItemId;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BodyProjection {
    pub version: u64,
    pub revision: u64,
    pub leaves: Range<usize>,
}

#[derive(Debug, Clone, Copy)]
pub enum ShapeText<'a> {
    Inline(&'a str),
    Live(&'a ropey::Rope),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeRevision {
    Saved(artisan_api::Seq),
    Live { epoch: u64, bytes: usize },
}

#[derive(Debug, Clone, Copy)]
pub struct ShapeSource<'a> {
    pub(crate) text: ShapeText<'a>,
    pub(crate) revision: ShapeRevision,
    pub(crate) message: bool,
    pub(crate) seq: u64,
    pub(crate) running: bool,
}

impl ShapeSource<'_> {
    fn text(&self) -> Cow<'_, str> {
        match self.text {
            ShapeText::Inline(text) => Cow::Borrowed(text),
            ShapeText::Live(rope) => Cow::Owned(rope.to_string()),
        }
    }

    fn appended_to(&self, shape: &BodyShape) -> Option<String> {
        match (shape.revision, self.revision, self.text) {
            (
                ShapeRevision::Live { epoch, bytes },
                ShapeRevision::Live {
                    epoch: next_epoch,
                    bytes: next_bytes,
                },
                ShapeText::Live(rope),
            ) if epoch == next_epoch
                && bytes <= next_bytes
                && shape.message == self.message
                && !self.message =>
            {
                Some(rope.byte_slice(bytes..).chunks().collect())
            }
            (
                ShapeRevision::Saved(_) | ShapeRevision::Live { .. },
                ShapeRevision::Saved(_) | ShapeRevision::Live { .. },
                ShapeText::Inline(_) | ShapeText::Live(_),
            ) => None,
        }
    }
}

struct BodyShape {
    revision: ShapeRevision,
    message: bool,
    signatures: Arc<[u64]>,
    streaming: Option<editor_ui::markdown::ShapedSource>,
}

#[derive(Default)]
pub struct Projections {
    bodies: HashMap<BodyKey, BodyProjection>,
    shapes: HashMap<BodyKey, BodyShape>,
}

impl std::fmt::Debug for Projections {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Projections").finish_non_exhaustive()
    }
}

fn block_version(signature: u64, seq: u64, running: bool, block: usize, count: usize) -> u64 {
    let mut hash = DefaultHasher::new();
    signature.hash(&mut hash);
    seq.hash(&mut hash);
    block.hash(&mut hash);
    (block + 1 == count).hash(&mut hash);
    (running && block + 1 == count).hash(&mut hash);
    hash.finish()
}

impl Projections {
    pub(crate) fn resident_bytes(&self) -> usize {
        self.bodies.capacity() * size_of::<(BodyKey, BodyProjection)>()
            + self.shapes.capacity() * size_of::<(BodyKey, BodyShape)>()
            + self
                .shapes
                .values()
                .map(|shape| {
                    size_of_val(&*shape.signatures)
                        + shape
                            .streaming
                            .as_ref()
                            .map_or(0, editor_ui::markdown::ShapedSource::resident_bytes)
                })
                .sum::<usize>()
    }

    pub(crate) fn leaf_count(&self, key: &BodyKey) -> Option<usize> {
        if let Some(shape) = self.shapes.get(key) {
            return Some(shape.signatures.len());
        }
        let leaves = &self.bodies.get(key)?.leaves;
        (leaves.start == 0).then_some(leaves.end)
    }

    pub(crate) fn shaped(&self, key: &BodyKey) -> bool {
        self.shapes.contains_key(key)
    }

    pub(crate) fn clear(&mut self) {
        self.bodies.clear();
        self.shapes.clear();
    }

    pub fn publish(&mut self, key: BodyKey, projection: BodyProjection) -> bool {
        if self.shapes.contains_key(&key) || self.bodies.get(&key) == Some(&projection) {
            return false;
        }
        self.bodies.insert(key, projection);
        true
    }

    fn shape(&mut self, key: &BodyKey, source: &ShapeSource<'_>) -> Option<Arc<[u64]>> {
        if let Some(shape) = self
            .shapes
            .get(key)
            .filter(|shape| shape.revision == source.revision && shape.message == source.message)
        {
            return Some(Arc::clone(&shape.signatures));
        }
        let previous = self.shapes.remove(key);
        let appended = previous
            .as_ref()
            .filter(|shape| shape.streaming.is_some())
            .and_then(|shape| source.appended_to(shape));
        let streaming = previous.and_then(|shape| shape.streaming);
        let shaped = match (streaming, appended) {
            (Some(streaming), Some(appended)) => streaming.append(&appended),
            (streaming, None | Some(_)) => {
                let text = source.text();
                let command = source
                    .message
                    .then(|| artisan_mentions::special_command(&text))
                    .flatten()
                    .and_then(|range| text.get(range));
                match streaming {
                    Some(previous) => previous.extend(&text, command),
                    None => editor_ui::markdown::ShapedSource::new(&text, command),
                }
            }
        };
        let shaped = match shaped {
            Ok(shaped) => shaped,
            Err(error) => {
                tracing::warn!(item = %key.item, slot = ?key.slot, %error, "shape message body; showing it as one block");
                return None;
            }
        };
        let signatures = Arc::clone(shaped.signatures());
        self.shapes.insert(
            key.clone(),
            BodyShape {
                revision: source.revision,
                message: source.message,
                signatures: Arc::clone(&signatures),
                streaming: source.running.then_some(shaped),
            },
        );
        Some(signatures)
    }

    pub fn expand<'a>(
        &mut self,
        rows: Vec<(Row, u64)>,
        mut literal: impl FnMut(&ItemId, Slot) -> Option<String>,
        mut source: impl FnMut(&ItemId, Slot) -> Option<ShapeSource<'a>>,
    ) -> Vec<(Row, u64)> {
        let mut retained = HashSet::new();
        let mut shaped = HashSet::new();
        let mut result = Vec::new();
        for (row, version) in rows {
            let Row::Text { id, slot, run, .. } = &row else {
                result.push((row, version));
                continue;
            };
            let key = BodyKey {
                item: id.clone(),
                slot: *slot,
            };
            retained.insert(key.clone());
            if slot.literal()
                && let Some(text) = literal(id, *slot)
            {
                for block in 0..super::fold::plain_ranges(&text).len() {
                    result.push((
                        Row::Text {
                            id: id.clone(),
                            slot: *slot,
                            block,
                            run: run.clone(),
                        },
                        version,
                    ));
                }
                continue;
            }
            let shape_source = if slot.literal() {
                None
            } else {
                source(id, *slot)
            };
            if let Some(source) = shape_source
                && let Some(signatures) = self
                    .shape(&key, &source)
                    .filter(|signatures| !signatures.is_empty())
            {
                shaped.insert(key.clone());
                let count = signatures.len();
                for (block, signature) in signatures.iter().enumerate() {
                    result.push((
                        Row::Text {
                            id: id.clone(),
                            slot: *slot,
                            block,
                            run: run.clone(),
                        },
                        block_version(*signature, source.seq, source.running, block, count),
                    ));
                }
                continue;
            }
            let Some(shape) = self
                .bodies
                .get(&key)
                .filter(|shape| shape.version == version && !shape.leaves.is_empty())
            else {
                result.push((row, version));
                continue;
            };
            let mut hash = DefaultHasher::new();
            version.hash(&mut hash);
            shape.revision.hash(&mut hash);
            let projected_version = hash.finish();
            for block in shape.leaves.clone() {
                result.push((
                    Row::Text {
                        id: id.clone(),
                        slot: *slot,
                        block,
                        run: run.clone(),
                    },
                    projected_version,
                ));
            }
        }
        self.bodies.retain(|key, _| retained.contains(key));
        self.shapes.retain(|key, _| shaped.contains(key));
        result
    }
}

#[cfg(test)]
#[path = "../../tests/conversation/projection.rs"]
mod tests;
