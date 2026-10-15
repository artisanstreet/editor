//! Mermaid fences drawn as diagrams.
//!
//! A closed `mermaid` fence lays out through `mermaid-rs-renderer` into an
//! SVG painted in the transcript's own tokens, and GPUI rasterizes that SVG
//! as an ordinary image. Layout runs on the owning Markdown worker before
//! the prepared block is published. No font scan or layout runs during paint.
//!
//! Source the renderer rejects — or panics on — yields no diagram, and the
//! caller keeps drawing the fence as code.

use crate::markdown::{Block, DiagramSvg, MarkdownDocument};
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use gpui::{RenderImage, SvgRenderer};
use mermaid_rs_renderer::{
    LayoutConfig, Theme, compute_layout, measure_svg_dimensions, parse_mermaid_strict, render_svg,
};

use crate::theme::{ArtisanTheme, Oklch};

/// Diagrams kept per thread, keyed by source. A streaming
/// reply re-renders every frame, so a settled fence above the stream must
/// not lay out again each time.
const MEMO_CAPACITY: usize = 32;

/// Longest source laid out; anything longer stays code.
const MAX_SOURCE_BYTES: usize = 32 * 1024;

/// Largest edge, in logical pixels, a diagram may claim. Beyond this the
/// rasterized image grows past what a transcript should hold.
const MAX_EDGE_PX: f32 = 8192.0;

/// Fonts the labels name. Layout measures against the system font database
/// and GPUI paints with the same one, so the families must be system ones;
/// the app's bundled faces are invisible to the measurer.
const FONT_FAMILY: &str =
    "\"Segoe UI\", \"Helvetica Neue\", Helvetica, Arial, \"DejaVu Sans\", sans-serif";

const FONT_SIZE_PX: f32 = 14.0;

/// One diagram ready to paint: the SVG as an image, and the size it claims.
#[derive(Debug)]
pub(super) struct MermaidDiagram {
    pub(super) image: Arc<RenderImage>,
    pub(super) width: f32,
    pub(super) height: f32,
}

/// Owned by the Markdown worker; exact keys and byte limits avoid collisions
/// and release all diagram resources when that worker closes.
#[derive(Default)]
pub(super) struct DiagramCache {
    entries: VecDeque<(String, Option<Arc<DiagramSvg>>)>,
}
impl DiagramCache {
    pub(super) fn prepare(&mut self, document: &mut MarkdownDocument) {
        self.blocks(&mut document.blocks);
    }
    fn blocks(&mut self, blocks: &mut [Block]) {
        for block in blocks {
            match block {
                Block::Code(fence)
                    if fence.closed && fence.language.as_deref() == Some("mermaid") =>
                {
                    fence.diagram = self.get(&fence.source);
                }
                Block::List { items, .. } => {
                    for item in items {
                        self.blocks(&mut item.blocks);
                    }
                }
                _ => {}
            }
        }
    }
    fn get(&mut self, source: &str) -> Option<Arc<DiagramSvg>> {
        if let Some((_, diagrams)) = self.entries.iter().find(|(key, _)| key == source) {
            return diagrams.clone();
        }
        let diagram = render(source, &ArtisanTheme::resolved()).map(Arc::new);
        self.entries.push_back((source.to_owned(), diagram.clone()));
        while self.entries.len() > MEMO_CAPACITY
            || self
                .entries
                .iter()
                .map(|(key, svgs)| key.len() + svgs.as_ref().map_or(0, |svg| svg.source.len()))
                .sum::<usize>()
                > 8 * 1024 * 1024
        {
            self.entries.pop_front();
        }
        diagram
    }
}
pub(super) fn image(svg: &DiagramSvg, renderer: SvgRenderer) -> Option<MermaidDiagram> {
    let image = super::rich::decode_image_bytes(svg.source.as_bytes(), renderer).ok()?;
    Some(MermaidDiagram {
        image,
        width: f32::from_bits(svg.width_bits),
        height: f32::from_bits(svg.height_bits),
    })
}

fn render(source: &str, theme: &ArtisanTheme) -> Option<DiagramSvg> {
    if source.trim().is_empty()
        || source.len() > MAX_SOURCE_BYTES
        || source
            .bytes()
            .filter(|byte| matches!(byte, b'\n' | b';' | b','))
            .count()
            > 512
    {
        return None;
    }
    let (svg, width, height) = layout_svg(source, &diagram_theme(theme))?;
    (svg.len() <= 2 * 1024 * 1024
        && width.is_finite()
        && height.is_finite()
        && width >= 1.0
        && height >= 1.0
        && width <= MAX_EDGE_PX
        && height <= MAX_EDGE_PX)
        .then(|| DiagramSvg {
            source: svg,
            width_bits: width.to_bits(),
            height_bits: height.to_bits(),
        })
}

/// Parses, lays out, and serializes `source`. The renderer is third-party
/// layout code over model-written input, so a panic counts as a rejection
/// rather than taking the window down.
fn layout_svg(source: &str, theme: &Theme) -> Option<(String, f32, f32)> {
    catch_unwind(AssertUnwindSafe(|| {
        let parsed = parse_mermaid_strict(source).ok()?;
        if parsed.graph.nodes.len()
            + parsed.graph.edges.len()
            + parsed.graph.subgraphs.len()
            + parsed.graph.gantt_tasks.len()
            + parsed.graph.pie_slices.len()
            + parsed.graph.sequence_participants.len()
            + parsed.graph.sequence_frames.len()
            + parsed.graph.sequence_notes.len()
            > 512
        {
            return None;
        }
        let config = LayoutConfig::default();
        let layout = compute_layout(&parsed.graph, theme, &config);
        let size = measure_svg_dimensions(&layout, &config, None);
        Some((render_svg(&layout, theme, &config), size.width, size.height))
    }))
    .ok()
    .flatten()
}

/// The Mermaid palette in transcript tokens.
///
/// The renderer derives shades from some of these, so each is an opaque
/// hex: the token composited over the transcript background. The canvas
/// itself is left unpainted (`none`), so the fence card shows through.
fn diagram_theme(theme: &ArtisanTheme) -> Theme {
    let colors = &theme.colors;
    let over = |color: Oklch, alpha: f32| composite_hex(colors.background, color, alpha);
    let text = over(colors.foreground, 1.0);
    let muted_text = over(colors.muted_foreground, 1.0);
    let node = over(colors.foreground, 0.09);
    let raised = over(colors.foreground, 0.14);
    let border = over(colors.foreground, 0.28);
    let line = over(colors.foreground, 0.55);
    let card = over(colors.foreground, 0.05);
    let cluster = over(colors.foreground, 0.03);
    let cluster_border = over(colors.foreground, 0.16);

    let base = Theme::dark();
    Theme {
        font_family: FONT_FAMILY.to_owned(),
        font_size: FONT_SIZE_PX,
        background: "none".to_owned(),
        primary_color: node.clone(),
        primary_text_color: text.clone(),
        primary_border_color: border.clone(),
        line_color: line.clone(),
        secondary_color: raised.clone(),
        tertiary_color: node.clone(),
        edge_label_background: card,
        cluster_background: cluster,
        cluster_border,
        sequence_actor_fill: node,
        sequence_actor_border: border.clone(),
        sequence_actor_line: line,
        sequence_note_fill: raised.clone(),
        sequence_note_border: border.clone(),
        sequence_activation_fill: raised,
        sequence_activation_border: border,
        text_color: text.clone(),
        pie_title_text_color: text.clone(),
        pie_section_text_color: text.clone(),
        pie_legend_text_color: text.clone(),
        git_commit_label_color: text.clone(),
        git_tag_label_color: text,
        pie_stroke_color: over(colors.background, 1.0),
        pie_outer_stroke_color: muted_text,
        ..base
    }
}

/// `color` at `alpha` over the opaque `base`, as `#rrggbb`.
fn composite_hex(base: Oklch, color: Oklch, alpha: f32) -> String {
    let base = base.to_srgb();
    let top = color.to_srgb();
    let alpha = (alpha * top.a).clamp(0.0, 1.0);
    let channel = |under: f32, over: f32| {
        let mixed = under + (over - under) * alpha;
        // Clamped to [0, 255] before narrowing, so the cast is exact.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let byte = (mixed.clamp(0.0, 1.0) * 255.0).round() as u8;
        byte
    };
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(base.r, top.r),
        channel(base.g, top.g),
        channel(base.b, top.b)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flowchart_renders_into_a_sized_svg() {
        let theme = ArtisanTheme::resolved();
        let diagram = render("flowchart TD\n  A[Browser] --> B[Web]\n", &theme)
            .expect("a valid flowchart renders");
        assert!(
            f32::from_bits(diagram.width_bits) >= 1.0 && f32::from_bits(diagram.height_bits) >= 1.0
        );
        let svg = diagram.source;
        assert!(svg.contains("<svg") && svg.contains("Browser"));
    }

    #[test]
    fn invalid_source_renders_nothing() {
        let theme = ArtisanTheme::resolved();
        assert!(render("this is not a diagram {{{", &theme).is_none());
        assert!(render("   \n", &theme).is_none());
    }
}
