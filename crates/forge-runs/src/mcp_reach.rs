use crate::mcp_auth::ReadMcpServersError;
use artisan_api::{Event, McpServer, ThreadId};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{Notify, watch};
use tokio::time::Instant;

pub(crate) const PATIENCE: Duration = Duration::from_secs(3);
pub(crate) const MARGIN: Duration = Duration::from_secs(1);
pub(crate) const FRESH: Duration = Duration::from_secs(60);
pub(crate) const REFRESH: Duration = Duration::from_secs(10);

pub(crate) type Endpoint = (String, u16);

type Published = watch::Receiver<Option<Instant>>;

fn endpoint(server: &McpServer) -> Option<Endpoint> {
    if server.disabled {
        return None;
    }
    let url = url::Url::parse(server.url.as_deref()?).ok()?;
    Some((url.host_str()?.to_owned(), url.port_or_known_default()?))
}

fn address(endpoint: &Endpoint) -> String {
    format!("{}:{}", endpoint.0, endpoint.1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reach {
    Answers,
    Silent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Trend {
    Changed,
    Steady,
}

impl Trend {
    const fn recheck_after(self) -> Duration {
        match self {
            Self::Changed => REFRESH,
            Self::Steady => FRESH,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ClaimId(u64);

#[derive(Debug)]
struct Probing {
    claim: ClaimId,
    published: Published,
}

impl Probing {
    fn abandoned(&self) -> bool {
        self.published.has_changed().is_err()
    }
}

struct WakeOnDrop(Arc<Notify>);

impl Drop for WakeOnDrop {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

struct Probe {
    claim: ClaimId,
    publish: watch::Sender<Option<Instant>>,
    _wake: WakeOnDrop,
}

#[derive(Debug)]
enum Entry {
    FirstProbe(Probing),
    Probed {
        reach: Reach,
        at: Instant,
        trend: Trend,
    },
    Reprobing {
        reach: Reach,
        probing: Probing,
    },
}

enum Claim {
    First,
    Again(Reach),
}

impl Entry {
    const fn known(&self) -> Option<Reach> {
        match self {
            Self::FirstProbe(_) => None,
            Self::Probed { reach, .. } | Self::Reprobing { reach, .. } => Some(*reach),
        }
    }

    const fn claim_id(&self) -> Option<ClaimId> {
        match self {
            Self::FirstProbe(probing) | Self::Reprobing { probing, .. } => Some(probing.claim),
            Self::Probed { .. } => None,
        }
    }

    fn first_probe(&self) -> Option<Published> {
        match self {
            Self::FirstProbe(probing) => Some(probing.published.clone()),
            Self::Probed { .. } | Self::Reprobing { .. } => None,
        }
    }

    fn due_at(&self, now: Instant) -> Option<Instant> {
        match self {
            Self::Probed { at, trend, .. } => Some(*at + trend.recheck_after()),
            Self::FirstProbe(probing) | Self::Reprobing { probing, .. } => {
                probing.abandoned().then_some(now)
            }
        }
    }

    fn claim(&self, refresh: Refresh, now: Instant) -> Option<Claim> {
        match (self, refresh) {
            (Self::FirstProbe(probing), _) if probing.abandoned() => Some(Claim::First),
            (Self::Reprobing { reach, probing }, _) if probing.abandoned() => {
                Some(Claim::Again(*reach))
            }
            (&Self::Probed { reach, .. }, Refresh::All) => Some(Claim::Again(reach)),
            (Self::Probed { reach, .. }, Refresh::Due)
                if self.due_at(now).is_some_and(|due| now >= due) =>
            {
                Some(Claim::Again(*reach))
            }
            (Self::Probed { .. } | Self::FirstProbe(_) | Self::Reprobing { .. }, _) => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Refresh {
    Due,
    All,
}

impl Refresh {
    const fn with(self, other: Self) -> Self {
        match (self, other) {
            (Self::Due, Self::Due) => Self::Due,
            (Self::All, _) | (_, Self::All) => Self::All,
        }
    }
}

#[derive(Debug, Default)]
struct Book {
    entries: HashMap<Endpoint, Entry>,
    removed: BTreeSet<Endpoint>,
}

#[derive(Debug, Default)]
pub(crate) struct Reachability {
    book: Mutex<Book>,
    claims: AtomicU64,
    rescheduled: Arc<Notify>,
}

#[derive(Debug, Default)]
pub(crate) struct FirstChecks {
    late: BTreeSet<Endpoint>,
}

pub(crate) enum Change {
    Edited,
    Missed,
    Closed,
}

pub(crate) trait ServerList: Send + Sync {
    fn read(&self) -> impl Future<Output = Result<Vec<McpServer>, ReadMcpServersError>> + Send;
    fn changed(&mut self) -> impl Future<Output = Change> + Send;
}

pub(crate) struct StoredServers {
    store: forge_store::Store,
    changes: forge_store::Subscription,
}

impl StoredServers {
    pub(crate) fn new(store: &forge_store::Store) -> Self {
        Self {
            store: store.clone(),
            changes: store.subscribe(),
        }
    }
}

impl ServerList for StoredServers {
    fn read(&self) -> impl Future<Output = Result<Vec<McpServer>, ReadMcpServersError>> + Send {
        crate::mcp_auth::read_mcp_servers(&self.store)
    }

    async fn changed(&mut self) -> Change {
        loop {
            match self.changes.recv().await {
                Ok(envelope) => {
                    if matches!(envelope.event, Event::Reset) {
                        return Change::Missed;
                    }
                    if let Event::Setting { key, .. } = &envelope.event
                        && key == "mcp_servers"
                    {
                        return Change::Edited;
                    }
                }
                Err(RecvError::Lagged(_)) => return Change::Missed,
                Err(RecvError::Closed) => return Change::Closed,
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Listing {
    Current,
    RetryAt(Instant, Refresh),
}

enum Woke {
    Due,
    Rescheduled,
    Changed(Change),
}

async fn reach(endpoint: Endpoint) -> (Endpoint, Reach) {
    let connect = tokio::net::TcpStream::connect((endpoint.0.as_str(), endpoint.1));
    let reach = match tokio::time::timeout(PATIENCE, connect).await {
        Ok(Ok(_)) => Reach::Answers,
        Ok(Err(_)) | Err(_) => Reach::Silent,
    };
    (endpoint, reach)
}

impl Reachability {
    fn book(&self) -> MutexGuard<'_, Book> {
        self.book.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn probing(&self) -> (Probing, Probe) {
        let claim = ClaimId(self.claims.fetch_add(1, Ordering::Relaxed));
        let (publish, published) = watch::channel(None);
        (
            Probing { claim, published },
            Probe {
                claim,
                publish,
                _wake: WakeOnDrop(Arc::clone(&self.rescheduled)),
            },
        )
    }

    fn claim(&self, servers: &[McpServer], refresh: Refresh) -> BTreeMap<Endpoint, Probe> {
        let now = Instant::now();
        let mut book = self.book();
        let Book { entries, removed } = &mut *book;
        let mut claimed = BTreeMap::new();
        for endpoint in servers.iter().filter_map(endpoint) {
            let claim = match entries.get(&endpoint) {
                None if removed.contains(&endpoint) => None,
                None => Some(Claim::First),
                Some(entry) => entry.claim(refresh, now),
            };
            let entry = match claim {
                Some(Claim::First) => {
                    let (probing, probe) = self.probing();
                    claimed.insert(endpoint.clone(), probe);
                    Entry::FirstProbe(probing)
                }
                Some(Claim::Again(reach)) => {
                    let (probing, probe) = self.probing();
                    claimed.insert(endpoint.clone(), probe);
                    Entry::Reprobing { reach, probing }
                }
                None => continue,
            };
            entries.insert(endpoint, entry);
        }
        claimed
    }

    fn record(&self, endpoint: &Endpoint, reach: Reach, probe: Probe) {
        self.record_at(endpoint, reach, probe, Instant::now());
    }

    fn record_at(&self, endpoint: &Endpoint, reach: Reach, probe: Probe, now: Instant) {
        match self.book().entries.get_mut(endpoint) {
            Some(entry) if entry.claim_id() == Some(probe.claim) => {
                let trend = match entry.known() {
                    Some(was) if was == reach => Trend::Steady,
                    Some(_) | None => Trend::Changed,
                };
                *entry = Entry::Probed {
                    reach,
                    at: now,
                    trend,
                };
            }
            Some(_) | None => tracing::debug!(
                address = %address(endpoint),
                "an MCP server's check finished after a newer check or its removal took over"
            ),
        }
        probe.publish.send_replace(Some(now));
        drop(probe);
    }

    async fn probe(&self, claimed: BTreeMap<Endpoint, Probe>) {
        let mut probes = tokio::task::JoinSet::new();
        let mut owners = HashMap::new();
        for (endpoint, probe) in claimed {
            owners.insert(endpoint.clone(), probe);
            probes.spawn(reach(endpoint));
        }
        while let Some(probed) = probes.join_next().await {
            let (endpoint, reach) = probed
                .expect("invariant: a reachability probe is never cancelled and never panics");
            let probe = owners
                .remove(&endpoint)
                .expect("invariant: every probe finishes once for the claim that started it");
            self.record(&endpoint, reach, probe);
        }
    }

    fn spawn_probe(self: &Arc<Self>, servers: &[McpServer], refresh: Refresh) {
        let claimed = self.claim(servers, refresh);
        if claimed.is_empty() {
            return;
        }
        let reachability = Arc::clone(self);
        drop(tokio::spawn(async move {
            reachability.probe(claimed).await;
        }));
    }

    fn configured(self: &Arc<Self>, servers: &[McpServer], refresh: Refresh) {
        {
            let configured: BTreeSet<Endpoint> = servers.iter().filter_map(endpoint).collect();
            let mut book = self.book();
            let Book { entries, removed } = &mut *book;
            removed.extend(
                entries
                    .keys()
                    .filter(|endpoint| !configured.contains(*endpoint))
                    .cloned(),
            );
            removed.retain(|endpoint| !configured.contains(endpoint));
            entries.retain(|endpoint, _| configured.contains(endpoint));
        }
        self.spawn_probe(servers, refresh);
    }

    fn next_due(&self) -> Option<Instant> {
        let now = Instant::now();
        self.book()
            .entries
            .values()
            .filter_map(|entry| entry.due_at(now))
            .min()
    }

    pub(crate) async fn watch(self: Arc<Self>, mut servers: impl ServerList) {
        let mut listing = self.reread(&servers, Refresh::Due).await;
        loop {
            let wake = match listing {
                Listing::RetryAt(at, _) => Some(at),
                Listing::Current => self.next_due(),
            };
            let event = tokio::select! {
                () = async {
                    match wake {
                        Some(wake) => tokio::time::sleep_until(wake).await,
                        None => std::future::pending::<()>().await,
                    }
                } => Woke::Due,
                () = self.rescheduled.notified() => Woke::Rescheduled,
                change = servers.changed() => Woke::Changed(change),
            };
            let pending = match listing {
                Listing::RetryAt(_, refresh) => refresh,
                Listing::Current => Refresh::Due,
            };
            listing = match event {
                Woke::Due | Woke::Changed(Change::Missed) => self.reread(&servers, pending).await,
                Woke::Rescheduled => listing,
                Woke::Changed(Change::Edited) => {
                    self.reread(&servers, pending.with(Refresh::All)).await
                }
                Woke::Changed(Change::Closed) => return,
            };
        }
    }

    async fn reread(self: &Arc<Self>, servers: &impl ServerList, refresh: Refresh) -> Listing {
        match servers.read().await {
            Ok(configured) => {
                self.configured(&configured, refresh);
                Listing::Current
            }
            Err(error) => {
                tracing::warn!(
                    error = diagnostics::chain(&error),
                    retry_s = REFRESH.as_secs(),
                    "read the MCP servers to check they answer"
                );
                Listing::RetryAt(Instant::now() + REFRESH, refresh)
            }
        }
    }

    pub(crate) async fn join_first_checks(
        self: &Arc<Self>,
        servers: &[McpServer],
        thread: &ThreadId,
    ) -> FirstChecks {
        self.spawn_probe(servers, Refresh::Due);
        let pending: BTreeMap<Endpoint, Published> = {
            let book = self.book();
            servers
                .iter()
                .filter_map(endpoint)
                .filter_map(|endpoint| {
                    let published = book.entries.get(&endpoint)?.first_probe()?;
                    Some((endpoint, published))
                })
                .collect()
        };
        let deadline = Instant::now() + PATIENCE + MARGIN;
        let mut late = BTreeSet::new();
        for (endpoint, mut published) in pending {
            let waited = tokio::time::timeout_at(deadline, published.wait_for(Option::is_some))
                .await
                .map(|result| result.map(|at| at.is_some_and(|at| at <= deadline)));
            let missed = match waited {
                Ok(Ok(true)) => continue,
                Ok(Ok(false)) => {
                    "the first check of an MCP server reported after the turn's deadline"
                }
                Ok(Err(_)) => "the first check of an MCP server ended without an answer",
                Err(_) => "the first check of an MCP server did not report in time",
            };
            tracing::warn!(%thread, address = %address(&endpoint), "{missed}");
            late.insert(endpoint);
        }
        FirstChecks { late }
    }

    pub(crate) fn answering(
        &self,
        servers: &[McpServer],
        checks: &FirstChecks,
        thread: &ThreadId,
    ) -> Vec<McpServer> {
        let book = self.book();
        servers
            .iter()
            .filter(|server| {
                let Some(endpoint) = endpoint(server) else {
                    return true;
                };
                let known = book
                    .entries
                    .get(&endpoint)
                    .and_then(Entry::known)
                    .filter(|_| !checks.late.contains(&endpoint));
                let left_out = match known {
                    Some(Reach::Answers) => return true,
                    Some(Reach::Silent) => {
                        "MCP server did not answer its last check; the turn starts without it"
                    }
                    None if book.removed.contains(&endpoint) => {
                        "MCP server was removed from the configuration; the turn starts without it"
                    }
                    None => {
                        "MCP server has no check that finished in time for this turn; the turn starts without it"
                    }
                };
                tracing::info!(
                    %thread,
                    server = %server.name,
                    address = %address(&endpoint),
                    "{left_out}"
                );
                false
            })
            .cloned()
            .collect()
    }
}

#[cfg(test)]
#[path = "../tests/mcp_reach/support.rs"]
pub(crate) mod support;
