use super::super::visual::{BodyTone, LinkAction};
use super::super::{Block, CodeFence};
use super::{
    CodeServices, HeatmapServices, MarkdownPresenter, PreparationProof, PresenterError,
    RetainedRich, StoredProjectedLeaf, StoredProjectedMarkdown, code_card,
};
use crate::ui::rich::{self, RichFence};
use crate::ui::theme::ArtisanTheme;
use aqua_core::GpuiHsla;
use aqua_text::document::{SelectionDocumentId, SelectionSource, SelectionSourceId};
use aqua_ui::collection_runtime::{SelectionBatchOwner, SelectionSourceBatch};
use aqua_ui::element::Element;
use editor_assets::AssetId;
use std::rc::Rc;
use std::sync::Arc;

impl MarkdownPresenter {
    pub(super) fn leaf_is_current(&self, row: usize, proof: &PreparationProof) -> bool {
        self.projected.is_some()
            && self
                .leaf_proofs
                .get(row)
                .is_some_and(|(_, current)| Arc::ptr_eq(current, &proof.0))
    }

    pub fn release_stored_leaf(&self, id: &str, ordinal: usize) {
        let removed = self
            .stored_leaves
            .borrow_mut()
            .remove(&(id.to_owned(), ordinal));
        if let Some(rich) = removed.and_then(|entry| entry.rich) {
            self.remove_rich(rich.id);
        }
    }

    pub fn prepare_stored_leaf(
        &mut self,
        owner: &super::super::stored::StoredOwner,
        id: &str,
        ordinal: usize,
        leaf: Rc<super::super::stored::StoredLeaf>,
        theme: &ArtisanTheme,
        document: SelectionDocumentId,
        prefix: &[u64],
        mut source_id: impl FnMut(usize) -> SelectionSourceId,
    ) -> Result<(), PresenterError> {
        if leaf.proof().owner() != id
            || usize::try_from(leaf.descriptor.ordinal).ok() != Some(ordinal)
            || !owner.leaf_is_current(leaf.proof())
        {
            return Err(PresenterError::Unavailable("stale stored leaf"));
        }
        let key = (id.to_owned(), ordinal);
        if self.stored_leaves.borrow().get(&key).is_some_and(|cached| {
            Rc::ptr_eq(&cached.leaf, &leaf)
                && cached.document == document
                && cached.prefix == prefix
        }) {
            return Ok(());
        }
        let mut rich = None;
        let mut fallback_note = None;
        let mut rich_without_source = false;
        let mut fence = None;
        if let super::super::IndexedLeafContent::Block(Block::Code(code)) = &leaf.content {
            if let Some(prepared) =
                rich::prepare(code.language.as_deref(), &code.source, code.closed)
            {
                rich_without_source =
                    matches!(prepared, RichFence::Ready(_) | RichFence::Pending { .. });
                if let RichFence::Invalid { note } = &prepared {
                    fallback_note = Some(note.clone());
                }
                let id = self.next_rich_id;
                self.next_rich_id = self.next_rich_id.wrapping_add(1);
                rich = Some(RetainedRich {
                    id,
                    source: code.clone(),
                    prepared,
                });
            }
            rich_without_source |= code.diagram.is_some();
            if !rich_without_source {
                fence = Some(super::super::fence::Fence::new(code));
            }
        }
        let (sources, bytes) = super::super::projected::stored_leaf_sources(
            &leaf,
            ordinal,
            theme,
            prefix,
            &mut source_id,
            fence.as_ref(),
            rich_without_source,
        )?;
        let bytes = bytes.saturating_add(rich.as_ref().map_or(0, |rich| {
            size_of::<RetainedRich>()
                + rich.source.source.capacity()
                + rich.source.language.as_ref().map_or(0, String::capacity)
                + rich.prepared.resident_bytes()
        }));
        if bytes > super::super::stored::PREPARED_BYTES / 2 {
            return Err(PresenterError::Unavailable(
                "Markdown leaf exceeds its normalized-memory budget; open the full source",
            ));
        }
        let selection_owner = SelectionBatchOwner::new();
        let selection_batch = Arc::new(SelectionSourceBatch::new(
            document,
            selection_owner,
            1,
            Arc::clone(&sources),
        ));
        let previous = self.stored_leaves.borrow_mut().insert(
            key,
            StoredProjectedLeaf {
                leaf,
                document,
                prefix: prefix.to_vec(),
                selection_owner,
                selection_batch,
                sources,
                fence,
                rich,
                fallback_note,
            },
        );
        if let Some(previous) = previous.and_then(|entry| entry.rich) {
            self.remove_rich(previous.id);
        }
        Ok(())
    }

    pub fn render_stored_leaf(
        &self,
        owner: &super::super::stored::StoredOwner,
        id: &str,
        ordinal: usize,
        slot_start: usize,
        final_row: bool,
        theme: &ArtisanTheme,
        tone: BodyTone,
        glyph: &Rc<dyn Fn(AssetId, f32, GpuiHsla) -> Element>,
        invalidate: &Rc<dyn Fn()>,
        on_link: Option<Rc<dyn Fn(LinkAction)>>,
        on_color: Option<Rc<dyn Fn(rich::actions::ShowCopyMenu)>>,
        scope: Option<rich::actions::ResourceScope>,
        charts: Option<&HeatmapServices>,
        code: Option<&CodeServices>,
    ) -> Result<StoredProjectedMarkdown, PresenterError> {
        let entries = self.stored_leaves.borrow();
        let entry = entries
            .get(&(id.to_owned(), ordinal))
            .ok_or(PresenterError::Unavailable("unprepared stored leaf"))?;
        if !owner.leaf_is_current(entry.leaf.proof()) {
            return Err(PresenterError::Unavailable("stale stored leaf"));
        }
        let rich_scope = scope.clone();
        let rich = |index: usize, fence: &CodeFence| {
            if fence.diagram.is_some() {
                return Ok(self.render_diagram(id, index, fence, theme, invalidate, code));
            }
            self.render_rich(
                entry
                    .rich
                    .as_ref()
                    .ok_or(PresenterError::Unavailable("nested rich block"))?,
                id,
                theme,
                &Rc::clone(glyph),
                &Rc::clone(invalidate),
                charts,
                code,
                rich_scope.clone(),
            )
        };
        let color_action = on_color.clone();
        let ordinary = |row: usize,
                        fence: &super::super::fence::Fence,
                        sources: &[SelectionSource],
                        note: Option<&str>| {
            code_card(
                fence,
                &format!("{id}-code-{row}"),
                theme,
                entry.document,
                sources,
                code,
                color_action.clone(),
                note,
            )
        };
        let reveal = self
            .reveals
            .borrow()
            .get(id)
            .and_then(|rows| rows.get(&ordinal))
            .cloned();
        let (element, geometry) = super::super::projected::render_stored_leaf(
            &entry.leaf,
            ordinal,
            &entry.sources,
            slot_start,
            final_row,
            theme,
            tone,
            entry.document,
            reveal.as_deref(),
            entry.selection_owner,
            &entry.prefix,
            on_link,
            on_color,
            scope,
            entry.fence.as_ref(),
            entry.fallback_note.as_deref(),
            Some(&rich),
            Some(&ordinary),
        )?;
        Ok(StoredProjectedMarkdown {
            element,
            selection_batch: Arc::clone(&entry.selection_batch),
            geometry,
            #[cfg(test)]
            list_depth: entry.leaf.context.lists.len(),
            proof: entry.leaf.proof().clone(),
        })
    }
}
