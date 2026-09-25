//! What the account-usage service pushes to connected Editors, and the
//! refresh cadence that keeps it current.
//!
//! The Forge owns when usage is read: while at least one Editor connection
//! observes it (it made a request; a lifecycle-only connection never does),
//! [`refresh_while_observed`] re-reads every engine shortly
//! before its report goes stale (and a failing engine at most once a
//! minute), so readiness verdicts stay fresh without any Editor polling.
//! Whenever what the service would serve changes (a new report, a failure,
//! or a verdict that lapsed with its freshness window), it bumps the host
//! state and every connection's delivery driver pushes the new snapshot.

use std::sync::{MutexGuard, OnceLock, PoisonError};

use artisan_transport::CancelHandle;

use super::*;
use crate::conversation_commit_notifier::ConversationCommitNotifier;

/// How long before a report goes stale the refresher reads it again.
const REFRESH_LEAD: Duration = Duration::from_secs(60);
/// How often the refresher looks for due engines.
const REFRESH_TICK: Duration = Duration::from_secs(20);
/// How long an engine waits after a refresh attempt before the next one.
const RETRY_AFTER: Duration = Duration::from_secs(60);

/// Push state owned by one service.
#[derive(Debug, Default)]
pub(super) struct UsagePush {
    notifier: OnceLock<ConversationCommitNotifier>,
    published: Mutex<Vec<EngineUsageSnapshot>>,
    attempted: Mutex<HashMap<String, Instant>>,
    /// The last report served per engine with its observation time; the
    /// source for engines no read has cached (a failure, a missing sign-in).
    served: Mutex<HashMap<String, (EngineUsageReport, String)>>,
    refresh: tokio::sync::Notify,
    observers: std::sync::atomic::AtomicUsize,
}

/// One Editor connection observing usage; the refresher keeps usage fresh
/// while any is held, and releases the connection's share when dropped.
#[derive(Debug)]
pub(crate) struct UsageObserver(Arc<AccountUsageService>);

impl Drop for UsageObserver {
    fn drop(&mut self) {
        self.0
            .push
            .observers
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

impl UsagePush {
    /// Remembers one served report.
    pub(super) fn record_served(&self, report: &EngineUsageReport, observed_at: &str) {
        lock(&self.served).insert(
            report.engine_id().to_owned(),
            (report.clone(), observed_at.to_owned()),
        );
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl AccountUsageService {
    /// Pushes this service's changes to connected Editors through the
    /// notifier's host state.
    #[must_use]
    pub fn with_host_state_notifier(self, notifier: ConversationCommitNotifier) -> Self {
        let _ = self.push.notifier.set(notifier);
        self
    }

    /// Registers one observing Editor connection and asks the refresher to
    /// read what is due now.
    pub(crate) fn observe(self: &Arc<Self>) -> UsageObserver {
        self.push
            .observers
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.push.refresh.notify_one();
        UsageObserver(Arc::clone(self))
    }

    /// Every engine the service has observed, as it would serve it now, one
    /// narrowed snapshot per engine in roster order (so each keeps its own
    /// observation time): the last-good report (marked with a later refresh
    /// failure) with its readiness verdict judged against the freshness
    /// window as it stands. Empty before the first observation.
    #[must_use]
    pub fn current_snapshots(&self) -> Vec<EngineUsageSnapshot> {
        let failures = lock(&self.failures).clone();
        let served = lock(&self.push.served).clone();
        let cache = lock(&self.cache);
        let mut snapshots = Vec::new();
        for reader in &self.readers {
            let engine = reader.engine_id();
            let (report, observed_at, fresh) = match (cache.get(engine), served.get(engine)) {
                (Some(cached), _) => (
                    match failures.get(engine) {
                        Some(failure) => cached
                            .report
                            .clone()
                            .with_failure(failure.clone())
                            .unwrap_or_else(|_| cached.report.clone()),
                        None => cached.report.clone(),
                    },
                    cached.fetched_at.clone(),
                    cached.observed.elapsed() < self.freshness,
                ),
                (None, Some((report, observed_at))) => (report.clone(), observed_at.clone(), false),
                (None, None) => continue,
            };
            let readiness = crate::account_readiness::engine_readiness(
                report.display_name(),
                Some(&report),
                fresh,
                None,
            );
            snapshots.extend(
                EngineUsageSnapshot::new(vec![report.with_readiness(readiness)], observed_at).ok(),
            );
        }
        snapshots
    }

    /// Bumps the host state when what the service would serve changed.
    pub(super) fn publish_if_changed(&self) {
        let snapshots = self.current_snapshots();
        let mut published = lock(&self.push.published);
        if *published == snapshots {
            return;
        }
        *published = snapshots;
        drop(published);
        if let Some(notifier) = self.push.notifier.get() {
            notifier.publish_host_state();
        }
    }

    /// Engines to read now: never read, or close to stale, and not
    /// attempted within [`RETRY_AFTER`]. Marks them attempted.
    fn due_engines(&self) -> Vec<String> {
        let cache = lock(&self.cache);
        let mut attempted = lock(&self.push.attempted);
        let stale_after = self.freshness.saturating_sub(REFRESH_LEAD);
        let due: Vec<String> = self
            .readers
            .iter()
            .map(|reader| reader.engine_id())
            .filter(|engine| {
                cache
                    .get(*engine)
                    .is_none_or(|cached| cached.observed.elapsed() >= stale_after)
                    && attempted
                        .get(*engine)
                        .is_none_or(|at| at.elapsed() >= RETRY_AFTER)
            })
            .map(str::to_owned)
            .collect();
        for engine in &due {
            attempted.insert(engine.clone(), Instant::now());
        }
        due
    }
}

/// Keeps every engine's usage fresh while an Editor observes it, until
/// `cancel` fires. Reads run concurrently, each under the service's
/// per-engine deadline.
pub(crate) async fn refresh_while_observed(
    service: Arc<AccountUsageService>,
    cancel: Arc<CancelHandle>,
) {
    loop {
        tokio::select! {
            () = cancel.wait() => return,
            () = service.push.refresh.notified() => {}
            () = tokio::time::sleep(REFRESH_TICK) => {}
        }
        if service
            .push
            .observers
            .load(std::sync::atomic::Ordering::Relaxed)
            == 0
        {
            continue;
        }
        let mut reads = JoinSet::new();
        for engine in service.due_engines() {
            let Ok(query) = ReadAccountUsage::new(Some(engine), true) else {
                continue;
            };
            let service = Arc::clone(&service);
            reads.spawn(async move {
                service.read(&query).await;
            });
        }
        while reads.join_next().await.is_some() {}
        // A verdict can lapse with its window even without a new read.
        service.publish_if_changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Reader {
        engine: &'static str,
        signed_in: bool,
    }

    impl AccountUsageReader for Reader {
        fn engine_id(&self) -> &'static str {
            self.engine
        }

        fn display_name(&self) -> &'static str {
            self.engine
        }

        fn read(
            &self,
        ) -> Pin<Box<dyn Future<Output = Result<ProviderUsage, ReaderFailure>> + Send + '_>>
        {
            let signed_in = self.signed_in;
            Box::pin(async move {
                if signed_in {
                    Ok(ProviderUsage::authenticated(Vec::new()))
                } else {
                    Err(ReaderFailure::new(
                        EngineUsageAuthentication::Unauthenticated,
                        None,
                        QuotaSurface::Unknown,
                        "not signed in",
                    ))
                }
            })
        }
    }

    #[tokio::test]
    async fn reads_publish_what_the_forge_would_serve_including_failures() {
        let notifier = ConversationCommitNotifier::new();
        let mut wake = notifier.subscribe_any();
        let service = AccountUsageService::with_readers(
            vec![
                Arc::new(Reader {
                    engine: "codex",
                    signed_in: true,
                }) as Arc<dyn AccountUsageReader>,
                Arc::new(Reader {
                    engine: "claude",
                    signed_in: false,
                }),
            ],
            ACCOUNT_USAGE_FRESHNESS,
            ACCOUNT_USAGE_PER_ENGINE_TIMEOUT,
        )
        .with_host_state_notifier(notifier.clone());
        assert!(service.current_snapshots().is_empty());
        let before = wake.host_revision();

        service
            .read(&ReadAccountUsage::new(None, false).expect("query"))
            .await;
        let verdicts: Vec<_> = service
            .current_snapshots()
            .iter()
            .flat_map(EngineUsageSnapshot::engines)
            .map(|report| (report.engine_id().to_owned(), report.readiness().is_ready()))
            .collect();
        assert_eq!(
            verdicts,
            [("codex".to_owned(), true), ("claude".to_owned(), false)]
        );
        let after = wake.host_revision();
        assert_ne!(after, before, "a new snapshot bumps the host state");

        service.publish_if_changed();
        assert_eq!(
            wake.host_revision(),
            after,
            "an unchanged snapshot is quiet"
        );
    }

    #[test]
    fn due_engines_skip_fresh_and_recently_attempted_engines() {
        let service = AccountUsageService::with_readers(
            vec![Arc::new(Reader {
                engine: "codex",
                signed_in: true,
            }) as Arc<dyn AccountUsageReader>],
            ACCOUNT_USAGE_FRESHNESS,
            ACCOUNT_USAGE_PER_ENGINE_TIMEOUT,
        );
        assert_eq!(service.due_engines(), ["codex"]);
        assert!(service.due_engines().is_empty(), "attempted a moment ago");
    }
}
