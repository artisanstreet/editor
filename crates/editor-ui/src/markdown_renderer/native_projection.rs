//! Projection of original native glyph records into bounded visual row windows.
//!
//! No substring is independently shaped. The original fonts, glyph IDs,
//! positions and cluster indices survive projection, including bidi runs.

use std::ops::Range;
use std::sync::{Arc, Weak};

use gpui::{LineLayout, Pixels, ShapedGlyph, ShapedRun, WrappedLineLayout, px};

const MAX_ROWS: usize = 128;
const MAX_PROJECTED_BYTES: usize = 2 * 1024 * 1024;
const MAX_INDEX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ProjectionError {
    #[error("The native paragraph window exceeds its row or glyph ownership budget")]
    WindowBudget,
    #[error("The native paragraph wrap index exceeds its memory budget")]
    IndexBudget,
    #[error("The native paragraph layout changed; rebuild its wrap index")]
    LayoutChanged,
    #[error("The native paragraph contains an invalid UTF-8 cluster boundary")]
    InvalidCluster,
}

/// Logical source fragments belonging to a projected visual row.
///
/// Bidi visual
/// order can produce several disjoint fragments; one source_slice cannot
/// represent that shape. This mapping never adds direction-control bytes.
#[derive(Debug)]
pub struct ProjectedSource {
    pub ranges: Vec<Range<usize>>,
}
impl ProjectedSource {
    pub const fn resident_bytes(&self) -> usize {
        self.ranges.capacity() * std::mem::size_of::<Range<usize>>()
    }
}

/// A visual cluster's original logical coverage and translated horizontal edges.
///
/// Several glyphs can belong to one cluster; hit testing must not invent an
/// intermediate UTF-8 caret inside a ligature or combining sequence.
#[derive(Debug, Clone)]
pub struct ProjectedCluster {
    pub source: Range<usize>,
    pub left: Pixels,
    pub right: Pixels,
    pub rtl: bool,
}

/// Visible hit/selection cells without copied native glyph records.
#[derive(Debug)]
pub struct VisualClusterRow {
    pub ordinal: usize,
    pub clusters: Vec<ProjectedCluster>,
    pub source_start: usize,
    pub break_offset: Option<usize>,
    pub width: Pixels,
    pub break_rtl: bool,
}

/// Only one admitted native row's glyphs, without a retained full layout.
#[derive(Debug)]
pub struct ProjectedNativeRow {
    // Geometry/pointer clones can outlive the page cache. Keep the exact ready
    // byte charge alive with every independently retained original glyph row.
    pub(crate) ready_budget: Option<Arc<tokio::sync::OwnedSemaphorePermit>>,
    pub layout: Arc<LineLayout>,
    /// Source indices in the layout remain original paragraph indices.
    pub source: ProjectedSource,
    pub physical_line: usize,
    pub source_start: usize,
    pub clusters: Vec<ProjectedCluster>,
}
impl ProjectedNativeRow {
    pub fn resident_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + std::mem::size_of::<LineLayout>()
            + 2 * std::mem::size_of::<usize>()
            + self.layout.runs.capacity() * std::mem::size_of::<ShapedRun>()
            + self
                .layout
                .runs
                .iter()
                .map(|run| run.glyphs.capacity() * std::mem::size_of::<ShapedGlyph>())
                .sum::<usize>()
            + self.source.resident_bytes()
            + self.clusters.capacity() * std::mem::size_of::<ProjectedCluster>()
    }
}

#[derive(Debug, Clone, PartialEq)]
struct Row {
    physical_line: usize,
    glyphs: Range<usize>,
    origin_x: Pixels,
    end_x: Pixels,
}
#[derive(Debug)]
struct PhysicalIndex {
    identity: Weak<WrappedLineLayout>,
    source_start: usize,
    cluster_starts: Vec<u32>,
    rtl: Vec<bool>,
    run_starts: Vec<usize>,
    base_rtl: bool,
}
impl PhysicalIndex {
    const fn resident_bytes(&self) -> usize {
        std::mem::size_of::<WrappedLineLayout>()
            + 2 * std::mem::size_of::<usize>()
            + self.cluster_starts.capacity() * std::mem::size_of::<u32>()
            + self.rtl.capacity() * std::mem::size_of::<bool>()
            + self.run_starts.capacity() * std::mem::size_of::<usize>()
    }
}

/// Compact original wrap boundaries. Drop the complete native layouts after
/// projecting the requested row range; retain only this index and hot rows.
#[derive(Debug)]
pub struct ParagraphWrapIndex {
    rows: Vec<Row>,
    text_bytes: usize,
    physical: Vec<PhysicalIndex>,
}
impl ParagraphWrapIndex {
    /// `physical` uses the same physical-line split as native text presentation.
    #[expect(
        clippy::too_many_lines,
        reason = "one sequential pass; splitting it would only thread its locals through helpers"
    )]
    pub fn from_native(
        physical: &[Arc<WrappedLineLayout>],
        text: &str,
    ) -> Result<Self, ProjectionError> {
        let text_bytes = text.len();
        if text_bytes > u32::MAX as usize {
            return Err(ProjectionError::IndexBudget);
        }
        let mut rows = Vec::new();
        let mut indexed = Vec::new();
        let mut source_start = 0;
        let mut index_bytes = 0usize;
        for (physical_line, line) in physical.iter().enumerate() {
            let native = &line.unwrapped_layout;
            let run_starts: Vec<usize> = native
                .runs
                .iter()
                .scan(0usize, |offset, run| {
                    let start = *offset;
                    *offset += run.glyphs.len();
                    Some(start)
                })
                .collect();
            let total = native
                .runs
                .iter()
                .map(|run| run.glyphs.len())
                .sum::<usize>();
            let estimated = total
                .saturating_add(1)
                .saturating_mul(std::mem::size_of::<u32>() + std::mem::size_of::<bool>())
                + run_starts.capacity() * std::mem::size_of::<usize>();
            if index_bytes.saturating_add(estimated) > MAX_INDEX_BYTES {
                return Err(ProjectionError::IndexBudget);
            }
            let source_end = source_start + native.len;
            let source = text
                .get(source_start..source_end)
                .ok_or(ProjectionError::InvalidCluster)?;
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the text was checked to fit u32 offsets"
            )]
            let mut clusters: Vec<u32> = native
                .runs
                .iter()
                .flat_map(|run| run.glyphs.iter().map(|g| g.index as u32))
                .chain([source.len() as u32])
                .collect();
            clusters.sort_unstable();
            clusters.dedup();
            if clusters
                .iter()
                .any(|index| !source.is_char_boundary(*index as usize))
            {
                return Err(ProjectionError::InvalidCluster);
            }
            let bidi = unicode_bidi::BidiInfo::new(source, None);
            let rtl = clusters
                .iter()
                .map(|index| {
                    bidi.levels
                        .get(*index as usize)
                        .is_some_and(unicode_bidi::Level::is_rtl)
                })
                .collect();
            let index = PhysicalIndex {
                identity: Arc::downgrade(line),
                source_start,
                cluster_starts: clusters,
                rtl,
                run_starts: run_starts.clone(),
                base_rtl: bidi
                    .paragraphs
                    .first()
                    .is_some_and(|paragraph| paragraph.level.is_rtl()),
            };
            index_bytes = index_bytes.saturating_add(index.resident_bytes());
            indexed.push(index);
            source_start = source_end.saturating_add(1);
            let mut first = 0;
            let mut origin_x = px(0.0);
            for boundary in &line.wrap_boundaries {
                let glyph = native
                    .runs
                    .get(boundary.run_ix)
                    .and_then(|run| run.glyphs.get(boundary.glyph_ix))
                    .ok_or(ProjectionError::LayoutChanged)?;
                let end = run_starts[boundary.run_ix] + boundary.glyph_ix;
                if end < first {
                    return Err(ProjectionError::LayoutChanged);
                }
                rows.push(Row {
                    physical_line,
                    glyphs: first..end,
                    origin_x,
                    end_x: glyph.position.x,
                });
                first = end;
                origin_x = glyph.position.x;
            }
            rows.push(Row {
                physical_line,
                glyphs: first..total,
                origin_x,
                end_x: native.width,
            });
            if index_bytes
                + rows.capacity() * std::mem::size_of::<Row>()
                + indexed.capacity() * std::mem::size_of::<PhysicalIndex>()
                > MAX_INDEX_BYTES
            {
                return Err(ProjectionError::IndexBudget);
            }
        }
        Ok(Self {
            rows,
            text_bytes,
            physical: indexed,
        })
    }
    pub const fn row_count(&self) -> usize {
        self.rows.len()
    }
    pub fn resident_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.rows.capacity() * std::mem::size_of::<Row>()
            + self.physical.capacity() * std::mem::size_of::<PhysicalIndex>()
            + self
                .physical
                .iter()
                .map(PhysicalIndex::resident_bytes)
                .sum::<usize>()
    }

    fn glyph_cell(
        row: &Row,
        indexed: &PhysicalIndex,
        native: &LineLayout,
        run: usize,
        glyph_ix: usize,
    ) -> ProjectedCluster {
        let glyph = &native.runs[run].glyphs[glyph_ix];
        let next = indexed
            .cluster_starts
            .partition_point(|index| *index as usize <= glyph.index);
        let end = indexed
            .cluster_starts
            .get(next)
            .copied()
            .map_or(native.len, |index| index as usize);
        let next_glyph = native.runs[run]
            .glyphs
            .get(glyph_ix + 1)
            .or_else(|| native.runs.get(run + 1).and_then(|run| run.glyphs.first()));
        ProjectedCluster {
            source: indexed.source_start + glyph.index..indexed.source_start + end,
            left: glyph.position.x - row.origin_x,
            right: next_glyph.map_or(row.end_x, |glyph| glyph.position.x) - row.origin_x,
            rtl: indexed
                .rtl
                .get(next.saturating_sub(1))
                .copied()
                .unwrap_or(false),
        }
    }

    /// Ordinary native text keeps its original glyph painter. Only currently
    /// visible rows acquire the SAME visual cells used by projected text.
    pub fn visual_rows(
        &self,
        physical: &[Arc<WrappedLineLayout>],
        window: Range<usize>,
    ) -> Result<Vec<VisualClusterRow>, ProjectionError> {
        if window.start > window.end || window.end > self.rows.len() || window.len() > MAX_ROWS {
            return Err(ProjectionError::WindowBudget);
        }
        let mut result = Vec::with_capacity(window.len());
        let mut bytes = result.capacity() * std::mem::size_of::<VisualClusterRow>();
        for ordinal in window {
            let row = &self.rows[ordinal];
            let physical = physical
                .get(row.physical_line)
                .ok_or(ProjectionError::LayoutChanged)?;
            let indexed = &self.physical[row.physical_line];
            if !indexed.identity.ptr_eq(&Arc::downgrade(physical)) {
                return Err(ProjectionError::LayoutChanged);
            }
            let native = &physical.unwrapped_layout;
            let first_run = indexed
                .run_starts
                .partition_point(|start| *start <= row.glyphs.start)
                .saturating_sub(1);
            let last_run = indexed
                .run_starts
                .partition_point(|start| *start < row.glyphs.end)
                .saturating_sub(1);
            let mut cells = Vec::new();
            if !row.glyphs.is_empty() {
                for run in first_run..=last_run {
                    let start = row
                        .glyphs
                        .start
                        .saturating_sub(indexed.run_starts[run])
                        .min(native.runs[run].glyphs.len());
                    let end = row
                        .glyphs
                        .end
                        .saturating_sub(indexed.run_starts[run])
                        .min(native.runs[run].glyphs.len());
                    bytes = bytes
                        .saturating_add((end - start) * std::mem::size_of::<ProjectedCluster>());
                    if bytes > MAX_PROJECTED_BYTES {
                        return Err(ProjectionError::WindowBudget);
                    }
                    cells.extend(
                        (start..end)
                            .map(|glyph| Self::glyph_cell(row, indexed, native, run, glyph)),
                    );
                }
            }
            let last_in_line = self
                .rows
                .get(ordinal + 1)
                .is_none_or(|next| next.physical_line != row.physical_line);
            result.push(VisualClusterRow {
                ordinal,
                clusters: cells,
                source_start: indexed.source_start,
                break_offset: (last_in_line && row.physical_line + 1 < self.physical.len())
                    .then_some(indexed.source_start + native.len),
                width: row.end_x - row.origin_x,
                break_rtl: indexed.base_rtl,
            });
        }
        Ok(result)
    }

    /// The worker receives the original full native snapshot transiently and
    /// copies only these rows. Glyph positions are translated, never reshaped.
    #[expect(
        clippy::too_many_lines,
        reason = "one sequential pass; splitting it would only thread its locals through helpers"
    )]
    pub fn project(
        &self,
        physical: &[Arc<WrappedLineLayout>],
        window: Range<usize>,
    ) -> Result<Vec<ProjectedNativeRow>, ProjectionError> {
        if window.start > window.end || window.end > self.rows.len() || window.len() > MAX_ROWS {
            return Err(ProjectionError::WindowBudget);
        }
        let mut projected = Vec::with_capacity(window.len());
        let mut bytes = projected.capacity() * std::mem::size_of::<ProjectedNativeRow>();
        for row in &self.rows[window] {
            let physical = physical
                .get(row.physical_line)
                .ok_or(ProjectionError::LayoutChanged)?;
            let indexed = &self.physical[row.physical_line];
            if !indexed.identity.ptr_eq(&Arc::downgrade(physical)) {
                return Err(ProjectionError::LayoutChanged);
            }
            let native = &physical.unwrapped_layout;
            let source_start = indexed.source_start;
            let run_starts = &indexed.run_starts;
            let mut runs = Vec::new();
            let mut ranges = Vec::new();
            let mut cells = Vec::<ProjectedCluster>::new();

            let first_run = run_starts
                .partition_point(|start| *start <= row.glyphs.start)
                .saturating_sub(1);
            let last_run = run_starts
                .partition_point(|start| *start < row.glyphs.end)
                .saturating_sub(1);
            let requested_runs = if row.glyphs.is_empty() {
                &native.runs[0..0]
            } else {
                native
                    .runs
                    .get(first_run..=last_run)
                    .ok_or(ProjectionError::LayoutChanged)?
            };
            for (relative_run, run) in requested_runs.iter().enumerate() {
                let glyph_cursor = run_starts[first_run + relative_run];
                let start = row
                    .glyphs
                    .start
                    .saturating_sub(glyph_cursor)
                    .min(run.glyphs.len());
                let end = row
                    .glyphs
                    .end
                    .saturating_sub(glyph_cursor)
                    .min(run.glyphs.len());
                if start >= end {
                    continue;
                }
                bytes = bytes.saturating_add(
                    (end - start)
                        * (std::mem::size_of::<ShapedGlyph>()
                            + std::mem::size_of::<ProjectedCluster>())
                        + std::mem::size_of::<ShapedRun>(),
                );
                if bytes > MAX_PROJECTED_BYTES {
                    return Err(ProjectionError::WindowBudget);
                }
                let glyphs = run.glyphs[start..end]
                    .iter()
                    .enumerate()
                    .map(|(local_glyph, glyph)| {
                        let mut glyph = glyph.clone();
                        let cell = Self::glyph_cell(
                            row,
                            indexed,
                            native,
                            first_run + relative_run,
                            start + local_glyph,
                        );
                        ranges.push(cell.source.clone());
                        cells.push(cell);
                        glyph.position.x -= row.origin_x;
                        glyph.index += source_start;
                        glyph
                    })
                    .collect();
                runs.push(ShapedRun {
                    font_id: run.font_id,
                    glyphs,
                });
            }
            ranges.sort_unstable_by_key(|range| range.start);
            let mut merged: Vec<Range<usize>> = Vec::new();
            for range in ranges {
                if let Some(previous) = merged
                    .last_mut()
                    .filter(|previous| previous.end >= range.start)
                {
                    previous.end = previous.end.max(range.end);
                } else {
                    merged.push(range);
                }
            }
            bytes = bytes.saturating_add(
                merged.capacity() * std::mem::size_of::<Range<usize>>()
                    + cells.capacity() * std::mem::size_of::<ProjectedCluster>()
                    + std::mem::size_of::<LineLayout>(),
            );
            if bytes > MAX_PROJECTED_BYTES {
                return Err(ProjectionError::WindowBudget);
            }
            projected.push(ProjectedNativeRow {
                ready_budget: None,
                layout: Arc::new(LineLayout {
                    font_size: native.font_size,
                    width: row.end_x - row.origin_x,
                    ascent: native.ascent,
                    descent: native.descent,
                    runs,
                    len: self.text_bytes,
                }),
                source: ProjectedSource { ranges: merged },
                physical_line: row.physical_line,
                source_start: indexed.source_start,
                clusters: cells,
            });
        }
        if projected.capacity() * std::mem::size_of::<ProjectedNativeRow>()
            + projected
                .iter()
                .map(ProjectedNativeRow::resident_bytes)
                .sum::<usize>()
            > MAX_PROJECTED_BYTES
        {
            return Err(ProjectionError::WindowBudget);
        }
        Ok(projected)
    }
}

#[cfg(test)]
mod tests;
