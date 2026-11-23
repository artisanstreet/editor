use artisan_api::EngineId;
use std::time::{Duration, Instant};

use super::picker::ModelRow;
use editor_ui::kit::{Row, Space, TextRole};
use editor_ui::theme::ArtisanTheme;

pub(crate) const ROW_GAP: Space = Space::S2;
pub(crate) const SECTION_SPACER: f32 = Space::S6.px();
pub(crate) const PADDING: Space = Space::S6;
const GAP: f32 = ROW_GAP.px();
const PADDING_TOP: f32 = PADDING.px();
const PADDING_BOTTOM: f32 = PADDING.px();
const JUMP_DURATION: Duration = Duration::from_millis(300);

#[derive(Clone, Debug, PartialEq)]
pub struct ListLayout {
    tops: Vec<f32>,
    height: f32,
    model: f32,
    spy_line: f32,
}

#[must_use]
pub(crate) fn group_row_height(theme: &ArtisanTheme) -> f32 {
    Row::Heading.height(theme)
}

pub(crate) const GATE_PADDING: Space = Space::S12;
pub(crate) const GATE_GAP: Space = Space::S4;
const GATE_REASON_LINES: f32 = 2.0;

#[must_use]
pub(crate) fn gate_row_height(theme: &ArtisanTheme) -> f32 {
    let line = TextRole::Label.line_px(theme);
    2.0f32.mul_add(
        GATE_PADDING.px(),
        GATE_REASON_LINES.mul_add(line, GATE_GAP.px() + line),
    )
}

pub(crate) fn model_row_height(theme: &ArtisanTheme) -> f32 {
    Row::MenuItem.height(theme)
}

fn row_height(theme: &ArtisanTheme, row: &ModelRow) -> f32 {
    match row {
        ModelRow::Engine(_) | ModelRow::Group { .. } => group_row_height(theme),
        ModelRow::Gate(_) => gate_row_height(theme),
        ModelRow::Gap(_) | ModelRow::Model { .. } => model_row_height(theme),
    }
}

impl ListLayout {
    #[must_use]
    pub fn new(theme: &ArtisanTheme, rows: &[ModelRow]) -> Self {
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
            y += row_height(theme, row) + GAP;
        }
        let content = if rows.is_empty() { y } else { y - GAP };
        Self {
            tops,
            height: content + PADDING_BOTTOM,
            model: model_row_height(theme),
            spy_line: group_row_height(theme),
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
            .take_while(|(_, top)| **top - scroll <= self.spy_line)
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
        let centre = self.tops[index] + self.model / 2.0 - viewport / 2.0;
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
enum Aim {
    #[default]
    Free,
    Jumping(Jump),
    Landed(EngineId),
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ModelScroll {
    aim: Aim,
    written: Option<f32>,
    saved: f32,
    hidden: bool,
}

impl ModelScroll {
    pub fn open(&mut self, offset: f32) {
        *self = Self {
            aim: Aim::Free,
            written: Some(offset),
            saved: offset,
            hidden: false,
        };
    }

    pub fn jump(&mut self, engine: EngineId, to: f32) {
        self.aim = Aim::Jumping(Jump {
            engine,
            to,
            from: None,
        });
        self.written = None;
    }

    pub fn hide(&mut self) {
        self.aim = Aim::Free;
        self.written = None;
        self.hidden = true;
    }

    #[must_use]
    pub const fn jumping(&self) -> bool {
        matches!(self.aim, Aim::Jumping(_))
    }

    #[must_use]
    pub(crate) const fn target(&self) -> Option<EngineId> {
        match self.aim {
            Aim::Free => None,
            Aim::Jumping(Jump { engine, .. }) | Aim::Landed(engine) => Some(engine),
        }
    }

    pub fn offset(&mut self, kept: f32, max: f32, now: Instant, reduced: bool) -> f32 {
        let kept = if self.hidden {
            self.hidden = false;
            self.saved
        } else {
            kept
        };
        let moved_by_reader = self
            .written
            .is_some_and(|written| (written - kept).abs() > 1.5);
        if moved_by_reader {
            self.aim = Aim::Free;
        }
        let value = match &mut self.aim {
            Aim::Free | Aim::Landed(_) => kept,
            Aim::Jumping(jump) if reduced => jump.to,
            Aim::Jumping(jump) => {
                let (from, started) = *jump.from.get_or_insert((kept, now));
                let progress = (now.saturating_duration_since(started).as_secs_f32()
                    / JUMP_DURATION.as_secs_f32())
                .clamp(0.0, 1.0);
                (jump.to - from).mul_add(editor_ui::motion::smooth_out(progress), from)
            }
        }
        .clamp(0.0, max);
        if let Aim::Jumping(jump) = self.aim
            && (value - jump.to.min(max)).abs() < 0.25
        {
            self.aim = Aim::Landed(jump.engine);
        }
        self.written = Some(value);
        self.saved = value;
        value
    }
}

#[cfg(test)]
#[path = "../../tests/composer/model_list.rs"]
mod tests;
