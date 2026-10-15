use super::*;
use gpui::{FontFeatures, TextRun, TextSystem, WindowTextSystem};

fn native() -> WindowTextSystem {
    // A real headless native backend, not TestAppContext's Noop metrics.
    let platform = gpui_platform::current_platform(true);
    let backend = platform.text_system();
    let system = Arc::new(TextSystem::new(backend));
    system
        .add_fonts(editor_assets::fonts::bundled_fonts())
        .expect("register bundled fonts");
    WindowTextSystem::new(system)
}
fn shape(system: &WindowTextSystem, text: &str, width: f32) -> Vec<Arc<WrappedLineLayout>> {
    let mut font = gpui::font("Spline Sans");
    font.features = FontFeatures(Arc::new(vec![("liga".into(), 1)]));
    let runs = [TextRun {
        len: text.len(),
        font,
        color: gpui::black(),
        background_color: None,
        underline: None,
        strikethrough: None,
        letter_spacing: None,
    }];
    system
        .shape_text(
            gpui::SharedString::new(text),
            px(16.0),
            &runs,
            Some(px(width)),
            None,
        )
        .unwrap()
        .iter()
        .map(|line| {
            Arc::new(WrappedLineLayout {
                unwrapped_layout: line.unwrapped_layout.clone(),
                wrap_boundaries: line.wrap_boundaries.clone(),
                wrap_width: line.wrap_width,
            })
        })
        .collect()
}

#[test]
fn native_unicode_bidi_and_ligature_glyphs_survive_projection_without_reshaping() {
    let system = native();
    let text = "office affinity e\u{301} سلام بالعالم שלום prefix סופית suffix";
    let physical = shape(&system, text, 95.0);
    let glyphs: Vec<_> = physical[0]
        .unwrapped_layout
        .runs
        .iter()
        .flat_map(|run| run.glyphs.iter())
        .collect();
    assert!(
        glyphs.iter().any(|glyph| glyph.id.0 != 0
            && text[glyph.index..]
                .chars()
                .next()
                .is_some_and(|c| ('א'..='ת').contains(&c))),
        "native font fallback must cover Hebrew"
    );
    assert!(
        glyphs.iter().any(|glyph| glyph.id.0 != 0
            && text[glyph.index..]
                .chars()
                .next()
                .is_some_and(|c| ('\u{0600}'..='\u{06ff}').contains(&c))),
        "native font fallback must cover Arabic"
    );
    assert!(
        glyphs.windows(2).any(|pair| pair[0].index > pair[1].index),
        "fixture must exercise real bidi glyph order"
    );
    let lam_alef = text.find("لا").expect("fixture contains joining lam-alef");
    assert!(
        !glyphs
            .iter()
            .any(|glyph| glyph.index == lam_alef + 'ل'.len_utf8()),
        "fixture must exercise native lam-alef joining as a multi-character cluster"
    );
    let index = ParagraphWrapIndex::from_native(&physical, &text).unwrap();
    assert!(index.row_count() > 1);
    let projected = index.project(&physical, 0..index.row_count()).unwrap();
    let mut original = Vec::new();
    for run in &physical[0].unwrapped_layout.runs {
        for glyph in &run.glyphs {
            original.push((run.font_id, glyph.id, glyph.index, glyph.position));
        }
    }
    let mut observed = Vec::new();
    for (row, descriptor) in projected.iter().zip(index.rows.iter()) {
        for run in &row.layout.runs {
            for glyph in &run.glyphs {
                let mut position = glyph.position;
                position.x += descriptor.origin_x;
                observed.push((run.font_id, glyph.id, glyph.index, position));
            }
        }
        for range in &row.source.ranges {
            assert!(text.get(range.clone()).is_some());
        }
    }
    assert_eq!(observed.len(), original.len());
    for (observed, original) in observed.iter().zip(original.iter()) {
        assert_eq!(
            (observed.0, observed.1, observed.2),
            (original.0, original.1, original.2)
        );
        assert!((f32::from(observed.3.x - original.3.x)).abs() <= 0.001);
        assert_eq!(observed.3.y, original.3.y);
    }
}

#[test]
fn native_wrap_index_keeps_only_requested_hot_glyphs() {
    let system = native();
    let text = "A long authored paragraph with ffi and Unicode β. ".repeat(2048);
    let physical = shape(&system, &text, 480.0);
    let index = ParagraphWrapIndex::from_native(&physical, &text).unwrap();
    assert!(index.row_count() > 100);
    let projected = index
        .project(&physical, index.row_count() - 4..index.row_count())
        .unwrap();
    let hot = projected
        .iter()
        .map(|row| {
            row.layout
                .runs
                .iter()
                .map(|run| run.glyphs.len())
                .sum::<usize>()
        })
        .sum::<usize>();
    let whole = physical
        .iter()
        .map(|line| {
            line.unwrapped_layout
                .runs
                .iter()
                .map(|run| run.glyphs.len())
                .sum::<usize>()
        })
        .sum::<usize>();
    assert!(hot * 100 < whole, "only the requested rows carry glyphs");
    assert!(
        projected
            .iter()
            .flat_map(|row| row.source.ranges.iter())
            .all(|range| range.start > text.len() / 2)
    );
}

#[test]
fn projected_source_offsets_remain_global_across_unequal_physical_lines() {
    let system = native();
    let text = "first line\nβ\nA much longer third physical line with ffi";
    let physical = shape(&system, text, 120.0);
    let index = ParagraphWrapIndex::from_native(&physical, &text).unwrap();
    let projected = index.project(&physical, 0..index.row_count()).unwrap();
    let third = text.find("A much").unwrap();
    let rows: Vec<_> = projected
        .iter()
        .filter(|row| row.physical_line == 2)
        .collect();
    assert!(!rows.is_empty());
    assert!(
        rows.iter()
            .flat_map(|row| row.layout.runs.iter())
            .flat_map(|run| run.glyphs.iter())
            .all(|glyph| glyph.index >= third)
    );
    assert!(
        rows.iter()
            .flat_map(|row| row.source.ranges.iter())
            .all(|range| range.start >= third && text.get(range.clone()).is_some())
    );
}
