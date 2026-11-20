use super::super::CodeFence;
use super::super::projected::LeafRender;
use super::super::visual::BodyContext;
use super::{
    BodyServices, MarkdownPresenter, PreparationProof, PresenterError, ProjectedLeafInfo,
    ProjectedMarkdown, SelectionPlace, code_card,
};
use aqua_text::document::{SelectionSource, SelectionSourceId};
use std::rc::Rc;
use std::sync::Arc;

impl MarkdownPresenter {
    pub fn projected_count(&self) -> Option<usize> {
        self.projected
            .as_ref()
            .map(super::super::projected::ProjectedDocument::len)
    }

    fn projected_range(&self, row: usize) -> Option<std::ops::Range<usize>> {
        self.projected.as_ref()?.range(row)
    }

    pub fn projected_leaf_info(&self, row: usize) -> Option<ProjectedLeafInfo> {
        let projected = self.projected.as_ref()?;
        Some(ProjectedLeafInfo {
            source_range: projected.range(row)?,
            list_depth: projected.depth(row)?,
            proof: self.leaf_proof(row)?,
        })
    }

    fn leaf_proof(&self, row: usize) -> Option<PreparationProof> {
        self.leaf_proofs
            .get(row)
            .map(|(_, proof)| PreparationProof(Arc::clone(proof)))
    }

    pub fn projected_leaf_is_current(&self, row: usize, info: &ProjectedLeafInfo) -> bool {
        self.leaf_is_current(row, &info.proof)
            && self.projected.as_ref().is_some_and(|projected| {
                projected.range(row).as_ref() == Some(&info.source_range)
                    && projected.depth(row) == Some(info.list_depth)
            })
    }

    pub fn projected_is_current(
        &self,
        row: usize,
        source_range: &std::ops::Range<usize>,
        proof: &PreparationProof,
    ) -> bool {
        self.leaf_is_current(row, proof) && self.projected_range(row).as_ref() == Some(source_range)
    }

    pub fn render_projected(
        &self,
        row: usize,
        body: BodyContext<'_>,
        place: SelectionPlace<'_>,
        mut source_id: impl FnMut(usize) -> SelectionSourceId,
    ) -> Result<ProjectedMarkdown, PresenterError> {
        let projected = self
            .projected
            .as_ref()
            .ok_or(PresenterError::Unavailable("unprepared"))?;
        let source_range = projected
            .range(row)
            .ok_or(PresenterError::Unavailable("projected row"))?;
        let render = LeafRender {
            body,
            place,
            reveal: None,
            batch_owner: self.selection_owner,
            rich: None,
            ordinary: None,
        };
        let (element, selection_sources, selection_batch, geometry) =
            projected.render(row, render, None, &self.selection_revision, &mut source_id)?;
        Ok(ProjectedMarkdown {
            element,
            selection_sources,
            selection_batch,
            geometry,
            source_range,
            proof: self
                .leaf_proof(row)
                .ok_or(PresenterError::Unavailable("projected row"))?,
        })
    }

    pub fn render_projected_with_code(
        &self,
        row: usize,
        id: &str,
        body: BodyContext<'_>,
        place: SelectionPlace<'_>,
        mut source_id: impl FnMut(usize) -> SelectionSourceId,
        services: BodyServices<'_>,
    ) -> Result<ProjectedMarkdown, PresenterError> {
        let projected = self
            .projected
            .as_ref()
            .ok_or(PresenterError::Unavailable("unprepared"))?;
        let source_range = projected
            .range(row)
            .ok_or(PresenterError::Unavailable("projected row"))?;
        let rich = |index: usize, fence: &CodeFence| {
            if fence.diagram.is_some() {
                return Ok(self.render_diagram(
                    id,
                    index,
                    fence,
                    body.theme,
                    services.invalidate,
                    services.code,
                ));
            }
            let entry = self
                .rich
                .get(index)
                .and_then(Option::as_ref)
                .ok_or(PresenterError::Unavailable("nested rich block"))?;
            self.render_rich(entry, id, body, services)
        };
        let ordinary = |ordinal: usize,
                        fence: &super::super::fence::Fence,
                        sources: &[SelectionSource],
                        note: Option<&editor_core::Problem>| {
            code_card(
                fence,
                &format!("{id}-code-{ordinal}"),
                body,
                place.document,
                sources,
                services.code,
                note,
            )
        };
        let reveal = self
            .reveals
            .borrow()
            .get(id)
            .and_then(|rows| rows.get(&row))
            .cloned();
        let render = LeafRender {
            body,
            place,
            reveal: reveal.as_deref(),
            batch_owner: self.selection_owner,
            rich: Some(&rich),
            ordinary: Some(&ordinary),
        };
        let (element, selection_sources, selection_batch, geometry) = projected.render(
            row,
            render,
            Some(id),
            &self.selection_revision,
            &mut source_id,
        )?;
        Ok(ProjectedMarkdown {
            element,
            selection_sources,
            selection_batch,
            geometry,
            source_range,
            proof: self
                .leaf_proof(row)
                .ok_or(PresenterError::Unavailable("projected row"))?,
        })
    }

    pub fn set_projected_reveal(
        &self,
        owner: &str,
        row: usize,
        reveal: Option<Rc<super::super::reveal::PreparedReveal>>,
    ) {
        let mut reveals = self.reveals.borrow_mut();
        if let Some(reveal) = reveal {
            reveals
                .entry(owner.to_owned())
                .or_default()
                .insert(row, reveal);
        } else if let Some(rows) = reveals.get_mut(owner) {
            rows.remove(&row);
            if rows.is_empty() {
                reveals.remove(owner);
            }
        }
    }
}
