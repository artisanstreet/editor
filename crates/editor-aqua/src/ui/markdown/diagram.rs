use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, OnceLock};

use mermaid_rs_renderer::{
    LayoutConfig, Theme, compute_layout, measure_svg_dimensions, parse_mermaid_strict, render_svg,
};

use super::DiagramSvg;
use crate::ui::theme::{ArtisanTheme, Oklch, ThemeMode};

pub(super) mod view;

const MEMO_CAPACITY: usize = 32;
const MAX_SOURCE_BYTES: usize = 32 * 1024;
const MAX_EDGE_PX: f32 = 8192.0;
const MAX_MEMO_BYTES: usize = 8 * 1024 * 1024;
const MAX_SVG_BYTES: usize = 2 * 1024 * 1024;
const MAX_SEPARATORS: usize = 512;
const MAX_GRAPH_ITEMS: usize = 512;
const FONT_FAMILY: &str = "Spline Sans";
const FONT_SIZE_PX: f32 = 14.0;

#[derive(Debug, Default)]
pub(super) struct DiagramCache {
    entries: VecDeque<(String, Option<Arc<DiagramSvg>>)>,
}

impl DiagramCache {
    pub(super) fn get(&mut self, source: &str) -> Option<Arc<DiagramSvg>> {
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
                > MAX_MEMO_BYTES
        {
            self.entries.pop_front();
        }
        diagram
    }
}

fn render(source: &str) -> Option<DiagramSvg> {
    if source.trim().is_empty()
        || source.len() > MAX_SOURCE_BYTES
        || source
            .bytes()
            .filter(|byte| matches!(byte, b'\n' | b';' | b','))
            .count()
            > MAX_SEPARATORS
    {
        return None;
    }
    let themes =
        [ThemeMode::Dark, ThemeMode::Light].map(|mode| diagram_theme(ArtisanTheme::for_mode(mode)));
    let ([dark, light], width, height) = layout_svg(source, &themes)?;
    (dark.len().max(light.len()) <= MAX_SVG_BYTES
        && width.is_finite()
        && height.is_finite()
        && width >= 1.0
        && height >= 1.0
        && width <= MAX_EDGE_PX
        && height <= MAX_EDGE_PX)
        .then(|| DiagramSvg {
            source: Arc::from(dark),
            light_source: Arc::from(light),
            width_bits: width.to_bits(),
            height_bits: height.to_bits(),
        })
}

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
            > MAX_GRAPH_ITEMS
        {
            return None;
        }
        let config = LayoutConfig {
            fast_text_metrics: true,
            ..LayoutConfig::default()
        };
        let layout = compute_layout(&parsed.graph, &themes[0], &config);
        let size = measure_svg_dimensions(&layout, &config, None);
        let svgs = [
            outline_text(&render_svg(&layout, &themes[0], &config))?,
            outline_text(&render_svg(&layout, &themes[1], &config))?,
        ];
        Some((svgs, size.width, size.height))
    }))
    .ok()
    .flatten()
}

fn outline_text(source: &str) -> Option<String> {
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    let fonts = FONTS.get_or_init(|| {
        let mut fonts = usvg::fontdb::Database::new();
        for font in editor_assets::fonts::ALL {
            for face in font.static_faces {
                fonts.load_font_data(face.to_vec());
            }
        }
        fonts.set_sans_serif_family(FONT_FAMILY);
        Arc::new(fonts)
    });
    let options = usvg::Options {
        font_family: FONT_FAMILY.to_owned(),
        fontdb: Arc::clone(fonts),
        ..Default::default()
    };
    let tree = usvg::Tree::from_str(source, &options).ok()?;
    Some(tree.to_string(&usvg::WriteOptions::default()))
}

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

fn hex(color: Oklch) -> String {
    let srgb = color.to_srgb();
    let channel = |value: f32| format!("{:02x}", (value.clamp(0.0, 1.0) * 255.0).round() as u8);
    format!("#{}{}{}", channel(srgb.r), channel(srgb.g), channel(srgb.b))
}

#[cfg(test)]
#[path = "../../../tests/markdown/diagram.rs"]
mod tests;
