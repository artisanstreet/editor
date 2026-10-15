//! Selection order and copy separators come from source ordinals.
//! The window owns one bounded gesture document; paint supplies only hit geometry.

use std::collections::HashMap;
use std::hash::{Hash as _, Hasher as _};
use std::ops::Range;
use std::rc::{Rc, Weak};

use gpui::{
    App, Bounds, EntityId, FocusHandle, Global, GlobalElementId, Pixels, Point, SharedString,
    WindowId,
};

use crate::text_navigation::SelectionUnit;

use super::{SelectableLinkHandler, SelectionSource, clamp_to_char_boundary};

/// Identity of one selectable leaf: a hash of its stable global element id.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct LeafKey(u64);

impl LeafKey {
    pub(super) fn semantic(scope: &str, order: &[u64]) -> Self {
        let mut hash = std::hash::DefaultHasher::new();
        scope.hash(&mut hash);
        order.hash(&mut hash);
        Self(hash.finish())
    }
    pub(super) fn new(id: &GlobalElementId) -> Self {
        let mut hasher = std::hash::DefaultHasher::new();
        id.hash(&mut hasher);
        Self(hasher.finish())
    }
}

/// The pointer event a roster was collected for: its kind and position.
pub(super) type EventStamp = (u8, Point<Pixels>);

/// One endpoint of the selection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Position {
    pub(super) leaf: LeafKey,
    pub(super) offset: usize,
}

/// What a leaf reports about itself, in paint order.
#[derive(Clone)]
pub(super) struct Observed {
    pub(super) key: LeafKey,
    pub(super) scope: SharedString,
    pub(super) order: Rc<[u64]>,
    pub(super) separator: &'static str,
    pub(super) text: SharedString,
    pub(super) source_slice: Option<Range<usize>>,
    /// Ranges of `text` that copy as other text: a color swatch slot copies
    /// as the value it stands for, or as nothing beside a shown value.
    pub(super) copies: Rc<[(Range<usize>, SharedString)]>,
    pub(super) view: EntityId,
    pub(super) bounds: Bounds<Pixels>,
}

impl Observed {
    pub(super) fn source_offset(&self, local: usize) -> usize {
        self.source_slice
            .as_ref()
            .map_or(0, |range| range.start)
            .saturating_add(local.min(self.text.len()))
    }
}

/// A leaf live in the rendered frame, as its listener reports it during
/// one pointer event's capture phase.
#[derive(Clone)]
pub(super) struct LiveLeaf {
    pub(super) observed: Observed,
    pub(super) layout: super::geometry::LeafLayout,
    pub(super) hovered: bool,
    pub(super) focus: Option<FocusHandle>,
    pub(super) links: Rc<[Range<usize>]>,
    pub(super) on_link: Option<SelectableLinkHandler>,
}

/// What the document remembers about a leaf.
struct Leaf {
    order: Rc<[u64]>,
    words: Option<crate::text_navigation::WordBoundaries>,
    text: SharedString,
    /// See [`Observed::copies`].
    copies: Rc<[(Range<usize>, SharedString)]>,
    view: EntityId,
    /// Text placed between the previous leaf and this one when copying.
    separator: &'static str,
    source_owner: Option<Rc<[u64]>>,
}

/// An in-flight pointer gesture.
#[derive(Clone, Debug)]
struct Drag {
    unit: SelectionUnit,
    /// The unit the press selected; every extension keeps it whole.
    anchor_start: Position,
    anchor_end: Position,
}

/// The selection of one window.
#[derive(Default)]
pub(super) struct Document {
    sources: HashMap<(SharedString, Rc<[u64]>), (Weak<SelectionSource>, EntityId)>,
    scope: Option<SharedString>,
    whole_source: bool,
    leaves: HashMap<LeafKey, Leaf>,
    retained_bytes: usize,
    order: Vec<LeafKey>,
    index: HashMap<LeafKey, usize>,
    anchor: Option<Position>,
    head: Option<Position>,
    drag: Option<Drag>,
    /// Leaves prepainted in the current frame, flushed into `order` once
    /// the frame's prepaint pass is complete.
    frame: Vec<Observed>,
    frame_open: bool,
    /// The leaves live for `roster_event`, in paint order, and whether the
    /// event was already processed.
    roster: Vec<LiveLeaf>,
    roster_event: Option<EventStamp>,
    roster_taken: bool,
    /// The leaf, glyph, and click count of the gesture's press, for link
    /// activation on release.
    pressed: Option<(LeafKey, usize, usize)>,
}

// Charge worst-case cached word boundaries, authored substitutions and ordinals too.
fn retention_cost(text: &str, copies: &[(Range<usize>, SharedString)], order: &[u64]) -> usize {
    text.len()
        .saturating_mul(1 + std::mem::size_of::<usize>())
        .saturating_add(
            copies
                .iter()
                .map(|(_, text)| text.len() + std::mem::size_of::<(Range<usize>, SharedString)>())
                .sum::<usize>(),
        )
        .saturating_add(std::mem::size_of_val(order))
        .saturating_add(std::mem::size_of::<Leaf>())
}

/// Appends `text[range]` to `copied`, writing each slot of `substitutions` that
/// lies wholly inside the range as its copy text. A slot the selection only
/// grazes copies nothing.
fn push_copied(
    copied: &mut String,
    text: &str,
    range: Range<usize>,
    substitutions: &[(Range<usize>, SharedString)],
) {
    let mut cursor = range.start;
    for (slot, copy) in substitutions {
        if slot.end <= range.start || slot.start >= range.end {
            continue;
        }
        copied.push_str(&text[cursor..slot.start.max(cursor)]);
        if slot.start >= range.start && slot.end <= range.end {
            copied.push_str(copy);
        }
        cursor = slot.end.min(range.end);
    }
    copied.push_str(&text[cursor..range.end]);
}

/// Every window's [`Document`].
#[derive(Default)]
pub(super) struct Documents(HashMap<WindowId, Document>);

impl Global for Documents {}

/// The document of `window`.
pub(super) fn document(cx: &mut App, window: WindowId) -> &mut Document {
    if cx.try_global::<Documents>().is_none() {
        cx.set_global(Documents::default());
        cx.on_window_closed(|cx, id| {
            cx.global_mut::<Documents>().0.remove(&id);
        })
        .detach();
    }
    cx.global_mut::<Documents>().0.entry(window).or_default()
}

pub(super) fn release_scope(cx: &mut App, window: WindowId, scope: &str) {
    if !cx.has_global::<Documents>() {
        return;
    }
    if let Some(document) = cx.global_mut::<Documents>().0.get_mut(&window) {
        document.release_scope(scope);
    }
}

impl Document {
    pub(super) fn register_source(&mut self, source: &Rc<SelectionSource>, view: EntityId) {
        self.sources
            .retain(|_, (source, _)| source.strong_count() > 0);
        let key = (source.scope.clone(), source.owner.clone());
        let changed = self
            .sources
            .get(&key)
            .is_none_or(|(previous, _)| previous.as_ptr() != Rc::as_ptr(source));
        self.sources.insert(key, (Rc::downgrade(source), view));
        if changed
            && self.is_active()
            && !self.whole_source
            && self.scope.as_ref() == Some(&source.scope)
        {
            self.replace_source(source, view);
        }
    }

    fn replace_source(&mut self, source: &Rc<SelectionSource>, view: EntityId) {
        let seen = Self::source_observations(source, view);
        let current: std::collections::HashSet<_> = seen.iter().map(|leaf| leaf.key).collect();
        let removed: Vec<_> = self
            .leaves
            .iter()
            .filter(|(key, leaf)| {
                leaf.source_owner.as_ref() == Some(&source.owner) && !current.contains(*key)
            })
            .map(|(key, _)| *key)
            .collect();
        if removed
            .iter()
            .any(|key| self.selected_prefix_len(*key).is_some())
        {
            self.clear();
            return;
        }
        for key in &removed {
            if let Some(leaf) = self.leaves.remove(key) {
                self.retained_bytes = self.retained_bytes.saturating_sub(retention_cost(
                    &leaf.text,
                    &leaf.copies,
                    &leaf.order,
                ));
            }
        }
        if !removed.is_empty() {
            self.order.retain(|key| self.leaves.contains_key(key));
            self.index_order();
        }
        // Only this source's replacement can remove its old leaves. Dropping
        // a renderer during window eviction leaves bounded gesture text intact.
        self.observe_batch(&seen, Some(&source.owner));
    }
    pub(super) fn release_scope(&mut self, scope: &str) {
        self.sources.retain(|_, (source, _)| {
            source
                .upgrade()
                .is_some_and(|source| source.scope.as_ref() != scope)
        });
        self.frame.retain(|leaf| leaf.scope.as_ref() != scope);
        self.roster
            .retain(|leaf| leaf.observed.scope.as_ref() != scope);
        if self
            .scope
            .as_ref()
            .is_some_and(|active| active.as_ref() == scope)
        {
            self.clear();
            self.pressed = None;
        }
    }
    fn source_observations(source: &SelectionSource, view: EntityId) -> Vec<Observed> {
        source
            .leaves
            .iter()
            .map(|leaf| Observed {
                key: LeafKey::semantic(&source.scope, &leaf.order),
                scope: source.scope.clone(),
                order: leaf.order.clone().into(),
                separator: leaf.separator,
                text: leaf.text.clone(),
                source_slice: None,
                copies: leaf.copies.clone(),
                view,
                bounds: Bounds::default(),
            })
            .collect()
    }
    fn source_leaves(&self, scope: &str) -> Vec<Observed> {
        self.sources
            .values()
            .filter_map(|(source, view)| source.upgrade().map(|source| (source, *view)))
            .filter(|(source, _)| source.scope.as_ref() == scope)
            .flat_map(|(source, view)| Self::source_observations(&source, view))
            .collect()
    }
    pub(super) fn reset_with_sources(&mut self, live: &[Observed]) {
        self.reset_to(live);
        if let Some(scope) = self.scope.clone() {
            let sources: Vec<_> = self
                .sources
                .values()
                .filter_map(|(source, view)| source.upgrade().map(|source| (source, *view)))
                .filter(|(source, _)| source.scope == scope)
                .collect();
            for (source, view) in sources {
                self.observe_batch(
                    &Self::source_observations(&source, view),
                    Some(&source.owner),
                );
            }
        }
    }
    pub(super) fn select_source(&mut self, observed: Observed) {
        let scope = observed.scope.clone();
        let has_source = self.sources.values().any(|(source, _)| {
            source
                .upgrade()
                .is_some_and(|source| source.scope == scope && !source.leaves.is_empty())
        });
        // Select All uses the renderer-owned text, even when a leaf exceeds
        // the separate gesture budget or paints only a clipped prefix.
        self.reset_to(&[observed]);
        self.anchor = None;
        self.head = None;
        self.drag = None;
        self.whole_source = has_source;
        if has_source {
            return;
        }
        let Some(first) = self.order.first().copied() else {
            return;
        };
        let last = *self.order.last().unwrap();
        self.drag = None;
        self.anchor = Some(Position {
            leaf: first,
            offset: 0,
        });
        self.head = Some(Position {
            leaf: last,
            offset: self.leaves[&last].text.len(),
        });
    }
    pub(super) fn endpoints(&self) -> (Option<Position>, Option<Position>) {
        (self.anchor.clone(), self.head.clone())
    }

    /// Whether a selection or a gesture is live, so leaves must report in.
    pub(super) fn is_active(&self) -> bool {
        self.whole_source || self.anchor.is_some() || self.drag.is_some()
    }

    pub(super) fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Queues a prepainted leaf; returns whether the caller must schedule
    /// [`Self::flush_frame`] for the end of the frame.
    pub(super) fn observe_in_frame(&mut self, observed: Observed) -> bool {
        self.frame.push(observed);
        !std::mem::replace(&mut self.frame_open, true)
    }

    /// Merges the leaves prepainted this frame into the document order.
    pub(super) fn flush_frame(&mut self) {
        if !self.frame_open {
            return;
        }
        self.frame_open = false;
        let frame = std::mem::take(&mut self.frame);
        if self.is_active() && !self.whole_source {
            self.observe(&frame);
        }
    }

    /// Records a leaf live for `event`. Capture-phase listeners run in
    /// paint order, so the roster comes out in document order.
    pub(super) fn check_in(&mut self, event: EventStamp, live: LiveLeaf) {
        if self.roster_event != Some(event) || self.roster_taken {
            self.roster.clear();
            self.roster_event = Some(event);
            self.roster_taken = false;
        }
        self.roster.push(live);
    }

    /// Claims the roster of `event` for processing, once: the first
    /// bubble-phase listener processes the event for every leaf.
    pub(super) fn take_roster(&mut self, event: EventStamp) -> Option<Vec<LiveLeaf>> {
        if self.roster_event != Some(event) || self.roster_taken {
            return None;
        }
        self.roster_taken = true;
        Some(std::mem::take(&mut self.roster))
    }

    /// Replaces the order with the live leaves, as a new gesture starts.
    pub(super) fn reset_to(&mut self, live: &[Observed]) {
        self.retained_bytes = 0;
        self.leaves.clear();
        self.order.clear();
        self.index.clear();
        self.whole_source = false;
        self.scope = live.first().map(|leaf| leaf.scope.clone());
        self.observe(live);
    }

    /// Merge one batch and sort once by source ordinal. Selection retention is
    /// bounded separately from the transcript's loaded window.
    pub(super) fn observe(&mut self, seen: &[Observed]) {
        self.observe_batch(seen, None);
    }

    fn observe_batch(&mut self, seen: &[Observed], source_owner: Option<&Rc<[u64]>>) {
        const MAX_BYTES: usize = 16 * 1024 * 1024;
        const MAX_LEAVES: usize = 8192;
        let mut bytes = self.retained_bytes;
        let mut reorder = false;
        for observed in seen {
            if self
                .scope
                .as_ref()
                .is_some_and(|scope| scope != &observed.scope)
            {
                continue;
            }
            if source_owner.is_none()
                && let Some(leaf) = self.leaves.get_mut(&observed.key)
                && leaf.source_owner.is_some()
            {
                leaf.view = observed.view;
                continue;
            }
            if observed.source_slice.is_some() {
                continue;
            }
            let previous = self
                .leaves
                .get(&observed.key)
                .map(|leaf| retention_cost(&leaf.text, &leaf.copies, &leaf.order));
            let next_bytes =
                bytes
                    .saturating_sub(previous.unwrap_or(0))
                    .saturating_add(retention_cost(
                        &observed.text,
                        &observed.copies,
                        &observed.order,
                    ));
            if next_bytes > MAX_BYTES || (previous.is_none() && self.leaves.len() >= MAX_LEAVES) {
                if previous.is_some() {
                    self.clear();
                    return;
                }
                continue;
            }
            bytes = next_bytes;
            if self.leaves.contains_key(&observed.key) {
                reorder |= self.leaves[&observed.key].order != observed.order;
                if !self.refresh_leaf(observed, observed.separator, source_owner) {
                    return;
                }
            } else {
                reorder = true;
                self.order.push(observed.key);
                self.leaves.insert(
                    observed.key,
                    Leaf {
                        order: observed.order.clone(),
                        words: None,
                        text: observed.text.clone(),
                        copies: observed.copies.clone(),
                        view: observed.view,
                        separator: observed.separator,
                        source_owner: source_owner.cloned(),
                    },
                );
            }
        }
        self.retained_bytes = bytes;
        if !reorder {
            return;
        }
        self.order.sort_unstable_by(|a, b| {
            self.leaves[a]
                .order
                .cmp(&self.leaves[b].order)
                .then(a.0.cmp(&b.0))
        });
        self.index_order();
    }

    fn index_order(&mut self) {
        self.index.clear();
        self.index.extend(
            self.order
                .iter()
                .enumerate()
                .map(|(index, key)| (*key, index)),
        );
    }

    /// Updates a known leaf. Text that changed under the selection keeps
    /// the selection only while the selected prefix survives (a streaming
    /// append); any other rewrite clears it. Returns whether the selection
    /// survived.
    fn refresh_leaf(
        &mut self,
        observed: &Observed,
        separator: &'static str,
        source_owner: Option<&Rc<[u64]>>,
    ) -> bool {
        let needed = self.selected_prefix_len(observed.key);
        let Some(leaf) = self.leaves.get_mut(&observed.key) else {
            return true;
        };
        let changed =
            !super::same_allocation(&leaf.text, &observed.text) && leaf.text != observed.text;
        let survives = !changed
            || needed.is_none_or(|needed| {
                observed.text.as_bytes().get(..needed) == leaf.text.as_bytes().get(..needed)
            });
        if changed {
            leaf.words = None;
        }
        leaf.order = observed.order.clone();
        leaf.text = observed.text.clone();
        leaf.copies = Rc::clone(&observed.copies);
        leaf.view = observed.view;
        leaf.separator = separator;
        leaf.source_owner = source_owner.cloned();
        if !survives {
            self.clear();
        }
        survives
    }

    /// How much of a leaf's old text the selection depends on, if any.
    fn selected_prefix_len(&self, key: LeafKey) -> Option<usize> {
        let (start, end) = self.ordered()?;
        let index = *self.index.get(&key)?;
        let (start_index, end_index) = (self.index[&start.leaf], self.index[&end.leaf]);
        if index < start_index || index > end_index {
            return None;
        }
        let text_len = self.leaves.get(&key).map_or(0, |leaf| leaf.text.len());
        Some(if index == end_index {
            end.offset.min(text_len)
        } else {
            text_len
        })
    }

    /// The anchor and head in document order, when both are known.
    fn ordered(&self) -> Option<(Position, Position)> {
        let (anchor, head) = (self.anchor.clone()?, self.head.clone()?);
        if self.compare(&anchor, &head)? == std::cmp::Ordering::Greater {
            Some((head, anchor))
        } else {
            Some((anchor, head))
        }
    }

    fn compare(&self, left: &Position, right: &Position) -> Option<std::cmp::Ordering> {
        let left_index = self.index.get(&left.leaf)?;
        let right_index = self.index.get(&right.leaf)?;
        Some(
            left_index
                .cmp(right_index)
                .then(left.offset.cmp(&right.offset)),
        )
    }

    /// Painted text can be a clipped prefix of a registered source leaf.
    pub(super) fn paint_range(
        &self,
        key: LeafKey,
        scope: &str,
        text: &str,
    ) -> Option<Range<usize>> {
        if self.whole_source {
            return (self.scope.as_deref() == Some(scope) && !text.is_empty())
                .then_some(0..text.len());
        }
        self.range_in(key, text)
    }

    pub(super) fn paint_slice_range(
        &self,
        key: LeafKey,
        scope: &str,
        text: &str,
        slice: Option<&Range<usize>>,
    ) -> Option<Range<usize>> {
        let Some(slice) = slice else {
            return self.paint_range(key, scope, text);
        };
        if self.whole_source {
            return (self.scope.as_deref() == Some(scope) && !text.is_empty())
                .then_some(0..text.len());
        }
        let leaf = self.leaves.get(&key)?;
        // Content replacement must not paint stale offsets over a different slice.
        if leaf.text.get(slice.clone()) != Some(text) {
            return None;
        }
        let selected = self.range_in(key, &leaf.text)?;
        let start = selected.start.max(slice.start);
        let end = selected.end.min(slice.end);
        (start < end).then_some(start.saturating_sub(slice.start)..end.saturating_sub(slice.start))
    }

    /// The selected byte range inside one leaf's `text`, if any.
    pub(super) fn range_in(&self, key: LeafKey, text: &str) -> Option<Range<usize>> {
        let (start, end) = self.ordered()?;
        let index = *self.index.get(&key)?;
        let (start_index, end_index) = (self.index[&start.leaf], self.index[&end.leaf]);
        if index < start_index || index > end_index {
            return None;
        }
        let from = if index == start_index {
            start.offset
        } else {
            0
        };
        let to = if index == end_index {
            end.offset
        } else {
            text.len()
        };
        let from = clamp_to_char_boundary(text, from);
        let to = clamp_to_char_boundary(text, to);
        (from < to).then_some(from..to)
    }

    /// Selection changes invalidate the views that own the selected sources.
    pub(super) fn painted(&self) -> Vec<(EntityId, LeafKey, Range<usize>)> {
        if self.whole_source {
            return self
                .sources
                .values()
                .filter_map(|(source, view)| {
                    let source = source.upgrade()?;
                    if self.scope.as_ref() != Some(&source.scope) {
                        return None;
                    }
                    let first = source.leaves.first()?;
                    Some((*view, LeafKey::semantic(&source.scope, &first.order), 0..1))
                })
                .collect();
        }
        let Some((start, end)) = self.ordered() else {
            return Vec::new();
        };
        let (start_index, end_index) = (self.index[&start.leaf], self.index[&end.leaf]);
        self.order[start_index..=end_index]
            .iter()
            .filter_map(|key| {
                let leaf = self.leaves.get(key)?;
                let range = self.range_in(*key, &leaf.text)?;
                Some((leaf.view, *key, range))
            })
            .collect()
    }

    /// The selected text across leaves, joined the way a browser copies
    /// block text.
    pub(super) fn selected_text(&self) -> Option<String> {
        if self.whole_source
            && let Some(scope) = &self.scope
        {
            let mut leaves = self.source_leaves(scope);
            leaves.sort_by(|a, b| a.order.cmp(&b.order));
            leaves.dedup_by(|a, b| a.key == b.key);
            let mut text = String::new();
            for (index, leaf) in leaves.iter().enumerate() {
                if index > 0 {
                    text.push_str(leaf.separator);
                }
                push_copied(&mut text, &leaf.text, 0..leaf.text.len(), &leaf.copies);
            }
            return Some(text);
        }

        let (start, end) = self.ordered()?;
        let (start_index, end_index) = (self.index[&start.leaf], self.index[&end.leaf]);
        let mut copied = String::new();
        for key in &self.order[start_index..=end_index] {
            let Some(leaf) = self.leaves.get(key) else {
                continue;
            };
            let Some(range) = self.range_in(*key, &leaf.text) else {
                continue;
            };
            if !copied.is_empty() {
                copied.push_str(leaf.separator);
            }
            push_copied(&mut copied, &leaf.text, range, &leaf.copies);
        }
        (!copied.is_empty()).then_some(copied)
    }

    /// Whether a non-empty selection exists.
    pub(super) fn has_selection(&self) -> bool {
        self.whole_source || self.ordered().is_some_and(|(start, end)| start != end)
    }

    fn unit_range(
        &mut self,
        key: LeafKey,
        unit: SelectionUnit,
        text: &str,
        offset: usize,
        dragging: bool,
    ) -> Range<usize> {
        if unit != SelectionUnit::Word {
            return if dragging {
                unit.drag_range_at(text, offset)
            } else {
                unit.range_at(text, offset)
            };
        }
        self.leaves
            .get_mut(&key)
            .expect("observed selection leaf")
            .words
            .get_or_insert_with(|| crate::text_navigation::WordBoundaries::new(text))
            .selection_range(unit, text, offset, dragging)
    }

    /// Only views between the old and new endpoints need repainting during a drag.
    pub(super) fn drag_views(
        &mut self,
        leaf: LeafKey,
        caret: usize,
        glyph: usize,
    ) -> Vec<EntityId> {
        let before = self.endpoints();
        if !self.drag_to(leaf, caret, glyph) {
            return Vec::new();
        }
        let after = self.endpoints();
        let changed = [(&before.0, &after.0), (&before.1, &after.1)];
        let mut low = usize::MAX;
        let mut high = 0;
        for (old, new) in changed {
            if old == new {
                continue;
            }
            for position in [old, new].into_iter().flatten() {
                if let Some(index) = self.index.get(&position.leaf) {
                    low = low.min(*index);
                    high = high.max(*index);
                }
            }
        }
        if low == usize::MAX {
            return Vec::new();
        }
        let mut views: Vec<_> = self.order[low..=high]
            .iter()
            .map(|key| self.leaves[key].view)
            .collect();
        views.sort_unstable();
        views.dedup();
        views
    }

    /// A press at `offset` (a glyph index) in `leaf`. Shift extends the
    /// existing selection from its anchor instead of starting a new one.
    pub(super) fn press(
        &mut self,
        leaf: LeafKey,
        offset: usize,
        unit: SelectionUnit,
        extend: bool,
    ) {
        self.whole_source = false;
        let Some(text) = self.leaves.get(&leaf).map(|leaf| leaf.text.clone()) else {
            return;
        };
        let offset = clamp_to_char_boundary(&text, offset);
        let at = |offset| Position { leaf, offset };
        if extend && unit == SelectionUnit::Character && self.anchor.is_some() {
            let anchor = self.anchor.clone().unwrap_or_else(|| at(offset));
            self.head = Some(at(offset));
            self.drag = Some(Drag {
                unit,
                anchor_start: anchor.clone(),
                anchor_end: anchor,
            });
            return;
        }
        let range = self.unit_range(leaf, unit, &text, offset, false);
        let (start, end) = (at(range.start), at(range.end));
        self.anchor = Some(start.clone());
        self.head = Some(end.clone());
        self.drag = Some(Drag {
            unit,
            anchor_start: start,
            anchor_end: end,
        });
    }

    /// Remembers the press of the current gesture.
    pub(super) fn set_pressed(&mut self, leaf: LeafKey, glyph: usize, click_count: usize) {
        self.pressed = Some((leaf, glyph, click_count));
    }

    /// Takes the press of the gesture that is ending.
    pub(super) fn take_pressed(&mut self) -> Option<(LeafKey, usize, usize)> {
        self.pressed.take()
    }

    /// Moves the gesture's head into `leaf`: to the `caret` offset when
    /// selecting characters, else to the unit around the `glyph` under the
    /// pointer. Returns whether the selection changed.
    pub(super) fn drag_to(&mut self, leaf: LeafKey, caret: usize, glyph: usize) -> bool {
        let Some(drag) = self.drag.clone() else {
            return false;
        };
        let offset = if drag.unit == SelectionUnit::Character {
            caret
        } else {
            glyph
        };
        let Some(text) = self.leaves.get(&leaf).map(|leaf| leaf.text.clone()) else {
            return false;
        };
        let offset = clamp_to_char_boundary(&text, offset);
        let range = self.unit_range(leaf, drag.unit, &text, offset, true);
        let unit_start = Position {
            leaf,
            offset: range.start,
        };
        let unit_end = Position {
            leaf,
            offset: range.end,
        };
        let before_anchor = self
            .compare(&unit_start, &drag.anchor_start)
            .is_some_and(std::cmp::Ordering::is_lt);
        let (anchor, head) = if before_anchor {
            (drag.anchor_end, unit_start)
        } else {
            (drag.anchor_start, unit_end)
        };
        let changed = self.anchor.as_ref() != Some(&anchor) || self.head.as_ref() != Some(&head);
        self.anchor = Some(anchor);
        self.head = Some(head);
        changed
    }

    /// Ends the gesture; a collapsed selection is dropped. Returns whether
    /// the gesture left a selection.
    pub(super) fn release(&mut self) -> bool {
        self.drag = None;
        if self.has_selection() {
            return true;
        }
        self.clear();
        false
    }

    /// Drops the selection, any gesture, and everything learned for them.
    pub(super) fn clear(&mut self) {
        self.anchor = None;
        self.head = None;
        self.drag = None;
        self.whole_source = false;
        self.scope = None;
        self.retained_bytes = 0;
        self.leaves.clear();
        self.order.clear();
        self.index.clear();
    }
}

#[cfg(test)]
mod tests {
    use std::hash::{Hash as _, Hasher as _};
    use std::rc::Rc;

    use gpui::{Bounds, EntityId, SharedString, point, px, size};

    use super::{Document, LeafKey, Observed, SelectionUnit, push_copied};

    fn key(name: &str) -> LeafKey {
        let mut hasher = std::hash::DefaultHasher::new();
        name.hash(&mut hasher);
        LeafKey(hasher.finish())
    }

    fn leaf(name: &str, text: &str, top: f32) -> Observed {
        Observed {
            key: key(name),
            scope: "test".into(),
            order: Rc::from([top as u64]),
            separator: if top == 44.0 { "\n\n" } else { "\n" },
            text: SharedString::from(text.to_owned()),
            source_slice: None,
            copies: std::rc::Rc::from([]),
            view: EntityId::from(1_u64),
            bounds: Bounds::new(point(px(0.0), px(top)), size(px(400.0), px(28.0))),
        }
    }

    fn page() -> Vec<Observed> {
        vec![
            leaf("heading", "Two things", 0.0),
            leaf("first", "Slow saves at idle", 44.0),
            leaf("second", "The Forge is fragile", 76.0),
        ]
    }

    #[test]
    fn copying_uses_authored_ordinals_after_visual_reordering() {
        let mut first = leaf("first", "alpha", 300.0);
        first.order = Rc::from([10, 0]);
        let mut second = leaf("second", "beta", 0.0);
        second.order = Rc::from([11, 0]);
        second.separator = "\n\n";
        let mut document = Document::default();
        document.reset_to(&[second, first]);
        document.press(key("first"), 0, SelectionUnit::Character, false);
        document.drag_to(key("second"), 4, 4);
        assert_eq!(document.selected_text().as_deref(), Some("alpha\n\nbeta"));
    }

    #[test]
    fn drag_from_one_paragraph_into_another_selects_across() {
        let mut document = Document::default();
        document.reset_to(&page());
        document.press(key("heading"), 4, SelectionUnit::Character, false);
        assert!(document.drag_to(key("second"), 9, 9));
        assert!(document.release());

        assert_eq!(document.range_in(key("heading"), "Two things"), Some(4..10));
        assert_eq!(
            document.range_in(key("first"), "Slow saves at idle"),
            Some(0..18)
        );
        assert_eq!(
            document.range_in(key("second"), "The Forge is fragile"),
            Some(0..9)
        );
        assert_eq!(
            document.selected_text().as_deref(),
            Some("things\n\nSlow saves at idle\nThe Forge")
        );
    }

    #[test]
    fn backward_drag_orders_endpoints() {
        let mut document = Document::default();
        document.reset_to(&page());
        document.press(key("second"), 3, SelectionUnit::Character, false);
        document.drag_to(key("first"), 5, 5);
        assert_eq!(
            document.selected_text().as_deref(),
            Some("saves at idle\nThe")
        );
    }

    #[test]
    fn double_click_drag_extends_by_whole_words() {
        let mut document = Document::default();
        document.reset_to(&page());
        document.press(key("first"), 6, SelectionUnit::Word, false);
        let word = document.selected_text().expect("word selected");
        let text = "Slow saves at idle";
        let anchor = SelectionUnit::Word.range_at(text, 6);
        assert_eq!(word, &text[anchor.clone()]);
        document.drag_to(key("first"), 15, 15);
        assert_eq!(document.selected_text().as_deref(), Some("saves at idle"));
        document.drag_to(key("first"), 1, 1);
        assert_eq!(
            document.selected_text().as_deref(),
            Some(&text[..anchor.end])
        );
    }

    #[test]
    fn click_without_movement_leaves_no_selection() {
        let mut document = Document::default();
        document.reset_to(&page());
        document.press(key("first"), 3, SelectionUnit::Character, false);
        assert!(!document.release());
        assert_eq!(document.selected_text(), None);
        assert!(!document.is_active());
    }

    #[test]
    fn leaves_seen_later_merge_into_document_order() {
        let mut document = Document::default();
        let page = page();
        document.reset_to(&page[..2]);
        document.press(key("heading"), 0, SelectionUnit::Character, false);
        // A later frame shows the last two leaves plus one below them.
        document.observe(&[
            page[1].clone(),
            page[2].clone(),
            leaf("third", "tail", 108.0),
        ]);
        document.drag_to(key("third"), 4, 4);
        assert_eq!(
            document.selected_text().as_deref(),
            Some("Two things\n\nSlow saves at idle\nThe Forge is fragile\ntail")
        );
    }

    #[test]
    fn streaming_append_keeps_the_selection_and_rewrites_clear_it() {
        let mut document = Document::default();
        document.reset_to(&page());
        document.press(key("first"), 0, SelectionUnit::Character, false);
        document.drag_to(key("first"), 4, 4);
        document.release();

        document.observe(&[leaf("first", "Slow saves at idle, even now", 44.0)]);
        assert_eq!(document.selected_text().as_deref(), Some("Slow"));

        document.observe(&[leaf("first", "Fast", 44.0)]);
        assert_eq!(document.selected_text(), None);
    }

    #[test]
    fn side_by_side_leaves_copy_with_a_tab() {
        let mut document = Document::default();
        let mut right = leaf("right", "b", 0.0);
        right.bounds.origin.x = px(400.0);
        right.order = Rc::from([1]);
        right.separator = "\t";
        document.reset_to(&[leaf("left", "a", 0.0), right]);
        document.press(key("left"), 0, SelectionUnit::Character, false);
        document.drag_to(key("right"), 1, 1);
        assert_eq!(document.selected_text().as_deref(), Some("a\tb"));
    }

    #[test]
    fn swatch_slots_copy_as_their_values_when_wholly_selected() {
        let slot = "\u{2060}\u{2003}\u{2060}";
        let text = format!("Coral ({slot}) and {slot}#333");
        let lone = 7..7 + slot.len();
        let beside_start = text.rfind(slot).expect("second slot");
        let beside = beside_start..beside_start + slot.len();
        let copies = [
            (lone.clone(), SharedString::from("#FF6B6B")),
            (beside, SharedString::default()),
        ];
        let mut whole = String::new();
        push_copied(&mut whole, &text, 0..text.len(), &copies);
        assert_eq!(whole, "Coral (#FF6B6B) and #333");
        // A selection that grazes a slot copies nothing of it.
        let mut grazed = String::new();
        push_copied(&mut grazed, &text, 0..lone.start + 3, &copies);
        assert_eq!(grazed, "Coral (");
    }
}

#[cfg(test)]
mod source_tests {
    use super::*;
    use crate::selectable_text::{SelectionLeaf, SelectionSource};
    fn source(scope: &str, texts: &[&str]) -> Rc<SelectionSource> {
        SelectionSource::new(
            scope,
            vec![0],
            texts
                .iter()
                .enumerate()
                .map(|(index, text)| SelectionLeaf {
                    order: vec![index as u64],
                    text: text.to_string().into(),
                    separator: "\n\n",
                    copies: Rc::from([]),
                })
                .collect(),
        )
    }
    #[test]
    fn visual_fragments_select_and_paint_one_registered_semantic_leaf() {
        let source = source("thread", &["alpha βeta gamma"]);
        let mut doc = Document::default();
        doc.register_source(&source, EntityId::from(1_u64));
        let mut fragment = doc.source_leaves("thread").remove(0);
        fragment.text = "βeta".into();
        fragment.source_slice = Some(6..11);
        doc.reset_with_sources(&[fragment.clone()]);
        doc.press(
            fragment.key,
            fragment.source_offset(0),
            SelectionUnit::Character,
            false,
        );
        doc.drag_to(
            fragment.key,
            fragment.source_offset(5),
            fragment.source_offset(5),
        );
        assert_eq!(doc.selected_text().as_deref(), Some("βeta"));
        assert_eq!(
            doc.paint_slice_range(fragment.key, "thread", "βeta", Some(&(6..11))),
            Some(0..5)
        );
        assert_eq!(
            doc.paint_slice_range(fragment.key, "thread", "alpha", Some(&(0..5))),
            None
        );
        doc.drag_to(fragment.key, 17, 17);
        assert_eq!(doc.selected_text().as_deref(), Some("βeta gamma"));
        // Another line of the same leaf must not overwrite the canonical source.
        fragment.text = "gamma".into();
        fragment.source_slice = Some(12..17);
        doc.observe(&[fragment.clone()]);
        assert_eq!(doc.selected_text().as_deref(), Some("βeta gamma"));
        doc.select_source(fragment);
        assert_eq!(doc.selected_text().as_deref(), Some("alpha βeta gamma"));
    }

    #[test]
    fn unregistered_fragment_does_not_pretend_to_be_the_full_source() {
        let source = source("thread", &["prefix body"]);
        let mut fragment = Document::source_observations(&source, EntityId::from(1_u64)).remove(0);
        fragment.text = "body".into();
        fragment.source_slice = Some(7..11);
        let mut doc = Document::default();
        doc.select_source(fragment);
        assert_eq!(doc.selected_text(), None);
    }

    #[test]
    fn agent_scope_copy_keeps_task_then_unpainted_children_then_result() {
        let task = SelectionSource::single("agent", vec![0, 10, 1, 0], "Assigned task", "\n\n");
        let child =
            SelectionSource::single("agent", vec![1, 25, 0], "Child work never painted", "\n\n");
        let result = SelectionSource::single("agent", vec![2, 10, 0], "Final result", "\n\n");
        let mut doc = Document::default();
        doc.register_source(&result, EntityId::from(3_u64));
        doc.register_source(&task, EntityId::from(1_u64));
        doc.register_source(&child, EntityId::from(2_u64));
        let focused = doc.source_leaves("agent").remove(0);
        doc.select_source(focused);
        assert_eq!(
            doc.selected_text().as_deref(),
            Some("Assigned task\n\nChild work never painted\n\nFinal result")
        );
    }
    #[test]
    fn ctrl_a_includes_never_painted_text_but_excludes_preview_scope() {
        let prose = source("thread", &["first", "never painted", "last"]);
        let preview = source("preview", &["separate source file"]);
        let mut doc = Document::default();
        doc.register_source(&prose, EntityId::from(1_u64));
        doc.register_source(&preview, EntityId::from(2_u64));
        let focused = doc.source_leaves("thread").remove(0);
        doc.select_source(focused);
        assert_eq!(
            doc.selected_text().as_deref(),
            Some("first\n\nnever painted\n\nlast")
        );
        drop(prose);
        assert!(
            doc.source_leaves("thread").is_empty(),
            "window registration must not retain a departed thread"
        );
        assert_eq!(doc.source_leaves("preview").len(), 1);
    }
    #[test]
    fn a_drag_across_unpainted_blocks_copies_source_order() {
        let prose = source("thread", &["alpha", "unpainted middle", "omega"]);
        let mut doc = Document::default();
        doc.register_source(&prose, EntityId::from(1_u64));
        let leaves = doc.source_leaves("thread");
        doc.reset_with_sources(&[leaves[2].clone(), leaves[0].clone()]);
        doc.press(leaves[0].key, 0, SelectionUnit::Character, false);
        doc.drag_to(leaves[2].key, 5, 5);
        assert_eq!(
            doc.selected_text().as_deref(),
            Some("alpha\n\nunpainted middle\n\nomega")
        );
    }
    #[test]
    fn releasing_a_scope_clears_copied_gesture_data_without_touching_another_scope() {
        let mut doc = Document::default();
        let source = super::super::SelectionSource::single("thread", vec![1], "thread text", "\n");
        let preview =
            super::super::SelectionSource::single("preview", vec![1], "preview text", "\n");
        let view = EntityId::from(1_u64);
        doc.register_source(&source, view);
        doc.register_source(&preview, view);
        let leaf = doc.source_leaves("thread").remove(0);
        doc.select_source(leaf);
        assert_eq!(doc.selected_text().as_deref(), Some("thread text"));
        doc.release_scope("thread");
        assert!(doc.selected_text().is_none());
        assert!(doc.leaves.is_empty());
        assert_eq!(doc.retained_bytes, 0);
        assert!(doc.source_leaves("thread").is_empty());
        assert_eq!(doc.source_leaves("preview").len(), 1);
        let preview_leaf = doc.source_leaves("preview").remove(0);
        doc.select_source(preview_leaf);
        doc.release_scope("thread");
        assert_eq!(doc.selected_text().as_deref(), Some("preview text"));
    }
    #[test]
    fn ctrl_a_survives_repainting_a_clipped_source_line() {
        let text = "long line ".repeat(4_000);
        let source = source("preview", &[&text]);
        let mut doc = Document::default();
        doc.register_source(&source, EntityId::from(1_u64));
        let mut visible = doc.source_leaves("preview").remove(0);
        visible.text = text[..16 * 1024].to_owned().into();
        let key = visible.key;
        doc.select_source(visible.clone());
        doc.observe_in_frame(visible.clone());
        doc.flush_frame();
        assert_eq!(doc.selected_text().as_deref(), Some(text.as_str()));
        assert_eq!(
            doc.paint_range(key, "preview", &visible.text),
            Some(0..visible.text.len())
        );
        assert_eq!(doc.paint_range(key, "thread", &visible.text), None);
        assert!(doc.has_selection());
    }

    #[test]
    fn ctrl_a_copies_a_source_larger_than_the_gesture_budget() {
        let text = "x".repeat(3 * 1024 * 1024);
        let source = source("thread", &[&text]);
        let mut doc = Document::default();
        doc.register_source(&source, EntityId::from(1_u64));
        let focused = doc.source_leaves("thread").remove(0);
        doc.select_source(focused);
        assert!(doc.is_active() && doc.has_selection());
        assert!(
            !doc.painted().is_empty(),
            "source owner receives invalidation"
        );
        assert_eq!(doc.selected_text().as_deref(), Some(text.as_str()));
    }
    #[test]
    fn dragging_across_a_clipped_line_copies_the_registered_source() {
        let text = "long line ".repeat(4_000);
        let source = source("preview", &[&text, "last"]);
        let mut doc = Document::default();
        doc.register_source(&source, EntityId::from(1_u64));
        let mut visible = doc.source_leaves("preview");
        visible[0].text = text[..16 * 1024].to_owned().into();
        doc.reset_with_sources(&visible);
        doc.press(visible[0].key, 0, SelectionUnit::Character, false);
        doc.drag_to(visible[1].key, 4, 4);
        for leaf in &visible {
            doc.observe_in_frame(leaf.clone());
        }
        doc.flush_frame();
        assert_eq!(doc.selected_text(), Some(format!("{text}\n\nlast")));
    }

    #[test]
    fn a_registered_content_rewrite_invalidates_an_existing_drag_selection() {
        let original = source("thread", &["original source"]);
        let mut doc = Document::default();
        doc.register_source(&original, EntityId::from(1_u64));
        let visible = doc.source_leaves("thread");
        doc.reset_with_sources(&visible);
        doc.press(visible[0].key, 0, SelectionUnit::Character, false);
        doc.drag_to(visible[0].key, 8, 8);
        assert_eq!(doc.selected_text().as_deref(), Some("original"));
        drop(original);
        let replacement = source("thread", &["rewritten source"]);
        doc.register_source(&replacement, EntityId::from(1_u64));
        assert_eq!(doc.selected_text(), None);
    }
    #[test]
    fn shorter_source_revision_clears_selection_in_a_removed_leaf() {
        let original = source("thread", &["first", "removed paragraph"]);
        let mut doc = Document::default();
        doc.register_source(&original, EntityId::from(1_u64));
        let visible = doc.source_leaves("thread");
        doc.reset_with_sources(&visible);
        doc.press(visible[1].key, 0, SelectionUnit::Character, false);
        doc.drag_to(visible[1].key, visible[1].text.len(), visible[1].text.len());
        assert_eq!(doc.selected_text().as_deref(), Some("removed paragraph"));
        // The old renderer can remain alive until the end of a frame. Owner
        // identity, not Rc lifetime, determines which revision replaces it.
        let replacement = source("thread", &["first"]);
        doc.register_source(&replacement, EntityId::from(1_u64));
        assert_eq!(doc.selected_text(), None);
        assert_eq!(doc.source_leaves("thread").len(), 1);
    }

    #[test]
    fn empty_source_revision_clears_selection_but_eviction_does_not() {
        let original = source("thread", &["keep while evicted"]);
        let mut doc = Document::default();
        doc.register_source(&original, EntityId::from(1_u64));
        let visible = doc.source_leaves("thread");
        doc.reset_with_sources(&visible);
        doc.press(visible[0].key, 0, SelectionUnit::Character, false);
        doc.drag_to(visible[0].key, 4, 4);
        drop(original);
        let other = SelectionSource::single("thread", vec![99], "another item", "\n");
        doc.register_source(&other, EntityId::from(2_u64));
        assert_eq!(doc.selected_text().as_deref(), Some("keep"));
        let empty = source("thread", &[]);
        doc.register_source(&empty, EntityId::from(1_u64));
        assert_eq!(doc.selected_text(), None);
    }

    #[test]
    fn appended_source_revision_preserves_the_selected_prefix() {
        let original = source("thread", &["selected text"]);
        let mut doc = Document::default();
        doc.register_source(&original, EntityId::from(1_u64));
        let visible = doc.source_leaves("thread");
        doc.reset_with_sources(&visible);
        doc.press(visible[0].key, 0, SelectionUnit::Character, false);
        doc.drag_to(visible[0].key, 8, 8);
        let appended = source("thread", &["selected text and more", "new paragraph"]);
        doc.register_source(&appended, EntityId::from(1_u64));
        assert_eq!(doc.selected_text().as_deref(), Some("selected"));
    }
}
