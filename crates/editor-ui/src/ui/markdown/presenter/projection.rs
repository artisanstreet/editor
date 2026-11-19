use super::super::CodeFence;
use super::super::visual::{BodyTone, LinkAction};
use super::{
    CodeServices, HeatmapServices, MarkdownPresenter, PreparationProof, PresenterError,
    ProjectedLeafInfo, ProjectedMarkdown, code_card,
};
use crate::ui::theme::ArtisanTheme;
use aqua_core::GpuiHsla;
use aqua_text::document::{SelectionDocumentId, SelectionSource, SelectionSourceId};
use aqua_ui::element::Element;
use editor_assets::AssetId;
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
        theme: &ArtisanTheme,
        tone: BodyTone,
        document: SelectionDocumentId,
        order: &[u64],
        mut source_id: impl FnMut(usize) -> SelectionSourceId,
        on_link: Option<Rc<dyn Fn(LinkAction)>>,
        on_color: Option<Rc<dyn Fn(crate::ui::rich::actions::ShowCopyMenu)>>,
        scope: Option<crate::ui::rich::actions::ResourceScope>,
    ) -> Result<ProjectedMarkdown, PresenterError> {
        let projected = self
            .projected
            .as_ref()
            .ok_or(PresenterError::Unavailable("unprepared"))?;
        let source_range = projected
            .range(row)
            .ok_or(PresenterError::Unavailable("projected row"))?;
        let (element, selection_sources, selection_batch, geometry) = projected.render(
            row,
            theme,
            tone,
            document,
            None,
            None,
            self.selection_owner,
            &self.selection_revision,
            order,
            &mut source_id,
            on_link,
            on_color,
            scope,
            None,
            None,
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

    pub fn render_projected_with_services(
        &self,
        row: usize,
        id: &str,
        theme: &ArtisanTheme,
        tone: BodyTone,
        document: SelectionDocumentId,
        order: &[u64],
        source_id: impl FnMut(usize) -> SelectionSourceId,
        glyph: Rc<dyn Fn(AssetId, f32, GpuiHsla) -> Element>,
        invalidate: Rc<dyn Fn()>,
        on_link: Option<Rc<dyn Fn(LinkAction)>>,
        on_color: Option<Rc<dyn Fn(crate::ui::rich::actions::ShowCopyMenu)>>,
        scope: Option<crate::ui::rich::actions::ResourceScope>,
        charts: Option<&HeatmapServices>,
    ) -> Result<ProjectedMarkdown, PresenterError> {
        self.render_projected_with_code(
            row, id, theme, tone, document, order, source_id, glyph, invalidate, on_link, on_color,
            scope, charts, None,
        )
    }

    pub fn render_projected_with_code(
        &self,
        row: usize,
        id: &str,
        theme: &ArtisanTheme,
        tone: BodyTone,
        document: SelectionDocumentId,
        order: &[u64],
        mut source_id: impl FnMut(usize) -> SelectionSourceId,
        glyph: Rc<dyn Fn(AssetId, f32, GpuiHsla) -> Element>,
        invalidate: Rc<dyn Fn()>,
        on_link: Option<Rc<dyn Fn(LinkAction)>>,
        on_color: Option<Rc<dyn Fn(crate::ui::rich::actions::ShowCopyMenu)>>,
        scope: Option<crate::ui::rich::actions::ResourceScope>,
        charts: Option<&HeatmapServices>,
        code: Option<&CodeServices>,
    ) -> Result<ProjectedMarkdown, PresenterError> {
        let projected = self
            .projected
            .as_ref()
            .ok_or(PresenterError::Unavailable("unprepared"))?;
        let source_range = projected
            .range(row)
            .ok_or(PresenterError::Unavailable("projected row"))?;
        let reference_scope = scope.clone();
        let rich = |index: usize, fence: &CodeFence| {
            if fence.diagram.is_some() {
                return Ok(self.render_diagram(id, index, fence, theme, &invalidate, code));
            }
            let entry = self
                .rich
                .get(index)
                .and_then(Option::as_ref)
                .ok_or(PresenterError::Unavailable("nested rich block"))?;
            self.render_rich(
                entry,
                id,
                theme,
                Rc::clone(&glyph),
                Rc::clone(&invalidate),
                charts,
                code,
                reference_scope.clone(),
            )
        };
        let color_action = on_color.clone();
        let ordinary = |ordinal: usize,
                        fence: &super::super::fence::Fence,
                        sources: &[SelectionSource],
                        note: Option<&str>| {
            code_card(
                fence,
                &format!("{id}-code-{ordinal}"),
                theme,
                document,
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
            .and_then(|rows| rows.get(&row))
            .cloned();
        let (element, selection_sources, selection_batch, geometry) = projected.render(
            row,
            theme,
            tone,
            document,
            Some(id),
            reveal.as_deref(),
            self.selection_owner,
            &self.selection_revision,
            order,
            &mut source_id,
            on_link,
            on_color,
            scope,
            Some(&rich),
            Some(&ordinary),
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
