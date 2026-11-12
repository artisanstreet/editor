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
    pub matched: Vec<std::ops::Range<usize>>,
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
    pub hits: Vec<HitRow>,
    pub next_tick: Option<Timestamp>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HitRow {
    pub thread: ThreadId,
    pub item: artisan_api::ItemId,
    pub position: artisan_api::Seq,
    pub title: String,
    pub snippet: String,
    pub matched: std::ops::Range<usize>,
    pub needle: String,
    pub depth: (u32, u32),
}

impl HitRow {
    #[must_use]
    pub fn of(hit: &artisan_api::TextHit) -> Option<Self> {
        let matched = hit.matched.0 as usize..hit.matched.1 as usize;
        let needle = hit.snippet.get(matched.clone())?.to_owned();
        Some(Self {
            thread: hit.thread_id.clone(),
            item: hit.item_id.clone(),
            position: hit.position,
            title: hit
                .thread_title
                .clone()
                .filter(|title| !title.trim().is_empty())
                .unwrap_or_else(|| "New thread".to_owned()),
            snippet: hit.snippet.clone(),
            matched,
            needle,
            depth: (hit.offset.0, hit.length),
        })
    }
}

fn marked(title: &str, mut bytes: Vec<usize>) -> Vec<std::ops::Range<usize>> {
    bytes.sort_unstable();
    bytes.dedup();
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    for byte in bytes {
        let start = (0..=byte)
            .rev()
            .find(|at| title.is_char_boundary(*at))
            .unwrap_or(0);
        let end = (byte + 1..=title.len())
            .find(|at| title.is_char_boundary(*at))
            .unwrap_or(title.len());
        match ranges.last_mut() {
            Some(last) if start <= last.end => last.end = last.end.max(end),
            _ => ranges.push(start..end),
        }
    }
    ranges
}

impl Lists {
    #[must_use]
    pub fn searched(self, query: &str) -> Self {
        use neo_frizbee::{Config, match_list_indices};
        let words: Vec<&str> = query.split_whitespace().collect();
        if words.is_empty() {
            return self;
        }
        let rows: Vec<RowData> = self
            .inbox
            .into_iter()
            .chain(self.history.into_iter().flat_map(|group| group.rows))
            .collect();
        let titles: Vec<&str> = rows.iter().map(|row| row.title.as_str()).collect();
        let labels: Vec<&str> = rows.iter().map(|row| row.subtitle.as_str()).collect();
        let config = Config {
            max_typos: Some(0),
            sort: false,
            ..Config::default()
        };
        let mut hits = vec![0_usize; rows.len()];
        let mut scores = vec![0_u32; rows.len()];
        let mut bytes: Vec<Vec<usize>> = vec![Vec::new(); rows.len()];
        for word in &words {
            let mut found = vec![false; rows.len()];
            for hit in match_list_indices(word, &titles, &config) {
                let row = hit.index as usize;
                found[row] = true;
                scores[row] += u32::from(hit.score);
                bytes[row].extend(hit.indices);
            }
            for hit in match_list_indices(word, &labels, &config) {
                let row = hit.index as usize;
                if !std::mem::replace(&mut found[row], true) {
                    scores[row] += u32::from(hit.score) / 2;
                }
            }
            for (row, found) in found.into_iter().enumerate() {
                hits[row] += usize::from(found);
            }
        }
        let mut ranked: Vec<(u32, RowData)> = rows
            .into_iter()
            .zip(hits)
            .zip(scores)
            .zip(bytes)
            .filter(|(((_, hits), _), _)| *hits == words.len())
            .map(|(((row, _), score), bytes)| {
                let matched = marked(&row.title, bytes);
                (score, RowData { matched, ..row })
            })
            .collect();
        ranked.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        Self {
            inbox: ranked.into_iter().map(|(_, row)| row).collect(),
            history: Vec::new(),
            hits: self.hits,
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
            matched: Vec::new(),
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
