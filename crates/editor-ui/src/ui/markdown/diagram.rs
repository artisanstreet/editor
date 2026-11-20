use std::collections::VecDeque;
use std::panic::AssertUnwindSafe;
use std::sync::{Arc, OnceLock};

use mermaid_rs_renderer::ir::Graph;
use mermaid_rs_renderer::{
    LayoutConfig, Theme, compute_layout, measure_svg_dimensions, parse_mermaid_strict, render_svg,
};

use aqua_fonts_system::{FaceBytes, FontDatabase, Query, Stretch, Style, Weight};

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
const FALLBACK_SAMPLES: [char; 7] = ['日', 'あ', '한', 'ع', 'א', 'Ж', 'क'];
const FALLBACK_FAMILIES: [&str; 10] = [
    "Segoe UI",
    "Microsoft YaHei UI",
    "Yu Gothic UI",
    "Malgun Gothic",
    "Nirmala UI",
    "Noto Sans",
    "Noto Sans CJK SC",
    "Noto Sans Arabic",
    "Noto Sans Devanagari",
    "DejaVu Sans",
];

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
    let ([dark, light], width, height, summary) = layout_svg(source, &themes)?;
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
            summary: Arc::from(summary),
        })
}

fn layout_svg(source: &str, themes: &[Theme; 2]) -> Option<([String; 2], f32, f32, String)> {
    let laid_out = diagnostics::crash::catch(AssertUnwindSafe(|| {
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
        Some((svgs, size.width, size.height, summary(&parsed.graph)))
    }));
    match laid_out {
        Ok(svg) => svg,
        Err(panic) => {
            tracing::error!(
                bytes = source.len(),
                %panic,
                "the diagram renderer panicked; the diagram shows as its source"
            );
            None
        }
    }
}

fn summary(graph: &Graph) -> String {
    let name = |id: &str| {
        graph
            .nodes
            .get(id)
            .map(|node| node.label.trim())
            .filter(|label| !label.is_empty())
            .unwrap_or(id)
            .to_owned()
    };
    let mut nodes: Vec<_> = graph.nodes.values().collect();
    nodes.sort_by_key(|node| {
        (
            graph
                .node_order
                .get(&node.id)
                .copied()
                .unwrap_or(usize::MAX),
            node.id.as_str(),
        )
    });
    let mut parts = Vec::new();
    if let Some(title) = graph.pie_title.as_ref().or(graph.gantt_title.as_ref()) {
        parts.push(title.clone());
    }
    if !nodes.is_empty() {
        let labels: Vec<_> = nodes.iter().map(|node| name(&node.id)).collect();
        parts.push(format!("Nodes: {}", labels.join(", ")));
    }
    if !graph.edges.is_empty() {
        let edges: Vec<_> = graph
            .edges
            .iter()
            .map(|edge| {
                let arrow = if edge.directed { "→" } else { "—" };
                let relation = format!("{} {arrow} {}", name(&edge.from), name(&edge.to));
                match edge.label.as_deref().map(str::trim) {
                    Some(label) if !label.is_empty() => format!("{relation}: {label}"),
                    _ => relation,
                }
            })
            .collect();
        parts.push(format!("Links: {}", edges.join("; ")));
    }
    if !graph.pie_slices.is_empty() {
        let slices: Vec<_> = graph
            .pie_slices
            .iter()
            .map(|slice| format!("{}: {}", slice.label, slice.value))
            .collect();
        parts.push(format!("Slices: {}", slices.join(", ")));
    }
    if !graph.gantt_tasks.is_empty() {
        let tasks: Vec<_> = graph
            .gantt_tasks
            .iter()
            .map(|task| task.label.clone())
            .collect();
        parts.push(format!("Tasks: {}", tasks.join(", ")));
    }
    parts.join(". ")
}

fn diagram_fonts() -> &'static Arc<usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    FONTS.get_or_init(|| Arc::new(fonts_with_fallbacks(&mut FontDatabase::system())))
}

fn fonts_with_fallbacks(system: &mut FontDatabase) -> usvg::fontdb::Database {
    let mut fonts = usvg::fontdb::Database::new();
    for face in editor_assets::fonts::FACES {
        fonts.load_font_data(face.to_vec());
    }
    fonts.set_sans_serif_family(FONT_FAMILY);
    let mut chosen: Vec<FaceBytes> = Vec::new();
    let preferred = FALLBACK_FAMILIES.iter().filter_map(|family| {
        system.query(&Query {
            family,
            weight: Weight(400),
            stretch: Stretch::NORMAL,
            style: Style::Normal,
        })
    });
    let candidates: Vec<_> = preferred
        .chain(system.faces().map(|face| face.id))
        .collect();
    for sample in FALLBACK_SAMPLES {
        if editor_assets::fonts::FACES
            .iter()
            .map(|face| (*face, 0))
            .chain(chosen.iter().map(|face| (face.bytes(), face.index())))
            .any(|(bytes, index)| covers(bytes, index, sample))
        {
            continue;
        }
        if let Some(face) = candidates
            .iter()
            .filter_map(|id| system.face_bytes(*id))
            .find(|face| covers(face.bytes(), face.index(), sample))
        {
            chosen.push(face);
        }
    }
    for face in chosen {
        fonts.load_font_source(usvg::fontdb::Source::Binary(Arc::new(SystemFace(face))));
    }
    fonts
}

fn covers(bytes: &[u8], index: u32, character: char) -> bool {
    skrifa::FontRef::from_index(bytes, index).is_ok_and(|font| {
        skrifa::charmap::Charmap::new(&font)
            .map(character)
            .is_some()
    })
}

struct SystemFace(FaceBytes);

impl AsRef<[u8]> for SystemFace {
    fn as_ref(&self) -> &[u8] {
        self.0.bytes()
    }
}

fn diagram_options() -> usvg::Options<'static> {
    usvg::Options {
        font_family: FONT_FAMILY.to_owned(),
        fontdb: Arc::clone(diagram_fonts()),
        ..Default::default()
    }
}

fn outline_text(source: &str) -> Option<String> {
    let tree = usvg::Tree::from_str(source, &diagram_options()).ok()?;
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
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the channel is clamped to 0..=255 first"
    )]
    let channel = |value: f32| format!("{:02x}", (value.clamp(0.0, 1.0) * 255.0).round() as u8);
    format!("#{}{}{}", channel(srgb.r), channel(srgb.g), channel(srgb.b))
}

#[cfg(test)]
#[path = "../../../tests/markdown/diagram.rs"]
mod tests;
