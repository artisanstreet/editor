use super::{HeatmapServices, MarkdownPresenter, PresenterError, RetainedRich, enveloped};
use crate::ui::rich::charts::{self};
use crate::ui::rich::narrative::callout::GlyphFactory;
use crate::ui::rich::{self};
use crate::ui::theme::ArtisanTheme;
use aqua_core::GpuiHsla;
use aqua_ui::element::Element;
use editor_assets::AssetId;
use std::rc::Rc;

impl MarkdownPresenter {
    pub(super) fn render_compare(
        &self,
        compare: &crate::ui::rich::narrative::compare::Compare,
        entry: &RetainedRich,
        envelope: &rich::Envelope,
        id: &str,
        theme: &ArtisanTheme,
        invalidate: Rc<dyn Fn()>,
        heatmap: Option<&HeatmapServices>,
    ) -> Result<Element, PresenterError> {
        let services = heatmap.ok_or(PresenterError::Unavailable("chart services"))?;
        let block_id = format!("{id}-rich-{}-compare", entry.id);
        self.comparison_ids
            .borrow_mut()
            .entry(id.to_owned())
            .or_default()
            .insert(block_id.clone());
        let content = compare.view(
            block_id,
            theme,
            self.comparisons.clone(),
            services.navigation_frame.clone(),
            invalidate.clone(),
        );
        Ok(enveloped(theme, envelope, content))
    }

    pub(super) fn render_stats(
        &self,
        stats: &crate::ui::rich::data::stats::Stats,
        entry: &RetainedRich,
        envelope: &rich::Envelope,
        id: &str,
        theme: &ArtisanTheme,
        glyph: Rc<dyn Fn(AssetId, f32, GpuiHsla) -> Element>,
        invalidate: Rc<dyn Fn()>,
    ) -> Element {
        let block_id = format!("{id}-rich-{}-stats", entry.id);
        self.stats_ids
            .borrow_mut()
            .entry(id.to_owned())
            .or_default()
            .insert(block_id.clone());
        let content = stats.view(
            block_id,
            theme,
            self.stats.clone(),
            glyph.clone(),
            Rc::new(charts::frame::stats_hint),
            invalidate.clone(),
        );
        enveloped(theme, envelope, content)
    }

    pub(super) fn render_callout(
        &self,
        callout: &crate::ui::rich::narrative::callout::Callout,
        entry: &RetainedRich,
        envelope: &rich::Envelope,
        id: &str,
        theme: &ArtisanTheme,
        glyph: Rc<dyn Fn(AssetId, f32, GpuiHsla) -> Element>,
        invalidate: Rc<dyn Fn()>,
    ) -> Element {
        let glyph: GlyphFactory = glyph.clone();
        let block_id = format!("{id}-rich-{}-callout", entry.id);
        self.callout_ids
            .borrow_mut()
            .entry(id.to_owned())
            .or_default()
            .insert(block_id.clone());
        callout.view(
            block_id,
            theme,
            crate::ui::rich::narrative::callout::CalloutNotes {
                subtitle: envelope.subtitle.as_deref(),
                caption: envelope.caption.as_deref(),
            },
            self.callouts.clone(),
            glyph,
            invalidate.clone(),
        )
    }
}
