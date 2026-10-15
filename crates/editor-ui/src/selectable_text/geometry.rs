//! Hit and paint geometry shared by ordinary and projected selectable leaves.
//! Projection does not create another gesture or clipboard document.
use crate::markdown_renderer::native_projection::{
    ProjectedCluster, ProjectedNativeRow, VisualClusterRow,
};
use gpui::{Bounds, Hsla, Pixels, Point, TextLayout, Window, point, size};
use std::ops::Range;
use std::sync::Arc;

#[derive(Clone)]
pub(super) enum LeafLayout {
    Native(TextLayout, Option<Arc<[VisualClusterRow]>>),
    Projected {
        row: Arc<ProjectedNativeRow>,
        bounds: Bounds<Pixels>,
        height: Pixels,
    },
}
impl LeafLayout {
    pub(super) fn bounds(&self) -> Bounds<Pixels> {
        match self {
            Self::Native(layout, _) => layout.bounds(),
            Self::Projected { bounds, .. } => *bounds,
        }
    }
    /// Logical bounds of the drawn row, independently of the canonical
    /// source text shared by every projected fragment.
    pub(super) fn source_range(&self, text_len: usize) -> Range<usize> {
        match self {
            Self::Native(..) => 0..text_len,
            Self::Projected { row, .. } => {
                let start = row.source.ranges.iter().map(|range| range.start)
                    .min().unwrap_or(row.source_start);
                let end = row.source.ranges.iter().map(|range| range.end)
                    .max().unwrap_or(start);
                start..end
            }
        }
    }
    pub(super) fn line_height(&self) -> Pixels {
        match self {
            Self::Native(layout, _) => layout.line_height(),
            Self::Projected { height, .. } => *height,
        }
    }
    pub(super) fn position_for_index(&self, index: usize) -> Option<Point<Pixels>> {
        match self {
            Self::Native(layout, Some(rows)) => {
                for row in rows.iter() {
                    if let Some(x) = cluster_position(&row.clusters, index) {
                        return Some(
                            layout.bounds().origin
                                + point(x, layout.line_height() * row.ordinal as f32),
                        );
                    }
                }
                None
            }
            Self::Native(layout, None) => layout.position_for_index(index),
            Self::Projected { row, bounds, .. } => cluster_position(&row.clusters, index)
                .map(|x| bounds.origin + point(x, Pixels::ZERO)),
        }
    }
    pub(super) fn index_for_position(&self, position: Point<Pixels>) -> Result<usize, usize> {
        match self {
            Self::Native(layout, Some(rows)) => {
                let row = native_row(layout, rows, position).ok_or(0usize)?;
                let x = position.x - layout.bounds().left();
                let index = row
                    .clusters
                    .iter()
                    .find(|cell| x >= cell.left && x < cell.right)
                    .map_or_else(
                        || cell_caret(&row.clusters, x, row.source_start),
                        |cell| cell.source.start,
                    );
                if layout.bounds().contains(&position) {
                    Ok(index)
                } else {
                    Err(index)
                }
            }
            Self::Native(layout, None) => layout.index_for_position(position),
            Self::Projected { row, bounds, .. } => {
                let x = position.x - bounds.left();
                if let Some(cell) = row
                    .clusters
                    .iter()
                    .find(|cell| x >= cell.left && x < cell.right)
                {
                    if bounds.contains(&position) {
                        Ok(cell.source.start)
                    } else {
                        Err(cell.source.start)
                    }
                } else {
                    Err(self.caret(position))
                }
            }
        }
    }
    pub(super) fn caret(&self, position: Point<Pixels>) -> usize {
        match self {
            Self::Native(layout, Some(rows)) => {
                native_row(layout, rows, position).map_or(0, |row| {
                    cell_caret(
                        &row.clusters,
                        position.x - layout.bounds().left(),
                        row.source_start,
                    )
                })
            }
            Self::Native(layout, None) => {
                crate::text_navigation::caret_index_for_position(layout, position)
            }
            Self::Projected { row, bounds, .. } => {
                let x = position.x - bounds.left();
                cell_caret(&row.clusters, x, row.source_start)
            }
        }
    }
    pub(super) fn paint_selection(&self, range: &Range<usize>, color: Hsla, window: &mut Window) {
        match self {
            Self::Native(layout, Some(rows)) => {
                for row in rows.iter() {
                    let origin = layout.bounds().origin
                        + point(Pixels::ZERO, layout.line_height() * row.ordinal as f32);
                    paint_cells(
                        &row.clusters,
                        origin,
                        layout.line_height(),
                        range,
                        color,
                        window,
                    );
                    if row
                        .break_offset
                        .is_some_and(|offset| range.start <= offset && range.end > offset)
                    {
                        window.paint_quad(gpui::fill(
                            Bounds::new(
                                origin
                                    + point(
                                        if row.break_rtl {
                                            -layout.line_height() * 0.25
                                        } else {
                                            row.width
                                        },
                                        Pixels::ZERO,
                                    ),
                                size(layout.line_height() * 0.25, layout.line_height()),
                            ),
                            color,
                        ));
                    }
                }
            }
            Self::Native(layout, None) => {
                super::paint_native_selection(layout, range, color, window)
            }
            Self::Projected {
                row,
                bounds,
                height,
            } => {
                paint_cells(&row.clusters, bounds.origin, *height, range, color, window);
            }
        }
    }
}

fn native_row<'a>(
    layout: &TextLayout,
    rows: &'a [VisualClusterRow],
    position: Point<Pixels>,
) -> Option<&'a VisualClusterRow> {
    let y = f32::from(position.y - layout.bounds().top()).max(0.0);
    let ordinal = (y / f32::from(layout.line_height())).floor() as usize;
    rows.iter().min_by_key(|row| row.ordinal.abs_diff(ordinal))
}
fn cluster_position(cells: &[ProjectedCluster], index: usize) -> Option<Pixels> {
    if let Some(cell) = cells.iter().find(|cell| cell.source.start == index) {
        return Some(if cell.rtl { cell.right } else { cell.left });
    }
    cells
        .iter()
        .find(|cell| cell.source.end == index)
        .map(|cell| if cell.rtl { cell.left } else { cell.right })
}
fn cell_caret(cells: &[ProjectedCluster], x: Pixels, empty: usize) -> usize {
    if let Some(cell) = cells.iter().find(|cell| x >= cell.left && x < cell.right) {
        let left_half = x - cell.left < (cell.right - cell.left) / 2.0;
        return if left_half == cell.rtl {
            cell.source.end
        } else {
            cell.source.start
        };
    }
    cells
        .iter()
        .flat_map(|cell| {
            [
                (
                    cell.left,
                    if cell.rtl {
                        cell.source.end
                    } else {
                        cell.source.start
                    },
                ),
                (
                    cell.right,
                    if cell.rtl {
                        cell.source.start
                    } else {
                        cell.source.end
                    },
                ),
            ]
        })
        .min_by(|(a, _), (b, _)| {
            (*a - x)
                .abs()
                .partial_cmp(&(*b - x).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map_or(empty, |(_, index)| index)
}
fn paint_cells(
    cells: &[ProjectedCluster],
    origin: Point<Pixels>,
    height: Pixels,
    range: &Range<usize>,
    color: Hsla,
    window: &mut Window,
) {
    for cell in cells {
        if cell.source.start < range.end && cell.source.end > range.start && cell.right > cell.left
        {
            window.paint_quad(gpui::fill(
                Bounds::new(
                    origin + point(cell.left, Pixels::ZERO),
                    size(cell.right - cell.left, height),
                ),
                color,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown_renderer::native_projection::ParagraphWrapIndex;
    use gpui::{TextRun, TextSystem, WindowTextSystem, WrappedLineLayout, px};

    #[test]
    fn real_native_bidi_clusters_drive_existing_global_selection_geometry() {
        let system = Arc::new(TextSystem::new(
            gpui_platform::current_platform(true).text_system(),
        ));
        system
            .add_fonts(editor_assets::fonts::bundled_fonts())
            .unwrap();
        let system = WindowTextSystem::new(system);
        let text = "office affinity e\u{301} سلام بالعالم שלום prefix suffix";
        let styles = [TextRun {
            len: text.len(),
            font: gpui::font("Spline Sans"),
            color: gpui::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
            letter_spacing: None,
        }];
        let physical: Vec<_> = system
            .shape_text(text, px(16.0), &styles, Some(px(95.0)), None)
            .unwrap()
            .iter()
            .map(|line| {
                Arc::new(WrappedLineLayout {
                    unwrapped_layout: line.unwrapped_layout.clone(),
                    wrap_boundaries: line.wrap_boundaries.clone(),
                    wrap_width: line.wrap_width,
                })
            })
            .collect();
        let index = ParagraphWrapIndex::from_native(&physical, text).unwrap();
        let rows = index.project(&physical, 0..index.row_count()).unwrap();
        let native_cells = index.visual_rows(&physical, 0..index.row_count()).unwrap();
        assert_eq!(native_cells.len(), rows.len());
        for (native, projected) in native_cells.iter().zip(&rows) {
            assert_eq!(native.clusters.len(), projected.clusters.len());
            for (a, b) in native.clusters.iter().zip(&projected.clusters) {
                assert_eq!(
                    (&a.source, a.left, a.right, a.rtl),
                    (&b.source, b.left, b.right, b.rtl)
                );
            }
        }
        let mut rtl_seen = false;
        for row in rows {
            let row = Arc::new(row);
            let layout = LeafLayout::Projected {
                row: row.clone(),
                bounds: Bounds::new(point(px(20.0), px(30.0)), size(row.layout.width, px(24.0))),
                height: px(24.0),
            };
            for cell in &row.clusters {
                assert!(text.is_char_boundary(cell.source.start));
                assert!(text.is_char_boundary(cell.source.end));
                if cell.right <= cell.left {
                    continue;
                }
                rtl_seen |= cell.rtl;
                let x = cell.left + (cell.right - cell.left) * 0.25;
                let expected = if cell.rtl {
                    cell.source.end
                } else {
                    cell.source.start
                };
                assert_eq!(layout.caret(point(px(20.0) + x, px(35.0))), expected);
                assert_eq!(
                    layout.index_for_position(point(px(20.0) + x, px(35.0))),
                    Ok(cell.source.start)
                );
            }
        }
        assert!(rtl_seen, "must exercise native Arabic or Hebrew clusters");
        let cloned: Vec<_> = physical
            .iter()
            .map(|line| {
                Arc::new(WrappedLineLayout {
                    unwrapped_layout: line.unwrapped_layout.clone(),
                    wrap_boundaries: line.wrap_boundaries.clone(),
                    wrap_width: line.wrap_width,
                })
            })
            .collect();
        assert!(matches!(
            index.project(&cloned, 0..1),
            Err(crate::markdown_renderer::native_projection::ProjectionError::LayoutChanged)
        ));
    }
}
