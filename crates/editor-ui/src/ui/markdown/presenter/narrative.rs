use super::{MarkdownPresenter, PresenterError, RichCall, enveloped};
use crate::ui::rich::charts::{self};
use crate::ui::rich::narrative::callout::GlyphFactory;
use crate::ui::rich::{self};
use aqua_ui::element::Element;
use std::rc::Rc;

impl MarkdownPresenter {
    pub(super) fn render_compare(
        &self,
        compare: &rich::narrative::compare::Compare,
        call: RichCall<'_>,
    ) -> Result<Element, PresenterError> {
        let RichCall {
            entry,
            envelope,
            id,
            theme,
            invalidate,
            heatmap,
            ..
        } = call;
        let services = heatmap.ok_or(PresenterError::Unavailable("chart services"))?;
        let block_id = format!("{id}-rich-{}-compare", entry.id);
        self.comparison_ids
            .borrow_mut()
            .entry(id.to_owned())
            .or_default()
            .insert(block_id.clone());
        let content = compare.view(
            &block_id,
            theme,
            self.comparisons.clone(),
            &services.navigation_frame,
            Rc::clone(invalidate),
        );
        Ok(enveloped(theme, envelope, content))
    }

    pub(super) fn render_stats(
        &self,
        stats: &rich::data::stats::Stats,
        call: RichCall<'_>,
    ) -> Element {
        let RichCall {
            entry,
            envelope,
            id,
            theme,
            glyph,
            invalidate,
            ..
        } = call;
        let block_id = format!("{id}-rich-{}-stats", entry.id);
        self.stats_ids
            .borrow_mut()
            .entry(id.to_owned())
            .or_default()
            .insert(block_id.clone());
        let hint: rich::data::stats::HintFactory = Rc::new(charts::frame::stats_hint);
        let content = stats.view(block_id, theme, &self.stats, glyph, &hint, invalidate);
        enveloped(theme, envelope, content)
    }

    pub(super) fn render_callout(
        &self,
        callout: &rich::narrative::callout::Callout,
        call: RichCall<'_>,
    ) -> Element {
        let RichCall {
            entry,
            envelope,
            id,
            theme,
            glyph,
            invalidate,
            ..
        } = call;
        let glyph: GlyphFactory = Rc::clone(glyph);
        let block_id = format!("{id}-rich-{}-callout", entry.id);
        self.callout_ids
            .borrow_mut()
            .entry(id.to_owned())
            .or_default()
            .insert(block_id.clone());
        callout.view(
            block_id,
            theme,
            rich::narrative::callout::CalloutNotes {
                subtitle: envelope.subtitle.as_deref(),
                caption: envelope.caption.as_deref(),
            },
            self.callouts.clone(),
            &glyph,
            Rc::clone(invalidate),
        )
    }
}
