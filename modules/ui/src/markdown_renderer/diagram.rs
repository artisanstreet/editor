//! Mermaid fences drawn as diagrams.
//!
//! A closed `mermaid` fence lays out through `mermaid-rs-renderer` into an
//! SVG painted in the transcript's own tokens, and GPUI rasterizes that SVG
//! as an ordinary image. Layout runs synchronously so the card claims its
//! final size in the frame the fence first renders: rasterization then
//! fills a box that never moves, and the transcript below never shifts.
//!
//! Layout measures labels against system fonts, and the first measurement
//! loads them all. [`warm_up`] spends that once on a background thread, so
//! the first diagram does not stall a frame on the font scan.
//!
//! Source the renderer rejects — or panics on — yields no diagram, and the
//! caller keeps drawing the fence as code.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::{Arc, Once};

use gpui::{Image, ImageFormat};
use mermaid_rs_renderer::{
    LayoutConfig, Theme, compute_layout, measure_svg_dimensions, parse_mermaid_strict, render_svg,
};

use crate::theme::{ArtisanTheme, Oklch, ThemeMode};

/// Diagrams kept per thread, keyed by source and theme mode. A streaming
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
    pub(super) image: Arc<Image>,
    pub(super) width: f32,
    pub(super) height: f32,
}

type Memo = VecDeque<(u64, Option<Rc<MermaidDiagram>>)>;

thread_local! {
    static MEMO: RefCell<Memo> = const { RefCell::new(VecDeque::new()) };
}

/// Loads the system fonts layout measures against, once, off the UI thread.
pub(super) fn warm_up() {
    static WARM: Once = Once::new();
    WARM.call_once(|| {
        let spawned = std::thread::Builder::new()
            .name("mermaid-font-warmup".into())
            .spawn(|| {
                let _ = layout_svg("flowchart LR\n  A --> B", &Theme::modern());
            });
        // Without the thread the first diagram loads the fonts itself.
        drop(spawned);
    });
}

/// The diagram for `source` in `theme`, or `None` when it does not render.
pub(super) fn mermaid_diagram(source: &str, theme: &ArtisanTheme) -> Option<Rc<MermaidDiagram>> {
    let key = memo_key(source, theme.mode);
    if let Some(hit) = MEMO.with_borrow(|memo| {
        memo.iter()
            .find(|(entry, _)| *entry == key)
            .map(|(_, diagram)| diagram.clone())
    }) {
        return hit;
    }
    let diagram = render(source, theme).map(Rc::new);
    MEMO.with_borrow_mut(|memo| {
        if memo.len() == MEMO_CAPACITY {
            memo.pop_front();
        }
        memo.push_back((key, diagram.clone()));
    });
    diagram
}

fn memo_key(source: &str, mode: ThemeMode) -> u64 {
    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    matches!(mode, ThemeMode::Dark).hash(&mut hasher);
    hasher.finish()
}

fn render(source: &str, theme: &ArtisanTheme) -> Option<MermaidDiagram> {
    if source.trim().is_empty() || source.len() > MAX_SOURCE_BYTES {
        return None;
    }
    let (svg, width, height) = layout_svg(source, &diagram_theme(theme))?;
    let sized = width.is_finite()
        && height.is_finite()
        && width >= 1.0
        && height >= 1.0
        && width <= MAX_EDGE_PX
        && height <= MAX_EDGE_PX;
    sized.then(|| MermaidDiagram {
        image: Arc::new(Image::from_bytes(ImageFormat::Svg, svg.into_bytes())),
        width,
        height,
    })
}

/// Parses, lays out, and serializes `source`. The renderer is third-party
/// layout code over model-written input, so a panic counts as a rejection
/// rather than taking the window down.
fn layout_svg(source: &str, theme: &Theme) -> Option<(String, f32, f32)> {
    catch_unwind(AssertUnwindSafe(|| {
        let parsed = parse_mermaid_strict(source).ok()?;
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

    let base = match theme.mode {
        ThemeMode::Light => Theme::modern(),
        ThemeMode::Dark => Theme::dark(),
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
        let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
        let diagram = mermaid_diagram("flowchart TD\n  A[Browser] --> B[Web]\n", &theme)
            .expect("a valid flowchart renders");
        assert!(diagram.width >= 1.0 && diagram.height >= 1.0);
        let svg = std::str::from_utf8(&diagram.image.bytes).expect("svg is utf-8");
        assert!(svg.contains("<svg") && svg.contains("Browser"));
    }

    #[test]
    fn invalid_source_renders_nothing() {
        let theme = ArtisanTheme::for_mode(ThemeMode::Light);
        assert!(mermaid_diagram("this is not a diagram {{{", &theme).is_none());
        assert!(mermaid_diagram("   \n", &theme).is_none());
    }

    #[test]
    fn composite_blends_toward_the_color() {
        let black = Oklch::new(0.0, 0.0, 0.0);
        let white = Oklch::new(1.0, 0.0, 0.0);
        assert_eq!(composite_hex(black, white, 1.0), "#ffffff");
        assert_eq!(composite_hex(black, white, 0.0), "#000000");
    }
}
