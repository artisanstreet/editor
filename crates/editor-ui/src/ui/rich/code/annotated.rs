pub(crate) mod view;

use super::super::kit::LineSpan;
use super::{code_lines, excerpt_width};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
struct NoteSpec {
    lines: LineSpan,
    text: String,
}

const fn first_line() -> u32 {
    1
}

#[derive(Debug, Deserialize)]
struct Spec {
    code: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default = "first_line")]
    start_line: u32,
    #[serde(default)]
    notes: Vec<NoteSpec>,
}

#[derive(Debug)]
struct Note {
    span: LineSpan,
    text: String,
}

#[derive(Debug)]
struct Model {
    lines: Vec<String>,
    width: f32,
    start: u32,
    path: Option<String>,
    notes: Vec<Note>,
    owner: Vec<Option<usize>>,
    marker: Vec<Option<usize>>,
}

impl Model {
    fn number(&self, index: usize) -> u32 {
        self.start
            .saturating_add(u32::try_from(index).unwrap_or(u32::MAX))
    }
}

#[derive(Debug, Deserialize)]
#[serde(from = "Spec")]
pub(crate) struct AnnotatedCode(Arc<Model>);
impl AnnotatedCode {
    pub(crate) fn resident_bytes(&self) -> usize {
        use crate::ui::rich::kit::{retained_optional, retained_strings, retained_vec};
        std::mem::size_of::<Self>()
            + std::mem::size_of::<Model>()
            + retained_strings(&self.0.lines)
            + retained_optional(self.0.path.as_ref())
            + retained_vec(&self.0.notes)
            + self
                .0
                .notes
                .iter()
                .map(|note| note.text.capacity())
                .sum::<usize>()
            + retained_vec(&self.0.owner)
            + retained_vec(&self.0.marker)
    }
}

impl From<Spec> for AnnotatedCode {
    fn from(spec: Spec) -> Self {
        const fn find(next: &mut [usize], mut index: usize) -> usize {
            let origin = index;
            while next[index] != index {
                index = next[index];
            }
            let root = index;
            index = origin;
            while next[index] != index {
                let parent = next[index];
                next[index] = root;
                index = parent;
            }
            root
        }
        let lines = code_lines(&spec.code);
        let width = excerpt_width(&lines);
        let start = spec.start_line.max(1);
        let count = u32::try_from(lines.len()).unwrap_or(u32::MAX);
        let last = start.saturating_add(count.saturating_sub(1));
        let notes: Vec<Note> = spec
            .notes
            .into_iter()
            .filter(|note| !note.text.trim().is_empty())
            .map(|note| {
                let mut span = note.lines;
                let outside = span.end < start || span.start > last;
                if outside && start > 1 && span.start >= 1 && span.end <= count {
                    span = LineSpan {
                        start: span.start + start - 1,
                        end: span.end + start - 1,
                    };
                }
                Note {
                    span,
                    text: String::from(note.text.trim().to_owned()),
                }
            })
            .collect();
        let mut owner = vec![None; lines.len()];
        let mut marker = vec![None; lines.len()];
        let mut next_free: Vec<usize> = (0..=lines.len()).collect();
        for (index, note) in notes.iter().enumerate() {
            if note.span.end >= start {
                let first = usize::try_from(note.span.start.saturating_sub(start))
                    .unwrap_or(usize::MAX)
                    .min(lines.len());
                let end = usize::try_from(note.span.end.saturating_sub(start))
                    .unwrap_or(usize::MAX)
                    .saturating_add(1)
                    .min(lines.len());
                let mut at = find(&mut next_free, first);
                while at < end {
                    owner[at] = Some(index);
                    next_free[at] = find(&mut next_free, at + 1);
                    at = next_free[at];
                }
            }
            let first = note.span.start.max(start);
            let at = usize::try_from(first - start).unwrap_or(usize::MAX);
            if note.span.end >= start
                && let Some(slot) = marker.get_mut(at)
                && slot.is_none()
            {
                *slot = Some(index);
            }
        }
        Self(Arc::new(Model {
            lines,
            width,
            start,
            path: spec
                .path
                .map(|path| path.trim().to_owned())
                .filter(|path| !path.is_empty())
                .map(String::from),
            notes,
            owner,
            marker,
        }))
    }
}
