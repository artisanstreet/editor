use super::list::{HitRow, Lists, RowData};
use editor_ui::kit::{Density, Divider, Row, Space, Surface};
use editor_ui::theme::ArtisanTheme;

pub(crate) const RAIL_WIDTH: f32 = 48.0;
pub(crate) const READ_AHEAD: f32 = 600.0;
pub(crate) const LIST: Surface = Surface::Inset;
pub(crate) const HEADING: Row = Row::ListHeading;
pub const TOGGLE: Row = Row::ListItem(Density::Regular);
const GROUP_GAP: Space = Space::S12;
const ROW_GAP: Space = Space::S2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadLayout {
    Grouped,
    Standalone,
}

impl ThreadLayout {
    pub(crate) const fn row(self) -> Row {
        match self {
            Self::Grouped => Row::ListItem(Density::Regular),
            Self::Standalone => Row::ListItem(Density::Tall),
        }
    }

    pub(crate) fn height(self, theme: &ArtisanTheme) -> f32 {
        self.row().height(theme)
    }
}

pub fn history_height(theme: &ArtisanTheme) -> f32 {
    Divider::Rule.height(theme) + TOGGLE.height(theme)
}

#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    Thread(RowData, ThreadLayout),
    Hit(HitRow),
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
    pub fn height(&self, theme: &ArtisanTheme) -> f32 {
        match self {
            Self::Thread(_, layout) => layout.height(theme),
            Self::Hit(_) => ThreadLayout::Standalone.height(theme),
            Self::Project { .. } | Self::Heading(_) => HEADING.height(theme),
            Self::History => history_height(theme),
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
            entries.push(Entry::Gap(GROUP_GAP.px()));
        }
        entries.push(Entry::Project {
            id: project.clone(),
            label: rows[0].subtitle.clone(),
        });
        for (index, row) in rows.into_iter().enumerate() {
            if index > 0 {
                entries.push(Entry::Gap(ROW_GAP.px()));
            }
            entries.push(Entry::Thread(row.clone(), ThreadLayout::Grouped));
        }
    }
    if !lists.hits.is_empty() {
        if !entries.is_empty() {
            entries.push(Entry::Gap(GROUP_GAP.px()));
        }
        entries.push(Entry::Heading("In messages"));
        for (index, hit) in lists.hits.iter().enumerate() {
            if index > 0 {
                entries.push(Entry::Gap(ROW_GAP.px()));
            }
            entries.push(Entry::Hit(hit.clone()));
        }
    }
    if !lists.history.is_empty() {
        entries.push(Entry::Spacer);
        entries.push(Entry::History);
        if history_visible {
            entries.push(Entry::Gap(ROW_GAP.px()));
            for (index, group) in lists.history.iter().enumerate() {
                if index > 0 {
                    entries.push(Entry::Gap(GROUP_GAP.px()));
                }
                if let Some(label) = group.age.label() {
                    entries.push(Entry::Heading(label));
                }
                for (index, row) in group.rows.iter().enumerate() {
                    if index > 0 {
                        entries.push(Entry::Gap(ROW_GAP.px()));
                    }
                    entries.push(Entry::Thread(row.clone(), ThreadLayout::Standalone));
                }
            }
        }
    }
    entries
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Listed {
    pub entry: usize,
    pub gap: f32,
    pub spaced: bool,
}

impl Listed {
    pub fn lead(self, spacer: f32) -> f32 {
        if self.spaced {
            self.gap + spacer
        } else {
            self.gap
        }
    }
}

pub fn listed(entries: &[Entry]) -> Vec<Listed> {
    let mut lead = Listed::default();
    entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| match entry {
            Entry::Gap(height) => {
                lead.gap += height;
                None
            }
            Entry::Spacer => {
                lead.spaced = true;
                None
            }
            Entry::Thread(..)
            | Entry::Hit(_)
            | Entry::Project { .. }
            | Entry::History
            | Entry::Heading(_) => Some(Listed {
                entry: index,
                ..std::mem::take(&mut lead)
            }),
        })
        .collect()
}

pub fn history_spacer(
    theme: &ArtisanTheme,
    entries: &[Entry],
    viewport_height: f32,
    fraction: f32,
) -> f32 {
    let mut after_history = false;
    let mut before = 0.0;
    let mut after = 0.0;
    for entry in entries {
        if after_history {
            after += entry.height(theme);
        } else {
            before += entry.height(theme);
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageLoad {
    Pending,
    Ready,
    Failed { scrolled: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HistoryPane {
    Empty,
    Open,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageState {
    pub load: PageLoad,
    pub at_latest: bool,
    pub has_older: bool,
    pub history: HistoryPane,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThreadPane {
    Inbox,
    History,
}

impl ThreadPane {
    pub(crate) const fn scroller(self) -> &'static str {
        match self {
            Self::Inbox => "sidebar-threads",
            Self::History => "sidebar-history-threads",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneDistances {
    pub viewport_height: f32,
    pub above: f32,
    pub below: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PaneViewports {
    inbox: Option<PaneDistances>,
    history: Option<PaneDistances>,
}

impl PaneViewports {
    const fn slot(&mut self, pane: ThreadPane) -> &mut Option<PaneDistances> {
        match pane {
            ThreadPane::Inbox => &mut self.inbox,
            ThreadPane::History => &mut self.history,
        }
    }

    pub const fn report(&mut self, pane: ThreadPane, distances: PaneDistances) {
        *self.slot(pane) = Some(distances);
    }

    pub const fn forget(&mut self, pane: ThreadPane) {
        *self.slot(pane) = None;
    }

    pub fn direction(&self, state: PageState) -> Option<PageDirection> {
        self.inbox
            .into_iter()
            .chain(self.history)
            .find_map(|distances| page_direction(distances, state))
    }
}

pub fn page_direction(distances: PaneDistances, state: PageState) -> Option<PageDirection> {
    let settled = match state.load {
        PageLoad::Ready | PageLoad::Failed { scrolled: true } => true,
        PageLoad::Pending | PageLoad::Failed { scrolled: false } => false,
    };
    let older_shown = match state.history {
        HistoryPane::Empty | HistoryPane::Open => true,
        HistoryPane::Closed => false,
    };
    if distances.viewport_height <= 0.0 || !settled {
        None
    } else if !state.at_latest && distances.above < READ_AHEAD {
        Some(PageDirection::Newer)
    } else if state.has_older && distances.below < READ_AHEAD && older_shown {
        Some(PageDirection::Older)
    } else {
        None
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Splice {
    start: usize,
    removed: usize,
    inserted: usize,
}

pub fn entry_keys(entries: &[Entry]) -> Vec<String> {
    let mut keys = vec![String::new(); entries.len()];
    let mut below = String::from("end");
    for (index, entry) in entries.iter().enumerate().rev() {
        keys[index] = match entry {
            Entry::Thread(row, _) => format!("thread:{}", row.id),
            Entry::Hit(hit) => format!("hit:{}", hit.item),
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

pub(crate) fn history_fraction(
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
        editor_ui::motion::MotionCurve::SmoothOut.sample_f32(progress),
        from,
    );
    (fraction, progress >= 1.0)
}
