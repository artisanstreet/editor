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

#[derive(Clone, Copy)]
pub enum ShapeText<'a> {
    Inline(&'a str),
    Live(&'a ropey::Rope),
}

#[derive(Clone, Copy)]
pub struct ShapeSource<'a> {
    pub text: ShapeText<'a>,
    pub message: bool,
    pub seq: u64,
    pub running: bool,
}

impl ShapeSource<'_> {
    fn fingerprint(&self) -> (usize, u64) {
        let mut hasher = DefaultHasher::new();
        let len = match self.text {
            ShapeText::Inline(text) => {
                hasher.write(text.as_bytes());
                text.len()
            }
            ShapeText::Live(rope) => {
                for chunk in rope.chunks() {
                    hasher.write(chunk.as_bytes());
                }
                rope.len_bytes()
            }
        };
        self.message.hash(&mut hasher);
        (len, hasher.finish())
    }

    fn text(&self) -> Cow<'_, str> {
        match self.text {
            ShapeText::Inline(text) => Cow::Borrowed(text),
            ShapeText::Live(rope) => Cow::Owned(rope.to_string()),
        }
    }
}

struct BodyShape {
    fingerprint: (usize, u64),
    signatures: Arc<[u64]>,
}

#[derive(Default)]
pub struct Projections {
    bodies: HashMap<BodyKey, BodyProjection>,
    shapes: HashMap<BodyKey, BodyShape>,
}

pub fn block_version(
    signature: u64,
    seq: u64,
    running: bool,
    block: usize,
    count: usize,
) -> u64 {
    let mut hash = DefaultHasher::new();
    signature.hash(&mut hash);
    seq.hash(&mut hash);
    block.hash(&mut hash);
    (block + 1 == count).hash(&mut hash);
    (running && block + 1 == count).hash(&mut hash);
    hash.finish()
}

impl Projections {
    pub fn leaf_count(&self, key: &BodyKey) -> Option<usize> {
        if let Some(shape) = self.shapes.get(key) {
            return Some(shape.signatures.len());
        }
        let leaves = &self.bodies.get(key)?.leaves;
        (leaves.start == 0).then_some(leaves.end)
    }

    pub fn shaped(&self, key: &BodyKey) -> bool {
        self.shapes.contains_key(key)
    }

    pub fn clear(&mut self) {
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
        let fingerprint = source.fingerprint();
        if let Some(shape) = self
            .shapes
            .get(key)
            .filter(|shape| shape.fingerprint == fingerprint)
        {
            return Some(shape.signatures.clone());
        }
        let text = source.text();
        let command = source
            .message
            .then(|| artisan_mentions::special_command(&text))
            .flatten()
            .and_then(|range| text.get(range));
        let signatures: Arc<[u64]> = editor_ui::markdown::leaf_signatures(&text, command)
            .ok()?
            .into();
        self.shapes.insert(
            key.clone(),
            BodyShape {
                fingerprint,
                signatures: signatures.clone(),
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
            let shape_source = if slot.literal() { None } else { source(id, *slot) };
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
