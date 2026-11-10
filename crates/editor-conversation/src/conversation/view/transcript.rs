use crate::conversation::controller::Conversation;
use crate::conversation::fold::RowKey;
use crate::conversation::fold::{Row, Slot};
use crate::conversation::presentation::{
    BodyKey, MarkdownOwners, PresentationScope, ProjectedProof,
};
use aqua_ui::collection::List;
use aqua_ui::collection_runtime::CollectionViewport;
use aqua_ui::element::Element;
use aqua_ui::style::{Length, Style};
use artisan_api::Seq;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone)]
pub struct RowSnapshot {
    pub row: Row,
    pub version: u64,
    pub item_seq: Option<Seq>,
    pub source: Option<Rc<str>>,
    pub active: Rc<std::cell::Cell<bool>>,
    pub revealed: bool,
    pub reveal_ready: Option<Rc<std::cell::Cell<bool>>>,
    pub live: Option<crate::conversation::live::LiveBinding>,
    pub stored: Option<(
        std::rc::Weak<RefCell<crate::conversation::stored::StoredConversation>>,
        crate::conversation::stored::StoredProof,
    )>,
}

pub struct HeightObservation {
    pub key: RowKey,
    pub version: u64,
    pub height: f32,
    pub active: Rc<std::cell::Cell<bool>>,
}

pub struct ViewportObservation {
    pub viewport: CollectionViewport,
    pub active: Rc<std::cell::Cell<bool>>,
}

#[derive(Clone)]
pub struct PaintedRow {
    pub snapshot: Rc<RowSnapshot>,
    pub bounds: aqua_core::Rect,
    pub proof: Option<ProjectedProof>,
    #[cfg(feature = "qualification")]
    pub serial: u64,
}

#[derive(Default)]
pub struct ObservationEvents {
    pub viewports: Vec<ViewportObservation>,
    pub heights: Vec<HeightObservation>,
    pub painted: Vec<Rc<PaintedRow>>,
}

#[derive(Default)]
struct ObservationState {
    events: ObservationEvents,
    painted: std::collections::HashMap<RowKey, Rc<PaintedRow>>,
    key_hashes: std::collections::HashMap<RowKey, aqua_ui::tree::KeyHash>,
    keys_epoch: Option<u64>,
    row_keys: Vec<RowKey>,
    current_versions: std::collections::HashMap<RowKey, u64>,
    reader_moved: bool,
    reader_moves: u64,
    reader_toward_end: Option<bool>,
    #[cfg(feature = "qualification")]
    paint_serial: u64,
}

impl ObservationState {
    fn sync_keys(&mut self, epoch: u64, rows: &[(Row, u64)]) {
        if self.keys_epoch == Some(epoch) {
            return;
        }
        self.keys_epoch = Some(epoch);
        self.row_keys = rows.iter().map(|(row, _)| row.key()).collect();
        self.current_versions = self
            .row_keys
            .iter()
            .zip(rows)
            .map(|(key, (_, version))| (key.clone(), *version))
            .collect();
    }
}

#[derive(Clone, Default)]
pub struct TranscriptObservations(Rc<RefCell<ObservationState>>);

impl TranscriptObservations {
    #[cfg(feature = "qualification")]
    pub fn qualification_paint_serial(&self) -> u64 {
        self.0.borrow().paint_serial
    }
    pub fn clear(&self) {
        let mut state = self.0.borrow_mut();
        #[cfg(feature = "qualification")]
        let paint_serial = state.paint_serial;
        *state = ObservationState::default();
        #[cfg(feature = "qualification")]
        {
            state.paint_serial = paint_serial;
        }
    }

    pub fn note_reader_move(&self) {
        self.0.borrow_mut().reader_moved = true;
    }

    pub fn note_reader_direction(&self, toward_end: bool) {
        self.0.borrow_mut().reader_toward_end = Some(toward_end);
    }

    pub fn prefetch_stored(
        &self,
        conversation: &Conversation,
        core: &editor_core::Core,
        scope: &PresentationScope,
        viewport: CollectionViewport,
        clip: aqua_core::Rect,
        painted: &[Rc<PaintedRow>],
        following_tail: bool,
    ) -> bool {
        if !scope.current(conversation, core) {
            return false;
        }
        let state = self.0.borrow();
        let top = conversation
            .rows
            .get(viewport.visible.start)
            .map(|(row, _)| row.key());
        let following = conversation
            .view
            .as_ref()
            .is_some_and(|view| view.viewport.state().is_following());
        let mut changed = false;
        for observation in painted {
            let snapshot = &observation.snapshot;
            if !snapshot.active.get()
                || !snapshot.revealed
                || snapshot
                    .reveal_ready
                    .as_ref()
                    .is_some_and(|ready| !ready.get())
                || observation.bounds.h <= 0.0
                || observation.bounds.intersect(clip).is_empty()
            {
                continue;
            }
            let Some((weak, proof)) = &snapshot.stored else {
                continue;
            };
            let Some(owner) = weak.upgrade() else {
                continue;
            };
            let Some((row, _)) = conversation
                .row_position(&snapshot.row.key())
                .and_then(|index| conversation.rows.get(index))
                .filter(|(_, version)| *version == snapshot.version)
            else {
                continue;
            };
            let Some(item) = row
                .anchor()
                .and_then(|id| conversation.view.as_ref()?.item(id))
            else {
                continue;
            };
            if snapshot.item_seq != Some(item.seq) {
                continue;
            }
            changed |= owner.borrow_mut().prefetch(
                proof,
                scope,
                row,
                item,
                top.clone(),
                state.reader_moves,
                state.reader_toward_end,
                following_tail,
                following,
            );
        }
        changed
    }
    pub fn viewport_callback(
        &self,
        active: Rc<std::cell::Cell<bool>>,
    ) -> Rc<dyn Fn(CollectionViewport)> {
        let events = self.clone();
        Rc::new(move |viewport| {
            if active.get() {
                events
                    .0
                    .borrow_mut()
                    .events
                    .viewports
                    .push(ViewportObservation {
                        viewport,
                        active: active.clone(),
                    });
            }
        })
    }

    pub fn callbacks(
        &self,
        snapshot: RowSnapshot,
        markdown: Rc<RefCell<MarkdownOwners>>,
    ) -> (Rc<dyn Fn(aqua_core::Rect)>, Rc<dyn Fn(aqua_core::Rect)>) {
        let snapshot = Rc::new(snapshot);
        let height_events = self.clone();
        let height_key = snapshot.row.key();
        let version = snapshot.version;
        let height_active = snapshot.active.clone();
        let height = Rc::new(move |bounds: aqua_core::Rect| {
            height_events
                .0
                .borrow_mut()
                .events
                .heights
                .push(HeightObservation {
                    key: height_key.clone(),
                    version,
                    height: bounds.h,
                    active: height_active.clone(),
                });
        });
        let paint_events = self.clone();
        let paint_key = snapshot.row.key();
        let painted = Rc::new(move |bounds| {
            let proof = match snapshot.row.semantic() {
                Row::Text {
                    id,
                    slot: Slot::Prose,
                    block,
                    ..
                } => markdown.borrow().capture_projected(
                    &BodyKey {
                        item: id.clone(),
                        slot: Slot::Prose,
                    },
                    *block,
                ),
                _ => None,
            };
            let mut state = paint_events.0.borrow_mut();
            #[cfg(feature = "qualification")]
            {
                state.paint_serial = state.paint_serial.saturating_add(1);
            }
            let painted = Rc::new(PaintedRow {
                snapshot: Rc::clone(&snapshot),
                bounds,
                proof,
                #[cfg(feature = "qualification")]
                serial: state.paint_serial,
            });
            match state.painted.get_mut(&paint_key) {
                Some(slot) => *slot = Rc::clone(&painted),
                None => {
                    state.painted.insert(paint_key.clone(), Rc::clone(&painted));
                }
            }
            state.events.painted.push(painted);
        });
        (height, painted)
    }

    pub fn drain(&self) -> ObservationEvents {
        std::mem::take(&mut self.0.borrow_mut().events)
    }

    pub fn observe_viewport(
        &self,
        conversation: &mut Conversation,
        core: &editor_core::Core,
        app: &aqua::UiApp,
        markdown: &MarkdownOwners,
        scope: &PresentationScope,
        viewport: CollectionViewport,
        clip: aqua_core::Rect,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            scope.current(conversation, core),
            "stale conversation presentation scope"
        );
        conversation.observe_visible_rows(
            viewport.visible.start..viewport.visible.end,
            viewport.mounted.start..viewport.mounted.end,
        );
        let source_prefix = conversation
            .rows
            .first()
            .is_some_and(|(row, _)| row.visual().is_some_and(|visual| visual > 0)
                || matches!(row.semantic(), Row::Text { id, slot, block, .. } if *block > 0 && conversation.view.as_ref().and_then(|view| view.item(id)).and_then(|item| slot.text(item)).is_some_and(|text| matches!(text, artisan_api::TextContent::Stored(_)))));
        let following = conversation
            .view
            .as_ref()
            .is_some_and(|view| view.viewport.state().is_following());
        let revealed_count = conversation
            .reveal_frontier
            .as_ref()
            .and_then(|frontier| conversation.row_position(frontier).map(|index| index + 1))
            .unwrap_or(conversation.rows.len());
        let mut state = self.0.borrow_mut();
        state.sync_keys(conversation.rows_epoch(), &conversation.rows);
        let ObservationState {
            painted,
            current_versions,
            ..
        } = &mut *state;
        painted.retain(|key, painted| {
            painted.snapshot.active.get()
                && current_versions.get(key) == Some(&painted.snapshot.version)
        });
        let is_ready = |row: &Row, _version: u64| {
            crate::conversation::read::evidence(conversation, core, row, |row, item| {
                if let Some((owner, proof)) = state
                    .painted
                    .get(&row.key())
                    .and_then(|painted| painted.snapshot.stored.as_ref())
                {
                    return owner
                        .upgrade()
                        .is_some_and(|owner| proof.accepts(&owner.borrow(), scope, row, item));
                }
                let Row::Text {
                    id,
                    slot: Slot::Prose,
                    block,
                    ..
                } = row.semantic()
                else {
                    return false;
                };
                let artisan_api::ItemKind::AssistantMessage {
                    text: artisan_api::TextContent::Inline(source),
                    ..
                } = &item.kind
                else {
                    return false;
                };
                markdown.prepared_body_current(
                    scope,
                    &BodyKey {
                        item: id.clone(),
                        slot: Slot::Prose,
                    },
                    source,
                    item.seq,
                    *block,
                )
            })
            .is_some()
        };
        let last_body_ready = conversation.rows[..revealed_count.min(conversation.rows.len())]
            .iter()
            .rev()
            .find_map(|(row, version)| {
                matches!(row, Row::Text { .. }).then(|| is_ready(row, *version))
            })
            .unwrap_or(true);
        {
            let ObservationState {
                key_hashes,
                current_versions,
                ..
            } = &mut *state;
            if key_hashes.len() > current_versions.len().saturating_mul(2) {
                key_hashes.retain(|key, _| current_versions.contains_key(key));
            }
        }
        let mounted = viewport.mounted.start.min(conversation.rows.len())
            ..viewport.mounted.end.min(conversation.rows.len());
        let mut bounds = std::collections::HashMap::new();
        for index in mounted.clone() {
            let key = &state.row_keys[index];
            let hash = match state.key_hashes.get(key) {
                Some(hash) => *hash,
                None => {
                    let hash = aqua_ui::tree::hash_key(aqua_ui::element::Key::from(format!(
                        "{key:?}"
                    )));
                    let key = key.clone();
                    state.key_hashes.insert(key, hash);
                    hash
                }
            };
            if let Some(rect) = app
                .tree()
                .last_node_with_key(hash)
                .and_then(|node| app.tree().visual_rect(node))
            {
                bounds.insert(index, rect);
            }
        }
        let near = |index: usize, reach: f32| {
            bounds.get(&index).is_some_and(|rect| {
                rect.bottom() > clip.y - reach && rect.y < clip.bottom() + reach
            })
        };
        let preparing = |index: usize| {
            conversation.rows.get(index).is_none_or(|(_, version)| {
                state
                    .row_keys
                    .get(index)
                    .and_then(|key| state.painted.get(key))
                    .is_none_or(|painted| {
                        !painted.snapshot.active.get() || painted.snapshot.version != *version
                    })
            })
        };
        let preparing_rows: Vec<bool> = mounted.clone().map(preparing).collect();
        let mounted_start = mounted.start;
        let reader_moved = std::mem::take(&mut state.reader_moved);
        if reader_moved {
            state.reader_moves = state.reader_moves.saturating_add(1);
        }
        drop(state);
        if reader_moved {
            reader_scrolled(conversation, core, viewport, source_prefix, revealed_count);
        }
        observer(
            conversation,
            core,
            viewport,
            following,
            last_body_ready,
            source_prefix,
            near,
            |index| {
                index
                    .checked_sub(mounted_start)
                    .and_then(|offset| preparing_rows.get(offset))
                    .copied()
                    .unwrap_or(true)
            },
        );
        Ok(())
    }
}

pub struct ObservedRead {
    candidate: crate::conversation::read::ReadCandidate,
    snapshot: Rc<RowSnapshot>,
    scope: PresentationScope,
    proof: ReadProof,
}

enum ReadProof {
    Inline(ProjectedProof),
    Stored(
        std::rc::Weak<RefCell<crate::conversation::stored::StoredConversation>>,
        crate::conversation::stored::StoredProof,
    ),
}

fn prepared_snapshot(
    row: &Row,
    item: &artisan_api::Item,
    snapshot: &RowSnapshot,
    scope: &PresentationScope,
    proof: &ReadProof,
    markdown: &MarkdownOwners,
    live_current: bool,
) -> bool {
    if !snapshot.active.get()
        || !snapshot
            .reveal_ready
            .as_ref()
            .is_none_or(|ready| ready.get())
        || snapshot.item_seq != Some(item.seq)
    {
        return false;
    }
    let ReadProof::Inline(proof) = proof else {
        let ReadProof::Stored(owner, proof) = proof else {
            return false;
        };
        return owner
            .upgrade()
            .is_some_and(|owner| proof.accepts(&owner.borrow(), scope, row, item));
    };
    let Some(source) = snapshot.source.as_deref() else {
        return false;
    };
    snapshot.active.get()
        && snapshot
            .reveal_ready
            .as_ref()
            .is_none_or(|ready| ready.get())
        && matches!(row.semantic(), Row::Text { id, slot: Slot::Prose, .. } if *id == item.id)
        && snapshot.item_seq == Some(item.seq)
        && (if snapshot.live.is_some() {
            live_current
        } else {
            matches!(&item.kind, artisan_api::ItemKind::AssistantMessage { text: artisan_api::TextContent::Inline(text), .. } if text == source)
        })
        && markdown.accepts_projected(scope, proof, source)
}

impl PaintedRow {
    pub fn capture(
        &self,
        conversation: &Conversation,
        core: &editor_core::Core,
        markdown: &MarkdownOwners,
        scope: &PresentationScope,
        clip: aqua_core::Rect,
        window_active: bool,
    ) -> Option<ObservedRead> {
        if !self.snapshot.active.get()
            || !scope.current(conversation, core)
            || !crate::conversation::read::painted(
                self.bounds,
                clip,
                window_active,
                self.snapshot.revealed,
            )
        {
            return None;
        }
        let proof = match (self.snapshot.stored.clone(), self.proof.clone()) {
            (Some((owner, proof)), _) => ReadProof::Stored(owner, proof),
            (None, Some(proof)) => ReadProof::Inline(proof),
            (None, None) => return None,
        };
        let key = self.snapshot.row.key();
        let (row, version) = conversation.rows.get(conversation.row_position(&key)?)?;
        if *version != self.snapshot.version {
            return None;
        }
        let live_current = self
            .snapshot
            .live
            .as_ref()
            .is_none_or(|live| live.complete(conversation, core));
        let candidate =
            crate::conversation::read::capture(conversation, core, row, |row, item| {
                prepared_snapshot(
                    row,
                    item,
                    &self.snapshot,
                    scope,
                    &proof,
                    markdown,
                    live_current,
                )
            })?;
        Some(ObservedRead {
            candidate,
            snapshot: Rc::clone(&self.snapshot),
            scope: scope.clone(),
            proof,
        })
    }
}

impl ObservedRead {
    #[cfg(feature = "qualification")]
    pub fn observed_seq(&self) -> artisan_api::Seq {
        self.candidate.observed_seq()
    }

    pub fn acknowledge(
        self,
        conversation: &Conversation,
        core: &mut editor_core::Core,
        markdown: &MarkdownOwners,
    ) -> bool {
        if !self.snapshot.active.get()
            || !self.scope.current(conversation, core)
            || !conversation
                .row_position(&self.snapshot.row.key())
                .and_then(|index| conversation.rows.get(index))
                .is_some_and(|(_, version)| *version == self.snapshot.version)
        {
            return false;
        }
        let live_current = self
            .snapshot
            .live
            .as_ref()
            .is_none_or(|live| live.complete(conversation, core));
        crate::conversation::read::acknowledge(self.candidate, conversation, core, |row, item| {
            prepared_snapshot(
                row,
                item,
                &self.snapshot,
                &self.scope,
                &self.proof,
                markdown,
                live_current,
            )
        })
    }
}

pub fn row_key_hash(key: &RowKey) -> aqua_ui::tree::KeyHash {
    const LIMIT: usize = 65_536;
    thread_local! {
        static HASHES: RefCell<std::collections::HashMap<RowKey, aqua_ui::tree::KeyHash>> =
            RefCell::new(std::collections::HashMap::new());
    }
    HASHES.with(|hashes| {
        let mut hashes = hashes.borrow_mut();
        if let Some(hash) = hashes.get(key) {
            return *hash;
        }
        if hashes.len() >= LIMIT {
            hashes.clear();
        }
        let hash = aqua_ui::tree::hash_key(aqua_ui::element::Key::from(format!("{key:?}")));
        hashes.insert(key.clone(), hash);
        hash
    })
}

pub fn apply_heights(
    conversation: &mut Conversation,
    heights: impl IntoIterator<Item = HeightObservation>,
) -> bool {
    let mut changed = false;
    for HeightObservation {
        key,
        version,
        height,
        active,
    } in heights
    {
        if !active.get()
            || !height.is_finite()
            || height < 0.0
            || conversation
                .row_position(&key)
                .and_then(|index| conversation.rows.get(index))
                .is_none_or(|(_, current)| *current != version)
        {
            continue;
        }
        if conversation.row_heights.get(&key) != Some(&height) {
            conversation.row_heights.insert(key, height);
            changed = true;
        }
    }
    changed
}

#[derive(Clone)]
struct TranscriptRow {
    index: usize,
    key: RowKey,
}

pub fn list(
    conversation: &Conversation,
    estimated_height: f32,
    row: Rc<dyn Fn(usize, &RowKey) -> Element>,
    viewport: Rc<dyn Fn(CollectionViewport)>,
    initial_height: Rc<dyn Fn(usize, &RowKey) -> f32>,
) -> Element {
    let rows: Vec<_> = conversation
        .rows
        .iter()
        .enumerate()
        .map(|(index, (row, _))| TranscriptRow {
            index,
            key: row.key(),
        })
        .collect();
    let heights = rows.clone();
    List::new()
        .items(rows)
        .key(|row: &TranscriptRow| format!("{:?}", row.key))
        .row(move |entry: &TranscriptRow| row(entry.index, &entry.key))
        .virtualize()
        .estimated_row_height(Length::Px(estimated_height))
        .overscan(0)
        .overscan_pixels(transcript_overscan_pixels(conversation))
        .initial_height(move |index| initial_height(index, &heights[index].key))
        .on_viewport(move |event| viewport(event))
        .style(
            Style::new()
                .width(Length::Pct(100.0))
                .height(Length::Pct(100.0)),
        )
        .build()
        .with_key("conversation-transcript")
}

pub type PreparedRowFactory = Rc<
    dyn Fn(
        usize,
        &RowKey,
    ) -> Result<
        aqua_ui::collection_runtime::PreparedCollectionRow,
        aqua_ui::collection_runtime::CollectionError,
    >,
>;

pub type BatchedRowFactory = Rc<
    dyn Fn(
        usize,
        &RowKey,
    ) -> Result<
        aqua_ui::collection::BatchedCollectionRow,
        aqua_ui::collection_runtime::CollectionError,
    >,
>;

pub type LayoutBatchedRowFactory = Rc<
    dyn Fn(
        usize,
        &RowKey,
        &aqua_ui::collection_runtime::CollectionLayout,
    ) -> Result<
        aqua_ui::collection::BatchedCollectionRow,
        aqua_ui::collection_runtime::CollectionError,
    >,
>;

pub type RowRevision = Rc<dyn Fn(usize, &RowKey) -> Option<u64>>;

const TRANSCRIPT_MAX_WIDTH: f32 = 768.0;

pub fn layout_batched_list(
    conversation: &Conversation,
    estimated_height: f32,
    row: LayoutBatchedRowFactory,
    revision: RowRevision,
    viewport: Rc<dyn Fn(CollectionViewport)>,
    initial_height: Rc<dyn Fn(usize, &RowKey) -> f32>,
) -> Element {
    let rows: Vec<_> = conversation
        .rows
        .iter()
        .enumerate()
        .map(|(index, (row, _))| TranscriptRow {
            index,
            key: row.key(),
        })
        .collect();
    let heights = rows.clone();
    let epoch = {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        conversation.rows_epoch().hash(&mut hasher);
        rows.len().hash(&mut hasher);
        rows.first().map(|row| &row.key).hash(&mut hasher);
        rows.last().map(|row| &row.key).hash(&mut hasher);
        hasher.finish()
    };
    List::new()
        .items(rows)
        .shape_epoch(epoch)
        .key(|row: &TranscriptRow| format!("{:?}", row.key))
        .revision(move |entry: &TranscriptRow| revision(entry.index, &entry.key))
        .try_batched_row_with_layout(move |entry: &TranscriptRow, layout| {
            row(entry.index, &entry.key, layout)
        })
        .width_class(|width| width.min(TRANSCRIPT_MAX_WIDTH))
        .virtualize()
        .estimated_row_height(Length::Px(estimated_height))
        .overscan(0)
        .overscan_pixels(transcript_overscan_pixels(conversation))
        .initial_height(move |index| initial_height(index, &heights[index].key))
        .on_viewport(move |event| viewport(event))
        .style(
            Style::new()
                .width(Length::Pct(100.0))
                .height(Length::Pct(100.0)),
        )
        .build()
        .with_key("conversation-transcript")
}

pub fn batched_list(
    conversation: &Conversation,
    estimated_height: f32,
    row: BatchedRowFactory,
    viewport: Rc<dyn Fn(CollectionViewport)>,
    initial_height: Rc<dyn Fn(usize, &RowKey) -> f32>,
) -> Element {
    let rows: Vec<_> = conversation
        .rows
        .iter()
        .enumerate()
        .map(|(index, (row, _))| TranscriptRow {
            index,
            key: row.key(),
        })
        .collect();
    let heights = rows.clone();
    List::new()
        .items(rows)
        .key(|row: &TranscriptRow| format!("{:?}", row.key))
        .try_batched_row(move |entry: &TranscriptRow| row(entry.index, &entry.key))
        .virtualize()
        .estimated_row_height(Length::Px(estimated_height))
        .overscan(0)
        .overscan_pixels(transcript_overscan_pixels(conversation))
        .initial_height(move |index| initial_height(index, &heights[index].key))
        .on_viewport(move |event| viewport(event))
        .style(
            Style::new()
                .width(Length::Pct(100.0))
                .height(Length::Pct(100.0)),
        )
        .build()
        .with_key("conversation-transcript")
}

pub fn try_list(
    conversation: &Conversation,
    estimated_height: f32,
    row: PreparedRowFactory,
    viewport: Rc<dyn Fn(CollectionViewport)>,
    initial_height: Rc<dyn Fn(usize, &RowKey) -> f32>,
) -> Element {
    let rows: Vec<_> = conversation
        .rows
        .iter()
        .enumerate()
        .map(|(index, (row, _))| TranscriptRow {
            index,
            key: row.key(),
        })
        .collect();
    let heights = rows.clone();
    List::new()
        .items(rows)
        .key(|row: &TranscriptRow| format!("{:?}", row.key))
        .try_row(move |entry: &TranscriptRow| row(entry.index, &entry.key))
        .virtualize()
        .estimated_row_height(Length::Px(estimated_height))
        .overscan(0)
        .overscan_pixels(transcript_overscan_pixels(conversation))
        .initial_height(move |index| initial_height(index, &heights[index].key))
        .on_viewport(move |event| viewport(event))
        .style(
            Style::new()
                .width(Length::Pct(100.0))
                .height(Length::Pct(100.0)),
        )
        .build()
        .with_key("conversation-transcript")
}

pub fn reader_scrolled(
    conversation: &mut Conversation,
    core: &editor_core::Core,
    viewport: CollectionViewport,
    source_prefix: bool,
    revealed_count: usize,
) {
    if let Some(position) = conversation
        .view
        .as_ref()
        .and_then(|view| view.items.values().map(|item| item.position).max())
    {
        conversation
            .reveal
            .borrow_mut()
            .rush(position.saturating_add(1));
    }
    let maximum = viewport.distance_from_start + viewport.distance_from_end;
    let gap_shown = crate::conversation::scroll::gap_visible(
        &conversation.rows,
        revealed_count,
        viewport.visible.end < conversation.rows.len(),
    );
    let inputs = crate::conversation::scroll::scrolled(
        conversation,
        viewport.visible.start,
        viewport.viewport_height,
        -viewport.scroll_y,
        maximum,
        source_prefix,
        gap_shown,
        |index| index >= viewport.visible.end,
    );
    for input in inputs {
        conversation.viewport(core, input);
    }
}

pub fn observer(
    conversation: &mut Conversation,
    core: &editor_core::Core,
    viewport: CollectionViewport,
    following: bool,
    last_body_ready: bool,
    source_prefix: bool,
    mut near: impl FnMut(usize, f32) -> bool,
    mut preparing: impl FnMut(usize) -> bool,
) {
    let Some(view) = conversation.view.as_ref() else {
        return;
    };
    let landing = conversation.landing.is_some();
    let more = (!following || last_body_ready)
        && !view.viewport.exhausted
        && !view.viewport.in_flight
        && conversation.load_error.is_none();
    let at_tail = viewport.distance_from_end <= 0.25;
    let older = more
        && !source_prefix
        && viewport.viewport_height > 0.0
        && viewport.distance_from_start < 1200.0
        && (!following || at_tail);
    let work = conversation
        .rows
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, (row, _))| match row {
            crate::conversation::fold::Row::EarlierWork { first, .. }
                if (index < viewport.visible.start && viewport.visible.start - index <= 8)
                    || near(index, 1200.0) =>
            {
                Some(*first)
            }
            _ => None,
        });
    let work_idle = conversation.work_idle() && conversation.load_error.is_none();
    let settled = view.viewport.loaded();
    let ready = settled
        && work_idle
        && work.is_none()
        && (view.viewport.exhausted || viewport.distance_from_start >= 1200.0)
        && !(viewport.mounted.start.min(conversation.rows.len())
            ..viewport.mounted.end.min(conversation.rows.len()))
            .any(|index| near(index, 0.0) && preparing(index));
    if older {
        conversation.viewport(core, crate::conversation::viewport::Input::TopReached);
    }
    if let Some(first) = work.filter(|_| work_idle) {
        conversation.load_work(core, first);
    }
    if landing && ready {
        conversation.landed();
    }
}

const LANDING_OVERSCAN_PIXELS: f32 = 320.0;
const SETTLED_OVERSCAN_PIXELS: f32 = 800.0;

fn overscan_from_environment(name: &str, fallback: f32) -> f32 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(fallback)
}

fn transcript_overscan_pixels(conversation: &Conversation) -> f32 {
    let settled = overscan_from_environment("AQUA_OVERSCAN_PX", SETTLED_OVERSCAN_PIXELS);
    match conversation.landing.is_some() {
        true => overscan_from_environment("AQUA_LANDING_OVERSCAN_PX", LANDING_OVERSCAN_PIXELS)
            .min(settled),
        false => settled,
    }
}
