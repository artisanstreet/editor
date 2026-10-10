//! One text selection per window, spanning every selectable leaf.
//!
//! A browser keeps exactly one selection per document, and dragging from
//! one paragraph into the next simply extends it. Transcript text is split
//! into many [`SelectableText`](super::SelectableText) leaves (paragraphs,
//! list items, code blocks, table cells), so the selection lives here, per
//! window, and each leaf paints the part of it that falls inside its text.
//!
//! Endpoints are `(leaf, byte offset)` pairs. Comparing endpoints in two
//! leaves needs document order, which GPUI does not expose, so the document
//! learns it from paint order: leaves report themselves in paint order when
//! they prepaint ([`Document::observe_frame`]) and, because cached rows
//! replay their listeners without prepainting, again from their capture-
//! phase mouse listeners on every event of a gesture
//! ([`Document::check_in`]). Each report is merged into a persistent order,
//! so leaves that scrolled out of the window (or out of the transcript's
//! build window) keep their place and their text for copying.
//!
//! The order and leaf table are rebuilt from the live leaves whenever a new
//! selection starts and are dropped when the selection clears, so the
//! document holds nothing while nothing is selected.

use std::collections::HashMap;
use std::hash::{Hash as _, Hasher as _};
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    App, Bounds, EntityId, FocusHandle, Global, GlobalElementId, Pixels, Point, SharedString,
    TextLayout, WindowId,
};

use crate::text_navigation::SelectionUnit;

use super::{SelectableLinkHandler, clamp_to_char_boundary};

/// Identity of one selectable leaf: a hash of its stable global element id.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct LeafKey(u64);

impl LeafKey {
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
    pub(super) text: SharedString,
    /// Ranges of `text` that copy as other text: a color swatch slot copies
    /// as the value it stands for, or as nothing beside a shown value.
    pub(super) copies: Rc<[(Range<usize>, SharedString)]>,
    pub(super) view: EntityId,
    pub(super) bounds: Bounds<Pixels>,
    pub(super) line_height: Pixels,
}

/// A leaf live in the rendered frame, as its listener reports it during
/// one pointer event's capture phase.
#[derive(Clone)]
pub(super) struct LiveLeaf {
    pub(super) observed: Observed,
    pub(super) layout: TextLayout,
    pub(super) hovered: bool,
    pub(super) focus: Option<FocusHandle>,
    pub(super) links: Rc<[Range<usize>]>,
    pub(super) on_link: Option<SelectableLinkHandler>,
}

/// What the document remembers about a leaf.
struct Leaf {
    text: SharedString,
    /// See [`Observed::copies`].
    copies: Rc<[(Range<usize>, SharedString)]>,
    view: EntityId,
    /// Text placed between the previous leaf and this one when copying.
    separator: &'static str,
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
    leaves: HashMap<LeafKey, Leaf>,
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
    cx.default_global::<Documents>()
        .0
        .entry(window)
        .or_default()
}

impl Document {
    /// Whether a selection or a gesture is live, so leaves must report in.
    pub(super) fn is_active(&self) -> bool {
        self.anchor.is_some() || self.drag.is_some()
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
        if self.is_active() {
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
        self.leaves.clear();
        self.order.clear();
        self.index.clear();
        self.observe(live);
    }

    /// Merges leaves seen in paint order into the persistent order and
    /// refreshes their text, view, and copy separator.
    pub(super) fn observe(&mut self, seen: &[Observed]) {
        let mut previous: Option<&Observed> = None;
        let mut insert_at: Option<usize> = None;
        for (position, observed) in seen.iter().enumerate() {
            let separator = previous.map_or("\n\n", |previous| separator(previous, observed));
            if let Some(index) = self.index.get(&observed.key).copied() {
                insert_at = Some(index + 1);
                if !self.refresh_leaf(observed, separator) {
                    // A rewrite cleared the selection and everything learned.
                    return;
                }
            } else {
                let at = insert_at.unwrap_or_else(|| self.insertion_before(&seen[position..]));
                self.order.insert(at, observed.key);
                self.reindex(at);
                self.leaves.insert(
                    observed.key,
                    Leaf {
                        text: observed.text.clone(),
                        copies: Rc::clone(&observed.copies),
                        view: observed.view,
                        separator,
                    },
                );
                insert_at = Some(at + 1);
            }
            previous = Some(observed);
        }
    }

    /// Where a run of leaves with no known predecessor goes: before the
    /// first of them the order already knows, else at the end.
    fn insertion_before(&self, run: &[Observed]) -> usize {
        run.iter()
            .find_map(|observed| self.index.get(&observed.key).copied())
            .unwrap_or(self.order.len())
    }

    fn reindex(&mut self, from: usize) {
        for (index, key) in self.order.iter().enumerate().skip(from) {
            self.index.insert(*key, index);
        }
    }

    /// Updates a known leaf. Text that changed under the selection keeps
    /// the selection only while the selected prefix survives (a streaming
    /// append); any other rewrite clears it. Returns whether the selection
    /// survived.
    fn refresh_leaf(&mut self, observed: &Observed, separator: &'static str) -> bool {
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
        leaf.text = observed.text.clone();
        leaf.copies = Rc::clone(&observed.copies);
        leaf.view = observed.view;
        leaf.separator = separator;
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

    /// Every leaf's selected range, keyed by the view that paints it.
    pub(super) fn painted(&self) -> Vec<(EntityId, LeafKey, Range<usize>)> {
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
        self.ordered().is_some_and(|(start, end)| start != end)
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
        let range = unit.range_at(&text, offset);
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
        let range = drag.unit.drag_range_at(&text, offset);
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

    /// Selects all of one leaf (`ctrl`+A while it has focus).
    pub(super) fn select_leaf(&mut self, observed: Observed) {
        let (leaf, len) = (observed.key, observed.text.len());
        self.reset_to(&[observed]);
        self.drag = None;
        self.anchor = Some(Position { leaf, offset: 0 });
        self.head = Some(Position { leaf, offset: len });
    }

    /// Drops the selection, any gesture, and everything learned for them.
    pub(super) fn clear(&mut self) {
        self.anchor = None;
        self.head = None;
        self.drag = None;
        self.leaves.clear();
        self.order.clear();
        self.index.clear();
    }
}

/// What a browser puts between two blocks when copying: a tab between
/// blocks on one row (table cells), a blank line across a paragraph gap,
/// and a line break between tightly stacked blocks (list items, lines).
fn separator(previous: &Observed, next: &Observed) -> &'static str {
    let same_row = next.bounds.top() < previous.bounds.bottom() - gpui::px(1.0)
        && next.bounds.left() >= previous.bounds.right() - gpui::px(1.0);
    if same_row {
        return "\t";
    }
    let gap = next.bounds.top() - previous.bounds.bottom();
    if gap >= next.line_height * 0.3 {
        "\n\n"
    } else {
        "\n"
    }
}

#[cfg(test)]
mod tests {
    use std::hash::{Hash as _, Hasher as _};

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
            text: SharedString::from(text.to_owned()),
            copies: std::rc::Rc::from([]),
            view: EntityId::from(1_u64),
            bounds: Bounds::new(point(px(0.0), px(top)), size(px(400.0), px(28.0))),
            line_height: px(28.0),
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
        assert!(word == "saves" || word == "saves ", "{word:?}");
        document.drag_to(key("first"), 15, 15);
        assert_eq!(document.selected_text().as_deref(), Some("saves at idle"));
        document.drag_to(key("first"), 1, 1);
        assert_eq!(document.selected_text().as_deref(), Some("Slow saves"));
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
