use artisan_api::{Project, ProjectId, RunStatus, Thread, ThreadId, Timestamp};

const DAY: i64 = 24 * 60 * 60 * 1000;

pub trait HostSnapshot {
    fn threads(&self) -> Vec<&Thread>;
    fn project(&self, id: &ProjectId) -> Option<&Project>;
    fn repository_url(&self, id: &ProjectId) -> Option<&str>;
    fn is_waiting(&self, id: &ThreadId) -> bool;
    fn is_working(&self, id: &ThreadId) -> bool;
    fn run_status(&self, id: &ThreadId) -> Option<RunStatus>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowState {
    Idle,
    Working,
    Waiting,
    Unread,
    Failed,
    Parked,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowData {
    pub id: ThreadId,
    pub title: String,
    pub subtitle: String,
    pub project: ProjectId,
    pub state: RowState,
    pub selected: bool,
    pub settleable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgeGroup {
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
    fn upper_bound(self) -> Option<i64> {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryGroup {
    pub age: AgeGroup,
    pub rows: Vec<RowData>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lists {
    pub inbox: Vec<RowData>,
    pub history: Vec<HistoryGroup>,
    pub next_tick: Option<Timestamp>,
}

impl Lists {
    #[must_use]
    pub fn searched(self, query: &str) -> Self {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        if words.is_empty() {
            return self;
        }
        let matches = |row: &RowData| {
            let text = format!("{} {}", row.title, row.subtitle).to_lowercase();
            words.iter().all(|word| text.contains(word.as_str()))
        };
        Self {
            inbox: self
                .inbox
                .into_iter()
                .chain(self.history.into_iter().flat_map(|group| group.rows))
                .filter(matches)
                .collect(),
            history: Vec::new(),
            next_tick: self.next_tick,
        }
    }
}

pub const fn listed(thread: &Thread) -> bool {
    !thread.archived && thread.parent_thread_id.is_none()
}

pub fn row_state(host: &impl HostSnapshot, thread: &Thread) -> RowState {
    if host.is_waiting(&thread.id) {
        RowState::Waiting
    } else if host.is_working(&thread.id) {
        RowState::Working
    } else if thread.parked_until.is_some() {
        RowState::Parked
    } else if thread.is_unread() {
        match host.run_status(&thread.id) {
            Some(RunStatus::Failed | RunStatus::Interrupted | RunStatus::Refused) => {
                RowState::Failed
            }
            _ => RowState::Unread,
        }
    } else {
        RowState::Idle
    }
}

pub fn lists(host: &impl HostSnapshot, open: Option<&ThreadId>, now: Timestamp) -> Lists {
    let mut result = Lists::default();
    let mut history: [Vec<RowData>; 4] = Default::default();
    let mut threads = host.threads();
    threads.sort_by(|a, b| {
        b.activity_seq
            .cmp(&a.activity_seq)
            .then_with(|| b.id.0.cmp(&a.id.0))
    });
    for thread in threads.into_iter().filter(|thread| listed(thread)) {
        let Some(project) = host.project(&thread.project_id) else {
            continue;
        };
        let state = row_state(host, thread);
        let elapsed = now.saturating_sub(thread.activity_at);
        let settled = thread.settled_seq > 0 && thread.settled_seq >= thread.activity_seq;
        let live = matches!(
            state,
            RowState::Working | RowState::Waiting | RowState::Parked
        );
        let inbox = live || (!settled && (state != RowState::Idle || elapsed < DAY));
        let row = RowData {
            id: thread.id.clone(),
            title: thread
                .title
                .clone()
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| "New thread".to_owned()),
            subtitle: host
                .repository_url(&project.id)
                .map_or_else(|| project.name.clone(), qualified_label),
            project: project.id.clone(),
            state,
            selected: open == Some(&thread.id),
            settleable: inbox && !live,
        };
        let group = AgeGroup::of(elapsed);
        if let Some(bound) = group.upper_bound() {
            let at = thread.activity_at.saturating_add(bound);
            if at > now {
                result.next_tick = Some(result.next_tick.map_or(at, |next| next.min(at)));
            }
        }
        if inbox {
            result.inbox.push(row);
        } else {
            history[group as usize].push(row);
        }
    }
    result.history = AgeGroup::ALL
        .into_iter()
        .zip(history)
        .filter(|(_, rows)| !rows.is_empty())
        .map(|(age, rows)| HistoryGroup { age, rows })
        .collect();
    result
}

pub fn qualified_label(web_url: &str) -> String {
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

impl HostSnapshot for editor_core::HostData {
    fn threads(&self) -> Vec<&Thread> {
        self.threads.values().collect()
    }

    fn project(&self, id: &ProjectId) -> Option<&Project> {
        self.projects.get(id)
    }

    fn repository_url(&self, id: &ProjectId) -> Option<&str> {
        self.repository(id)
            .and_then(|repository| repository.web_url.as_deref())
    }

    fn is_waiting(&self, id: &ThreadId) -> bool {
        self.is_waiting(id)
    }

    fn is_working(&self, id: &ThreadId) -> bool {
        self.is_working(id)
    }

    fn run_status(&self, id: &ThreadId) -> Option<RunStatus> {
        self.runs.get(id).map(|run| run.status)
    }
}
