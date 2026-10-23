//! Original native glyph rows share one canonical selection source.
use super::*;
use crate::markdown_renderer::native_projection::ParagraphWrapIndex;
use gpui::{EntityId, WindowTextSystem, WrappedLineLayout, point, px, size};

#[test]
#[expect(clippy::too_many_lines, reason = "one scenario, step by step")]
#[expect(
    clippy::significant_drop_tightening,
    reason = "the flagged bindings are references or plain values, not guards"
)]
fn native_row_window_drag_bounds_copy_original_utf8_without_hidden_prefix_or_tail() {
    let platform = gpui_platform::current_platform(true);
    let system = Arc::new(gpui::TextSystem::new(platform.text_system()));
    system
        .add_fonts(editor_assets::fonts::bundled_fonts())
        .unwrap();
    let worker = WindowTextSystem::new(system);
    let raw: SharedString = format!("{}\n", "界🙂 alpha βeta e\u{301} ".repeat(100)).into();
    let visible: SharedString = raw.strip_suffix('\n').unwrap().to_owned().into();
    let runs = [TextRun {
        len: visible.len(),
        font: gpui::font("Spline Sans"),
        color: gpui::black(),
        background_color: None,
        underline: None,
        strikethrough: None,
        letter_spacing: None,
    }];
    let physical: Vec<_> = worker
        .shape_text(visible.clone(), px(16.0), &runs, Some(px(140.0)), None)
        .unwrap()
        .iter()
        .map(|line| {
            Arc::new(WrappedLineLayout {
                unwrapped_layout: std::sync::Arc::clone(&line.unwrapped_layout),
                wrap_boundaries: line.wrap_boundaries.clone(),
                wrap_width: line.wrap_width,
            })
        })
        .collect();
    let index = ParagraphWrapIndex::from_native(&physical, &visible).unwrap();
    assert!(index.row_count() > 8);
    let rows = index.project(&physical, 2..5).unwrap();
    let source = SelectionSource::single("native-window", vec![42], raw.clone(), "");
    let key = LeafKey::semantic("native-window", &[42]);
    let roster: Vec<_> = rows
        .into_iter()
        .enumerate()
        .map(|(ordinal, row)| {
            let bounds = Bounds::new(
                point(px(0.0), px(crate::f32_of_count(ordinal) * 28.0)),
                size(px(140.0), px(28.0)),
            );
            LiveLeaf {
                observed: Observed {
                    key,
                    scope: "native-window".into(),
                    order: Rc::from([42]),
                    separator: "",
                    text: visible.clone(),
                    source_slice: Some(0..visible.len()),
                    copies: Rc::from([]),
                    view: EntityId::from(ordinal as u64 + 1),
                    bounds,
                },
                layout: LeafLayout::Projected {
                    row: Arc::new(row),
                    bounds,
                    height: px(28.0),
                },
                hovered: false,
                focus: None,
                links: Rc::from([]),
                on_link: None,
            }
        })
        .collect();
    let first = roster.first().unwrap();
    let last = roster.last().unwrap();
    let start = first.layout.source_range(visible.len()).start;
    let end = last.layout.source_range(visible.len()).end;
    assert!(
        start > 0 && end < visible.len(),
        "fixture hides both original source ends"
    );
    let above = drag_target(&roster, point(px(70.0), px(-1.0))).unwrap();
    let below = drag_target(&roster, point(px(70.0), px(90.0))).unwrap();
    assert_eq!(above, (key, start, start));
    assert_eq!(below, (key, end, end));
    // Each native row still supplies its original paragraph cluster offsets.
    for live in [first, last] {
        let LeafLayout::Projected { row, bounds, .. } = &live.layout else {
            unreachable!()
        };
        let cell = row
            .clusters
            .iter()
            .find(|cell| cell.right > cell.left)
            .unwrap();
        let position = bounds.origin + point((cell.left + cell.right) / 2.0, px(14.0));
        let hit = drag_target(&roster, position).unwrap();
        assert_eq!(hit.0, key);
        assert_eq!(hit.2, cell.source.start);
        assert!(raw.is_char_boundary(hit.1) && raw.is_char_boundary(hit.2));
    }
    let mut doc = document::Document::default();
    doc.register_source(&source, EntityId::from(1_u64));
    doc.reset_with_sources(&observed(&roster));
    doc.press(above.0, above.1, SelectionUnit::Character, false);
    doc.drag_to(below.0, below.1, below.2);
    assert_eq!(doc.selected_text().as_deref(), Some(&raw[start..end]));
    doc.select_source(first.observed.clone());
    assert_eq!(
        doc.selected_text().as_deref(),
        Some(raw.as_ref()),
        "whole-source copy preserves the raw final LF"
    );
}
