use super::list::{Lists, RowData};

pub const RAIL_WIDTH: f32 = 48.0;
pub const ROW_HEIGHT: f32 = 48.0;
pub const READ_AHEAD: f32 = 600.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadLayout {
    Grouped,
    Standalone,
}

impl ThreadLayout {
    pub const fn height(self) -> f32 {
        match self {
            Self::Grouped => 32.0,
            Self::Standalone => ROW_HEIGHT,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    Thread(RowData, ThreadLayout),
    Project {
        id: artisan_api::ProjectId,
        label: String,
    },
    History,
    Heading(&'static str),
    Gap(f32),
    Spacer,
}

impl Entry {
    pub fn height(&self) -> f32 {
        match self {
            Self::Thread(_, layout) => layout.height(),
            Self::Project { .. } => 28.0,
            Self::History => 43.0,
            Self::Heading(_) => 21.0,
            Self::Gap(height) => *height,
            Self::Spacer => 0.0,
        }
    }
}

pub fn entries(lists: &Lists, history_visible: bool) -> Vec<Entry> {
    let mut groups: Vec<(&artisan_api::ProjectId, Vec<&RowData>)> = Vec::new();
    for row in &lists.inbox {
        match groups
            .iter_mut()
            .find(|(project, _)| *project == &row.project)
        {
            Some((_, rows)) => rows.push(row),
            None => groups.push((&row.project, vec![row])),
        }
    }
    let mut entries = Vec::new();
    for (index, (project, rows)) in groups.into_iter().enumerate() {
        if index > 0 {
            entries.push(Entry::Gap(12.0));
        }
        entries.push(Entry::Project {
            id: project.clone(),
            label: rows[0].subtitle.clone(),
        });
        for (index, row) in rows.into_iter().enumerate() {
            if index > 0 {
                entries.push(Entry::Gap(2.0));
            }
            entries.push(Entry::Thread(row.clone(), ThreadLayout::Grouped));
        }
    }
    if !lists.history.is_empty() {
        entries.push(Entry::Spacer);
        entries.push(Entry::History);
        if history_visible {
            entries.push(Entry::Gap(2.0));
            for (index, group) in lists.history.iter().enumerate() {
                if index > 0 {
                    entries.push(Entry::Gap(12.0));
                }
                if let Some(label) = group.age.label() {
                    entries.push(Entry::Heading(label));
                }
                for (index, row) in group.rows.iter().enumerate() {
                    if index > 0 {
                        entries.push(Entry::Gap(2.0));
                    }
                    entries.push(Entry::Thread(row.clone(), ThreadLayout::Standalone));
                }
            }
        }
    }
    entries
}

pub fn history_spacer(entries: &[Entry], viewport_height: f32, fraction: f32) -> f32 {
    let mut after_history = false;
    let mut before = 0.0;
    let mut after = 0.0;
    for entry in entries {
        if after_history {
            after += entry.height();
        } else {
            before += entry.height();
        }
        after_history |= matches!(entry, Entry::History);
    }
    let closed = (viewport_height - before).max(12.0);
    let open = (viewport_height - before - after).max(12.0);
    (open - closed).mul_add(fraction, closed)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageDirection {
    Newer,
    Older,
}

pub struct PagePosition {
    pub viewport_height: f32,
    pub above: f32,
    pub below: f32,
    pub loading: bool,
    pub loaded: bool,
    pub failed: bool,
    pub scrolled: bool,
    pub at_latest: bool,
    pub has_older: bool,
    pub history_open: bool,
    pub history_empty: bool,
}

pub fn page_direction(page: &PagePosition) -> Option<PageDirection> {
    if page.viewport_height <= 0.0
        || page.loading
        || !page.loaded
        || (page.failed && !page.scrolled)
    {
        return None;
    }
    if !page.at_latest && page.above < READ_AHEAD {
        Some(PageDirection::Newer)
    } else if page.has_older && page.below < READ_AHEAD && (page.history_open || page.history_empty)
    {
        Some(PageDirection::Older)
    } else {
        None
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Splice {
    pub start: usize,
    pub removed: usize,
    pub inserted: usize,
}

pub fn entry_keys(entries: &[Entry]) -> Vec<String> {
    let mut keys = vec![String::new(); entries.len()];
    let mut below = String::from("end");
    for (index, entry) in entries.iter().enumerate().rev() {
        keys[index] = match entry {
            Entry::Thread(row, _) => format!("thread:{}", row.id),
            Entry::Project { id, .. } => format!("project:{id}"),
            Entry::History => "history".into(),
            Entry::Spacer => "spacer".into(),
            Entry::Heading(label) => format!("heading:{label}"),
            Entry::Gap(height) => format!("gap:{height}:{below}"),
        };
        below.clone_from(&keys[index]);
    }
    keys
}

pub fn splices(old: &[Entry], new: &[Entry]) -> Vec<Splice> {
    let (old, new) = (entry_keys(old), entry_keys(new));
    let old_set: std::collections::HashSet<_> = old.iter().collect();
    let new_set: std::collections::HashSet<_> = new.iter().collect();
    let (mut i, mut j) = (0, 0);
    let mut changes = Vec::new();
    while i < old.len() || j < new.len() {
        if i < old.len() && j < new.len() && old[i] == new[j] {
            i += 1;
            j += 1;
            continue;
        }
        let removed = old[i..]
            .iter()
            .take_while(|key| !new_set.contains(key))
            .count();
        if removed > 0 {
            changes.push(Splice {
                start: j,
                removed,
                inserted: 0,
            });
            i += removed;
            continue;
        }
        let inserted = new[j..]
            .iter()
            .take_while(|key| !old_set.contains(key))
            .count();
        if inserted > 0 {
            changes.push(Splice {
                start: j,
                removed: 0,
                inserted,
            });
            j += inserted;
            continue;
        }
        changes.push(Splice {
            start: j,
            removed: old.len() - i,
            inserted: new.len() - j,
        });
        break;
    }
    changes
}

pub fn history_fraction(
    open: bool,
    from: f32,
    elapsed_seconds: f64,
    reduced_motion: bool,
) -> (f32, bool) {
    let duration = if open { 0.4 } else { 0.35 };
    let progress = if reduced_motion {
        1.0
    } else {
        (elapsed_seconds / duration).min(1.0)
    };
    let target = if open { 1.0 } else { 0.0 };
    let fraction = (target - from).mul_add(
        crate::ui::motion::MotionCurve::SmoothOut.sample_f32(progress),
        from,
    );
    (fraction, progress >= 1.0)
}
