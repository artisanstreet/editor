use artisan_api::EngineId;
use std::time::{Duration, Instant};

use super::picker::ModelRow;
use editor_ui::kit::{Row, Space};

const fn height(row: Row) -> f32 {
    match row.height() {
        Some(height) => height,
        None => 0.0,
    }
}

pub const ENGINE_ROW_HEIGHT: f32 = height(Row::Heading);
pub const GATE_ROW_HEIGHT: f32 = 84.0;
pub const GROUP_ROW_HEIGHT: f32 = height(Row::Heading);
pub const MODEL_ROW_HEIGHT: f32 = height(Row::MenuItem);
pub const ROW_GAP: Space = Space::S2;
pub const SECTION_SPACER: f32 = Space::S6.px();
pub const PADDING: Space = Space::S6;
const GAP: f32 = ROW_GAP.px();
const PADDING_TOP: f32 = PADDING.px();
const PADDING_BOTTOM: f32 = PADDING.px();
const SPY_LINE: f32 = 28.0;
const JUMP_DURATION: Duration = Duration::from_millis(300);

#[derive(Clone, Debug, PartialEq)]
pub struct ListLayout {
    tops: Vec<f32>,
    height: f32,
}

const fn row_height(row: &ModelRow) -> f32 {
    match row {
        ModelRow::Engine(_) => ENGINE_ROW_HEIGHT,
        ModelRow::Gate(_) => GATE_ROW_HEIGHT,
        ModelRow::Group { .. } => GROUP_ROW_HEIGHT,
        ModelRow::Model { .. } => MODEL_ROW_HEIGHT,
    }
}

impl ListLayout {
    #[must_use]
    pub fn new(rows: &[ModelRow]) -> Self {
        let mut tops = Vec::with_capacity(rows.len());
        let mut y = PADDING_TOP;
        let mut section = None;
        for row in rows {
            let engine = row.engine();
            if section.is_some_and(|previous| previous != engine) {
                y += SECTION_SPACER + GAP;
            }
            section = Some(engine);
            tops.push(y);
            y += row_height(row) + GAP;
        }
        let content = if rows.is_empty() { y } else { y - GAP };
        Self {
            tops,
            height: content + PADDING_BOTTOM,
        }
    }

    #[must_use]
    pub fn max_scroll(&self, viewport: f32) -> f32 {
        (self.height - viewport).max(0.0)
    }

    #[must_use]
    pub fn anchor(&self, rows: &[ModelRow], engine: EngineId, viewport: f32) -> Option<f32> {
        let index = rows.iter().position(|row| row.engine() == engine)?;
        Some((self.tops[index] - PADDING_TOP).clamp(0.0, self.max_scroll(viewport)))
    }

    #[must_use]
    pub fn spied(&self, rows: &[ModelRow], scroll: f32, viewport: f32) -> Option<EngineId> {
        if !rows.is_empty() && scroll >= self.max_scroll(viewport) - 1.0 && scroll > 0.0 {
            return rows.last().map(ModelRow::engine);
        }
        rows.iter()
            .zip(&self.tops)
            .take_while(|(_, top)| **top - scroll <= SPY_LINE)
            .last()
            .or_else(|| rows.iter().zip(&self.tops).next())
            .map(|(row, _)| row.engine())
    }

    #[must_use]
    pub fn centred(
        &self,
        rows: &[ModelRow],
        engine: EngineId,
        id: &str,
        viewport: f32,
    ) -> Option<f32> {
        let index = rows.iter().position(|row| {
            matches!(row, ModelRow::Model { engine: row_engine, id: row_id, .. }
                if *row_engine == engine && row_id == id)
        })?;
        let centre = self.tops[index] + MODEL_ROW_HEIGHT / 2.0 - viewport / 2.0;
        Some(centre.clamp(0.0, self.max_scroll(viewport)))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Jump {
    engine: EngineId,
    to: f32,
    from: Option<(f32, Instant)>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ModelScroll {
    jump: Option<Jump>,
    written: Option<f32>,
    saved: f32,
    hidden: bool,
}

impl ModelScroll {
    pub fn open(&mut self, offset: f32) {
        *self = Self {
            jump: None,
            written: Some(offset),
            saved: offset,
            hidden: false,
        };
    }

    pub fn jump(&mut self, engine: EngineId, to: f32) {
        self.jump = Some(Jump {
            engine,
            to,
            from: None,
        });
        self.written = None;
    }

    pub fn hide(&mut self) {
        self.jump = None;
        self.written = None;
        self.hidden = true;
    }

    #[must_use]
    pub const fn jumping(&self) -> bool {
        self.jump.is_some()
    }

    #[must_use]
    pub fn target(&self) -> Option<EngineId> {
        self.jump.map(|jump| jump.engine)
    }

    pub fn offset(&mut self, kept: f32, max: f32, now: Instant, reduced: bool) -> f32 {
        let kept = if self.hidden {
            self.hidden = false;
            self.saved
        } else {
            kept
        };
        let moved_by_reader = self.written.is_some_and(|written| (written - kept).abs() > 1.5);
        if moved_by_reader {
            self.jump = None;
        }
        let value = match &mut self.jump {
            None => kept,
            Some(jump) if reduced => jump.to,
            Some(jump) => {
                let (from, started) = *jump.from.get_or_insert((kept, now));
                let progress = (now.saturating_duration_since(started).as_secs_f32()
                    / JUMP_DURATION.as_secs_f32())
                .clamp(0.0, 1.0);
                (jump.to - from).mul_add(editor_ui::motion::smooth_out(progress), from)
            }
        }
        .clamp(0.0, max);
        if self.jump.is_some_and(|jump| (value - jump.to.min(max)).abs() < 0.25) {
            self.jump = None;
        }
        self.written = Some(value);
        self.saved = value;
        value
    }
}
