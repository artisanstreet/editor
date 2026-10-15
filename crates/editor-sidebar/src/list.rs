//! What the sidebar lists, as a pure function of what Forge sent and the
//! clock.
//!
//! The sidebar is an inbox: a thread stays at the top until the user
//! settles it (from its row's menu) or a day passes without activity; then it moves into `History`, which keeps age groups inside.
//! Reading a thread never moves it. Live work, an open question or an
//! unseen outcome brings a settled thread back to the top.

use artisan_api::{ProjectId, RunStatus, ThreadId, Timestamp};
use editor_core::HostData;
use gpui::SharedString;

const HOUR: i64 = 60 * 60 * 1000;
const DAY: i64 = 24 * HOUR;

/// What a row's trailing dot says. At most one shows; waiting on the user
/// outranks the work it blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RowState {
    Idle,
    Working,
    Waiting,
    /// The latest run finished and the user hasn't seen it.
    Unread,
    /// Unread, and the latest run failed.
    Failed,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RowData {
    pub id: ThreadId,
    pub title: SharedString,
    /// The repository's `owner/name`, else the project's name.
    pub subtitle: SharedString,
    pub project: ProjectId,
    pub state: RowState,
    /// The open thread: drawn with a link-coloured title.
    pub selected: bool,
    /// Whether the user can settle it into History.
    pub settleable: bool,
}

/// How long ago a History thread was last active. The youngest
/// group has no heading.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AgeGroup {
    LastDay,
    LastThreeDays,
    LastMonth,
    Older,
}

impl AgeGroup {
    const ALL: [Self; 4] = [
        Self::LastDay,
        Self::LastThreeDays,
        Self::LastMonth,
        Self::Older,
    ];

    /// The age at which a row leaves this group, if it ever does.
    const fn upper_bound(self) -> Option<i64> {
        match self {
            Self::LastDay => Some(DAY),
            Self::LastThreeDays => Some(3 * DAY),
            Self::LastMonth => Some(30 * DAY),
            Self::Older => None,
        }
    }

    fn of(elapsed: i64) -> Self {
        Self::ALL
            .into_iter()
            .find(|group| group.upper_bound().is_none_or(|bound| elapsed < bound))
            .unwrap_or(Self::Older)
    }

    pub const fn label(self) -> Option<&'static str> {
        match self {
            Self::LastDay => None,
            Self::LastThreeDays => Some("Last 3 days"),
            Self::LastMonth => Some("Last 30 days"),
            Self::Older => Some("Older"),
        }
    }
}

/// One non-empty age group of History.
pub(crate) struct HistoryGroup {
    pub age: AgeGroup,
    /// Newest turn first.
    pub rows: Vec<RowData>,
}

#[derive(Default)]
pub(crate) struct Lists {
    /// Threads not yet settled, newest turn first.
    pub inbox: Vec<RowData>,
    /// Settled threads in non-empty age groups, newest first.
    pub history: Vec<HistoryGroup>,
    /// When the next row changes group.
    pub next_tick: Option<Timestamp>,
}

impl Lists {
    pub fn history_len(&self) -> usize {
        self.history.iter().map(|group| group.rows.len()).sum()
    }
}

/// Match Forge's summary cursor ordering. Read acknowledgements do not change activity_seq.
fn by_latest_turn(host: &HostData) -> Vec<&artisan_api::Thread> {
    let mut threads: Vec<_> = host.threads.values().collect();
    threads.sort_by(|a, b| {
        b.activity_seq
            .cmp(&a.activity_seq)
            .then_with(|| b.id.0.cmp(&a.id.0))
    });
    threads
}

/// Project and thread metadata determine rows; only visible rows request icons.
pub(crate) fn lists(host: &HostData, open: Option<&ThreadId>, now: Timestamp) -> Lists {
    let mut lists = Lists::default();
    let mut history: [Vec<RowData>; AgeGroup::ALL.len()] = Default::default();
    let mut tick = |at: Timestamp| {
        if at > now {
            lists.next_tick = Some(lists.next_tick.map_or(at, |next| next.min(at)));
        }
    };
    let mut inbox = Vec::new();
    for thread in by_latest_turn(host) {
        // Subagent and handed-off threads live inside their parent.
        if thread.archived || thread.parent_thread_id.is_some() {
            continue;
        }
        let Some(project) = host.projects.get(&thread.project_id) else {
            continue;
        };
        let selected = open == Some(&thread.id);
        let run = host.runs.get(&thread.id);
        let state = if host.is_waiting(&thread.id) {
            RowState::Waiting
        } else if host.is_working(&thread.id) {
            RowState::Working
        } else if thread.is_unread() {
            match run.map(|run| run.status) {
                Some(RunStatus::Failed | RunStatus::Interrupted | RunStatus::Refused) => {
                    RowState::Failed
                }
                _ => RowState::Unread,
            }
        } else {
            RowState::Idle
        };
        let active_at = thread.activity_at;
        let elapsed = now.saturating_sub(active_at);
        let repository = host.repository(&project.id);
        let subtitle = repository
            .and_then(|repository| repository.web_url.as_deref())
            .map_or_else(|| project.name.clone(), qualified_label);
        let is_settled = thread.settled_seq > 0 && thread.settled_seq >= thread.activity_seq;
        let quiet = elapsed >= DAY;
        // Settling files the current outcome without claiming that its content was read.
        let in_inbox = matches!(state, RowState::Working | RowState::Waiting)
            || (!is_settled && (state != RowState::Idle || !quiet));
        let row = RowData {
            id: thread.id.clone(),
            title: thread
                .title
                .clone()
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| "New thread".to_owned())
                .into(),
            subtitle: subtitle.into(),
            project: project.id.clone(),
            state,
            selected,
            settleable: in_inbox && state != RowState::Working && state != RowState::Waiting,
        };
        let group = AgeGroup::of(elapsed);
        if let Some(bound) = group.upper_bound() {
            tick(active_at.saturating_add(bound));
        }
        if in_inbox {
            inbox.push(row);
        } else {
            history[group as usize].push(row);
        }
    }
    lists.inbox = inbox;
    lists.history = AgeGroup::ALL
        .into_iter()
        .zip(history)
        .filter(|(_, rows)| !rows.is_empty())
        .map(|(age, rows)| HistoryGroup { age, rows })
        .collect();
    lists
}

/// The innermost `owner/name` of a repository's web page.
pub(crate) fn qualified_label(web_url: &str) -> String {
    let path = web_url
        .split_once("://")
        .map_or(web_url, |(_, rest)| rest)
        .trim_end_matches('/');
    let segments: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    match segments.as_slice() {
        [] => web_url.to_owned(),
        [host] => (*host).to_owned(),
        [_, name] => (*name).to_owned(),
        [.., owner, name] => format!("{owner}/{name}"),
    }
}
