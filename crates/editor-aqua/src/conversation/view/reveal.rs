use crate::conversation::controller::Conversation;
use crate::conversation::fold::RowKey;
use crate::conversation::presentation::{MarkdownOwners, PresentationScope, ProjectedProof};
use crate::core::Core;
use crate::ui::markdown::presenter::{ProjectedGeometry, ProjectedRevealText};
use crate::ui::markdown::reveal::PreparedReveal;
use aqua_core::NodeId;
use aqua_ui::tree::{NodeKind, UiTree};
use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

#[derive(Default)]
pub struct MountedReveals {
    scope: Option<PresentationScope>,
    rows: HashMap<RowKey, Weak<RefCell<MountedReveal>>>,
    proofs: HashMap<RowKey, ProjectedProof>,
}

pub(crate) struct MountedReveal {
    reveal: Rc<PreparedReveal>,
    fresh: bool,
    ready: Rc<Cell<bool>>,
    frame: Option<OwnedGeometry>,
    stored_frame: Option<StoredGeometry>,
    settled_at: Cell<Option<(u64, u64)>>,
}

struct StoredGeometry {
    version: u64,
    block: usize,
    proof: crate::conversation::stored::StoredProof,
    geometry: ProjectedGeometry,
    text: Vec<ProjectedRevealText>,
}

struct OwnedGeometry {
    version: u64,
    block: usize,
    proof: ProjectedProof,
    geometry: ProjectedGeometry,
    text: Vec<ProjectedRevealText>,
    live: Option<crate::conversation::live::LiveBinding>,
}

impl MountedReveals {
    pub fn ready(&self, key: &RowKey) -> Option<bool> {
        self.rows
            .get(key)?
            .upgrade()
            .map(|row| row.borrow().ready.get())
    }

    #[cfg(feature = "test-support")]
    pub fn retained_for_tests(&self, key: &RowKey) -> Option<Rc<PreparedReveal>> {
        self.rows
            .get(key)?
            .upgrade()
            .map(|row| row.borrow().reveal.clone())
    }

    pub fn rebind(&mut self, scope: Option<PresentationScope>) {
        if self.scope != scope {
            for row in self.rows.values().filter_map(Weak::upgrade) {
                row.borrow().ready.set(false);
            }
            self.rows.clear();
            self.proofs.clear();
            self.scope = scope;
        }
    }

    pub(crate) fn acquire(
        &mut self,
        owner: &super::row_cache::RowOwner,
        fresh: impl FnOnce() -> bool,
    ) -> (Rc<PreparedReveal>, bool, Rc<Cell<bool>>) {
        let mut slot = owner.reveal.borrow_mut();
        let row = slot.get_or_insert_with(|| {
            Rc::new(RefCell::new(MountedReveal {
                reveal: Rc::new(PreparedReveal::default()),
                fresh: fresh(),
                ready: Rc::new(Cell::new(false)),
                frame: None,
                stored_frame: None,
                settled_at: Cell::new(None),
            }))
        });
        let row = row.borrow();
        (row.reveal.clone(), row.fresh, row.ready.clone())
    }

    pub(crate) fn record(
        &mut self,
        owner: &super::row_cache::RowOwner,
        version: u64,
        block: usize,
        prepared: crate::conversation::presentation::PreparedProjectedRow,
        live: Option<crate::conversation::live::LiveBinding>,
    ) -> aqua_ui::collection::BatchedCollectionRow {
        let slot = owner.reveal.borrow();
        let row = slot.as_ref().expect("reveal row acquired before rendering");
        let mut row = row.borrow_mut();
        row.ready.set(false);
        row.settled_at.set(None);
        row.stored_frame = None;
        row.frame = Some(OwnedGeometry {
            version,
            block,
            proof: prepared.proof,
            geometry: prepared.geometry,
            text: prepared.text,
            live,
        });
        prepared.row
    }

    pub(crate) fn record_stored(
        &mut self,
        owner: &super::row_cache::RowOwner,
        version: u64,
        block: usize,
        proof: crate::conversation::stored::StoredProof,
        prepared: crate::ui::markdown::presenter::StoredProjectedMarkdown,
    ) -> aqua_ui::collection::BatchedCollectionRow {
        let slot = owner.reveal.borrow();
        let row = slot.as_ref().expect("reveal row acquired before rendering");
        let mut row = row.borrow_mut();
        row.ready.set(false);
        row.settled_at.set(None);
        row.frame = None;
        let text = prepared.reveal_text(&row.reveal);
        row.stored_frame = Some(StoredGeometry {
            version,
            block,
            proof,
            geometry: prepared.geometry,
            text,
        });
        aqua_ui::collection::BatchedCollectionRow {
            element: prepared.element,
            batches: vec![prepared.selection_batch],
        }
    }

    pub(crate) fn activate(&mut self, key: RowKey, owner: &super::row_cache::RowOwner) {
        let slot = owner.reveal.borrow();
        if let Some(row) = slot.as_ref() {
            if let Some(frame) = &row.borrow().frame {
                self.proofs.insert(key.clone(), frame.proof.clone());
            }
            self.rows.insert(key, Rc::downgrade(row));
        }
    }

    pub fn sync(
        &mut self,
        app: &mut aqua::UiApp,
        conversation: &Conversation,
        core: &Core,
        markdown: &mut MarkdownOwners,
        stored: Option<&crate::conversation::stored::StoredConversation>,
    ) -> RevealSync {
        let Some(scope) = self.scope.as_ref() else {
            return RevealSync::default();
        };
        if !scope.current(conversation, core) {
            return RevealSync {
                reason: "sync_scope",
                pending: true,
                ..RevealSync::default()
            };
        }
        let mut result = RevealSync::default();
        self.rows.retain(|key, row| {
            if row.strong_count() > 0 {
                true
            } else {
                if let Some(proof) = self.proofs.remove(key) {
                    markdown.release_projected_reveal(&proof);
                }
                false
            }
        });
        for (key, weak) in &self.rows {
            let Some(row) = weak.upgrade() else { continue };
            let row = row.borrow_mut();
            let epochs = (app.tree().layout_version(), app.tree().structure_epoch());
            if row.ready.get()
                && row.settled_at.get() == Some(epochs)
                && !row.reveal.is_animating()
            {
                continue;
            }
            row.settled_at.set(None);
            let present = app
                .tree()
                .root()
                .and_then(|root| {
                    unique_hash(app.tree(), root, super::transcript::row_key_hash(key))
                })
                .is_some();
            if !present {
                row.ready.set(false);
                continue;
            }
            if let Some(frame) = &row.stored_frame {
                let valid = stored.is_some_and(|stored| {
                    conversation
                        .row_position(key)
                        .and_then(|index| conversation.rows.get(index))
                        .filter(|(_, version)| *version == frame.version)
                        .and_then(|(candidate, _)| {
                            candidate
                                .anchor()
                                .and_then(|id| conversation.view.as_ref()?.item(id))
                                .map(|item| frame.proof.accepts_row(stored, scope, candidate, item))
                        })
                        .unwrap_or(false)
                });
                if !valid {
                    row.ready.set(false);
                    result.reason = "stored_invalid";
                    result.pending = true;
                    continue;
                }
                let Some(changed) = sync_contents(
                    app,
                    key,
                    frame.block,
                    &frame.geometry,
                    &frame.text,
                    &row.reveal,
                ) else {
                    row.ready.set(false);
                    result.reason = "stored_contents";
                    result.pending = true;
                    continue;
                };
                result.changed |= changed;
                result.animating |= row.reveal.is_animating();
                row.ready.set(!row.reveal.is_animating());
                row.settled_at.set(row.ready.get().then_some(epochs));
                continue;
            }
            let Some(frame) = &row.frame else {
                row.ready.set(false);
                result.animating = true;
                continue;
            };
            if frame
                .live
                .as_ref()
                .is_some_and(|live| !live.current(conversation, core))
            {
                row.ready.set(false);
                result.reason = "live_stale";
                result.pending = true;
                continue;
            }
            let state = sync_geometry(
                app,
                conversation,
                core,
                markdown,
                GeometryFrame {
                    scope,
                    row: key,
                    row_version: frame.version,
                    block: frame.block,
                    proof: &frame.proof,
                    geometry: &frame.geometry,
                    text: &frame.text,
                    reveal: &row.reveal,
                },
            );
            let Some(changed) = state else {
                row.ready.set(false);
                result.reason = "geometry";
                result.pending = true;
                continue;
            };
            result.changed |= changed;
            result.animating |= row.reveal.is_animating();
            let complete = frame
                .live
                .as_ref()
                .is_none_or(|live| live.complete(conversation, core));
            row.ready.set(complete && !row.reveal.is_animating());
            row.settled_at.set(row.ready.get().then_some(epochs));
        }
        result
    }
}

#[derive(Default)]
pub struct RevealSync {
    pub reason: &'static str,
    pub frontier_changed: bool,
    pub changed: bool,
    pub animating: bool,
    pub pending: bool,
}

fn unique_key(tree: &UiTree, root: NodeId, key: &str) -> Option<NodeId> {
    unique_hash(tree, root, crate::shell::wheel::key_hash(key))
}

fn unique_hash(tree: &UiTree, root: NodeId, hash: aqua_ui::tree::KeyHash) -> Option<NodeId> {
    match tree.sole_node_with_key(hash) {
        Some(node) => is_within(tree, node, root).then_some(node),
        None => {
            let mut within = tree
                .nodes_with_key(hash)
                .into_iter()
                .filter(|node| is_within(tree, *node, root));
            let found = within.next()?;
            within.next().is_none().then_some(found)
        }
    }
}

fn is_within(tree: &UiTree, node: NodeId, root: NodeId) -> bool {
    let mut current = Some(node);
    while let Some(id) = current {
        if id == root {
            return true;
        }
        current = tree.get(id).and_then(|record| record.parent);
    }
    false
}

pub struct GeometryFrame<'a> {
    pub scope: &'a PresentationScope,
    pub row: &'a RowKey,
    pub row_version: u64,
    pub block: usize,
    pub proof: &'a ProjectedProof,
    pub geometry: &'a ProjectedGeometry,
    pub text: &'a [ProjectedRevealText],
    pub reveal: &'a PreparedReveal,
}

pub fn sync_geometry(
    app: &mut aqua::UiApp,
    conversation: &Conversation,
    core: &Core,
    owners: &MarkdownOwners,
    frame: GeometryFrame<'_>,
) -> Option<bool> {
    if !frame.scope.current(conversation, core)
        || !owners.accepts_projected_row(frame.scope, frame.proof)
        || !conversation
            .row_position(frame.row)
            .and_then(|index| conversation.rows.get(index))
            .is_some_and(|(row, version)| {
                *version == frame.row_version && frame.proof.matches_row(row)
            })
    {
        crate::conversation::open_timing::note_fail("proof");
        return None;
    }
    sync_contents(
        app,
        frame.row,
        frame.block,
        frame.geometry,
        frame.text,
        frame.reveal,
    )
}

fn sync_contents(
    app: &mut aqua::UiApp,
    row_key: &RowKey,
    block: usize,
    geometry: &ProjectedGeometry,
    text_parts: &[ProjectedRevealText],
    reveal: &PreparedReveal,
) -> Option<bool> {
    let tree = app.tree();
    let Some(row) = tree
        .root()
        .and_then(|root| unique_hash(tree, root, super::transcript::row_key_hash(row_key)))
    else {
        crate::conversation::open_timing::note_fail("row_missing");
        return None;
    };
    let Some(content) = unique_key(tree, row, &geometry.frame_key) else {
        crate::conversation::open_timing::note_fail("frame_missing");
        return None;
    };
    let Some(bounds) = tree.pre_transform_rect(content) else {
        crate::conversation::open_timing::note_fail("bounds_missing");
        return None;
    };
    let mut parts = Vec::with_capacity(text_parts.len());
    for part in text_parts {
        let Some(node) = unique_key(tree, content, &part.key) else {
            crate::conversation::open_timing::note_fail("part_missing");
            return None;
        };
        let Some(NodeKind::Text(text)) = tree.get(node).map(|record| &record.kind) else {
            crate::conversation::open_timing::note_fail("part_kind");
            return None;
        };
        if text.text() != part.text.as_ref() {
            crate::conversation::open_timing::note_fail("part_text");
            return None;
        }
        parts.push((node, part));
    }
    for (node, part) in &parts {
        if !reveal.report_text_frontier(app, *node, &part.text, part.end) {
            crate::conversation::open_timing::note_fail("frontier");
            return None;
        }
    }
    let mut changed = reveal.update_visible_height(bounds.y, bounds.h);
    if geometry.block_entrance {
        changed |= reveal.update_block_height(block, bounds.h);
    }
    for (node, part) in parts {
        let ranges: std::sync::Arc<[aqua_core::scene::GpuiTextOpacityRange]> = part
            .fades
            .iter()
            .map(|fade| aqua_core::scene::GpuiTextOpacityRange {
                bytes: fade.range.clone(),
                opacity: fade.opacity,
            })
            .collect::<Vec<_>>()
            .into();
        let different = matches!(&app.tree().get(node)?.kind, NodeKind::Text(text) if text.gpui_text_opacity != ranges);
        if different {
            if !part.apply_opacity(app, node) {
                return None;
            }
            changed = true;
        }
    }
    Some(changed)
}
