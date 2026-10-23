//! Mermaid fences drawn as diagrams.
//!
//! A closed `mermaid` fence lays out through `mermaid-rs-renderer` into an
//! SVG painted in the transcript's own tokens, once per theme mode, and GPUI
//! rasterizes the SVG of the active mode as an ordinary image. Layout runs on the owning Markdown worker before
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

use crate::theme::{ArtisanTheme, Oklch, ThemeMode};

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

/// One diagram ready to paint: the SVG of each mode as an image, and the
/// size it claims.
#[derive(Debug)]
pub(super) struct MermaidDiagram {
    pub(super) dark: Arc<RenderImage>,
    pub(super) light: Arc<RenderImage>,
    pub(super) width: f32,
    pub(super) height: f32,
}

impl MermaidDiagram {
    /// The image painted in `mode`'s tokens.
    pub(super) const fn image(&self, mode: ThemeMode) -> &Arc<RenderImage> {
        match mode {
            ThemeMode::Dark => &self.dark,
            ThemeMode::Light => &self.light,
        }
    }
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
                Block::Heading { .. }
                | Block::Paragraph { .. }
                | Block::Code(_)
                | Block::Html { .. }
                | Block::Table(_) => {}
            }
        }
    }
    fn get(&mut self, source: &str) -> Option<Arc<DiagramSvg>> {
        if let Some((_, diagrams)) = self.entries.iter().find(|(key, _)| key == source) {
            return diagrams.clone();
        }
        let diagram = render(source).map(Arc::new);
        self.entries.push_back((source.to_owned(), diagram.clone()));
        while self.entries.len() > MEMO_CAPACITY
            || self
                .entries
                .iter()
                .map(|(key, svgs)| {
                    key.len()
                        + svgs
                            .as_ref()
                            .map_or(0, |svg| svg.source.len() + svg.light_source.len())
                })
                .sum::<usize>()
                > 8 * 1024 * 1024
        {
            self.entries.pop_front();
        }
        diagram
    }
}
pub(super) fn image(svg: &DiagramSvg, renderer: SvgRenderer) -> Option<MermaidDiagram> {
    let dark = super::rich::decode_image_bytes(svg.source.as_bytes(), renderer.clone()).ok()?;
    let light = super::rich::decode_image_bytes(svg.light_source.as_bytes(), renderer).ok()?;
    Some(MermaidDiagram {
        dark,
        light,
        width: f32::from_bits(svg.width_bits),
        height: f32::from_bits(svg.height_bits),
    })
}

/// Lays out `source` once and paints it in the tokens of both modes.
fn render(source: &str) -> Option<DiagramSvg> {
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
    let themes =
        [ThemeMode::Dark, ThemeMode::Light].map(|mode| diagram_theme(ArtisanTheme::for_mode(mode)));
    let ([dark, light], width, height) = layout_svg(source, &themes)?;
    (dark.len().max(light.len()) <= 2 * 1024 * 1024
        && width.is_finite()
        && height.is_finite()
        && width >= 1.0
        && height >= 1.0
        && width <= MAX_EDGE_PX
        && height <= MAX_EDGE_PX)
        .then(|| DiagramSvg {
            source: dark,
            light_source: light,
            width_bits: width.to_bits(),
            height_bits: height.to_bits(),
        })
}

/// Parses, lays out, and serializes `source` once per theme. The renderer is third-party
/// layout code over model-written input, so a panic counts as a rejection
/// rather than taking the window down.
fn layout_svg(source: &str, themes: &[Theme; 2]) -> Option<([String; 2], f32, f32)> {
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
        // Both themes share fonts and sizes, so one layout serves both.
        let layout = compute_layout(&parsed.graph, &themes[0], &config);
        let size = measure_svg_dimensions(&layout, &config, None);
        let svgs = [
            render_svg(&layout, &themes[0], &config),
            render_svg(&layout, &themes[1], &config),
        ];
        Some((svgs, size.width, size.height))
    }))
    .ok()
    .flatten()
}

/// The Mermaid palette in transcript tokens.
///
/// Every colour is an opaque token written as hex. The canvas itself is
/// left unpainted (`none`), so the fence card shows through; fills step up
/// from the card's surface.
fn diagram_theme(theme: &ArtisanTheme) -> Theme {
    let colors = &theme.colors;
    let text = hex(colors.text);
    let muted_text = hex(colors.text_muted);
    let node = hex(colors.raised);
    let raised = hex(colors.hover);
    let border = hex(colors.line_strong);
    let line = hex(colors.gray.step(9));
    let card = hex(colors.surface);
    let cluster = hex(colors.surface);
    let cluster_border = hex(colors.gray.step(5));
    let series = |offset: usize| colors.palette[offset % colors.palette.len()];

    let base = match colors.mode {
        ThemeMode::Dark => Theme::dark(),
        ThemeMode::Light => Theme::modern(),
    };
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
        sequence_actor_fill: node.clone(),
        sequence_actor_border: border.clone(),
        sequence_actor_line: line,
        sequence_note_fill: raised.clone(),
        sequence_note_border: border.clone(),
        sequence_activation_fill: raised.clone(),
        sequence_activation_border: border.clone(),
        text_color: text.clone(),
        git_colors: std::array::from_fn(|index| hex(series(index))),
        git_inv_colors: std::array::from_fn(|index| hex(series(index + 4))),
        git_branch_label_colors: std::array::from_fn(|_| text.clone()),
        git_commit_label_color: text.clone(),
        git_commit_label_background: raised,
        git_tag_label_color: text.clone(),
        git_tag_label_background: node,
        git_tag_label_border: border,
        pie_colors: std::array::from_fn(|index| hex(series(index))),
        pie_title_text_color: text.clone(),
        pie_section_text_color: text.clone(),
        pie_legend_text_color: text,
        pie_stroke_color: hex(colors.background),
        pie_outer_stroke_color: muted_text,
        pie_opacity: 1.0,
        ..base
    }
}

/// An opaque token as `#rrggbb`.
fn hex(color: Oklch) -> String {
    let srgb = color.to_srgb();
    let channel = |value: f32| {
        // Clamped to [0, 255] before narrowing, so the cast is exact.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "clamped to [0, 255] first"
        )]
        let byte = (value.clamp(0.0, 1.0) * 255.0).round() as u8;
        byte
    };
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(srgb.r),
        channel(srgb.g),
        channel(srgb.b)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flowchart_renders_into_a_sized_svg() {
        let diagram =
            render("flowchart TD\n  A[Browser] --> B[Web]\n").expect("a valid flowchart renders");
        assert!(
            f32::from_bits(diagram.width_bits) >= 1.0 && f32::from_bits(diagram.height_bits) >= 1.0
        );
        let svg = diagram.source;
        assert!(svg.contains("<svg") && svg.contains("Browser"));
    }

    #[test]
    fn invalid_source_renders_nothing() {
        assert!(render("this is not a diagram {{{").is_none());
        assert!(render("   \n").is_none());
    }
}
