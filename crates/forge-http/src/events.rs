use crate::routes::App;
use artisan_api::{DeviceId, Envelope, Event, LiveTarget, Seq, ThreadId};
use axum::extract::{Extension, Query, State};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures::Stream;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};

const BACKLOG_LIMIT: usize = 20_000;

const QUEUE_BYTES: usize = 8 * 1024 * 1024;

const AUTHORIZATION_RECHECK: Duration = Duration::from_secs(5);

struct Queued {
    seq: Option<Seq>,
    data: String,
    _bytes: OwnedSemaphorePermit,
}
struct Queue {
    sender: mpsc::Sender<Queued>,
    bytes: Arc<Semaphore>,
}
#[derive(Debug, thiserror::Error)]
pub enum SinkError {
    #[error("encode an event at seq {seq:?} for the stream")]
    Encode {
        seq: Option<Seq>,
        #[source]
        source: serde_json::Error,
    },
    #[error("wait for room in the stream's byte budget")]
    Budget(#[source] tokio::sync::AcquireError),
    #[error("the stream's reader went away")]
    Gone,
    #[error(
        "an event of {bytes} bytes at seq {seq:?} exceeds the stream's byte budget; the client resets"
    )]
    Oversized { seq: Option<Seq>, bytes: usize },
}

impl EnvelopeSink for Queue {
    async fn send(&self, envelope: Envelope) -> Result<(), SinkError> {
        let encoded = |envelope: &Envelope| {
            serde_json::to_string(envelope).map_err(|source| SinkError::Encode {
                seq: envelope.seq,
                source,
            })
        };
        let data = encoded(&envelope)?;
        let oversized = (data.len() > QUEUE_BYTES).then_some(data.len());
        let data = if oversized.is_some() {
            encoded(&Envelope {
                seq: None,
                live_revision: 0,
                event: Event::Reset,
            })?
        } else {
            data
        };
        let budget = u32::try_from(data.len())
            .expect("invariant: a queued event fits the 8 MiB stream budget");
        let bytes = Arc::clone(&self.bytes)
            .acquire_many_owned(budget)
            .await
            .map_err(SinkError::Budget)?;
        let queued = Queued {
            seq: if oversized.is_some() {
                None
            } else {
                envelope.seq
            },
            data,
            _bytes: bytes,
        };
        if self.sender.send(queued).await.is_err() {
            return Err(SinkError::Gone);
        }
        match oversized {
            Some(bytes) => Err(SinkError::Oversized {
                seq: envelope.seq,
                bytes,
            }),
            None => Ok(()),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ForwardError {
    #[error("send the event stream's {step}")]
    Send {
        step: &'static str,
        #[source]
        source: SinkError,
    },
    #[error(transparent)]
    Backlog(#[from] forge_store::ChangesSinceError),
    #[error(transparent)]
    Snapshot(#[from] forge_runs::runtime::SnapshotError),
}

fn sent(step: &'static str) -> impl FnOnce(SinkError) -> ForwardError {
    move |source| ForwardError::Send { step, source }
}

#[derive(Deserialize)]
pub struct Since {
    #[serde(default)]
    #[expect(
        clippy::struct_field_names,
        reason = "the field is the `since` query parameter"
    )]
    since: Seq,
    #[serde(default)]
    summary: bool,
    thread: Option<artisan_api::ThreadId>,
}

impl Since {
    pub(crate) fn from_events_path(path: &str) -> Option<Self> {
        let uri: axum::http::Uri = path.parse().ok()?;
        (uri.path() == "/events")
            .then(|| Query::<Self>::try_from_uri(&uri).ok())
            .flatten()
            .map(|Query(since)| since)
    }
}

pub trait EnvelopeSink: Sync {
    fn send(&self, envelope: Envelope) -> impl Future<Output = Result<(), SinkError>> + Send;
}

impl EnvelopeSink for tokio::sync::mpsc::Sender<Envelope> {
    async fn send(&self, envelope: Envelope) -> Result<(), SinkError> {
        if Self::send(self, envelope).await.is_err() {
            return Err(SinkError::Gone);
        }
        Ok(())
    }
}

pub async fn stream(
    State(app): State<App>,
    Query(scope): Query<Since>,
    Extension(client): Extension<crate::routes::Client>,
    Extension(device): Extension<DeviceId>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let (sender, receiver) = mpsc::channel(64);
    let sender = Queue {
        sender,
        bytes: Arc::new(Semaphore::new(QUEUE_BYTES)),
    };
    let stop = app.shutdown.child_token();
    let stream_stop = stop.clone();
    let tasks = app.tasks.clone();
    tasks.spawn(async move {
        let store = app.hub.store();
        let revocations = store.subscribe_revocations();
        if !still_authorized(store, &client.certificate) {
            stop.cancel();
            return;
        }
        tokio::select! {
            biased;
            () = stop.cancelled() => {},
            () = watch_authorization(store, &device, &client.certificate, revocations) => { stop.cancel(); },
            () = sender.sender.closed() => {},
            result = async {
                if let Err(error) = store.touch_device(&device).await {
                    tracing::warn!(%device, error = diagnostics::chain(&error), "recording when the device was last seen");
                }
                forward(&app, &scope, &sender).await
            } => {
                if let Err(error) = result {
                    tracing::debug!(%device, error = diagnostics::chain(&error), "event stream ended");
                }
            },
        }
    });
    let events =
        futures::stream::unfold((receiver, stream_stop), |(mut receiver, stop)| async move {
            let queued = tokio::select! {
                biased;
                () = stop.cancelled() => return None,
                queued = receiver.recv() => queued?,
            };
            let mut event = SseEvent::default();
            if let Some(seq) = queued.seq {
                event = event.id(seq.to_string());
            }
            let event = event.data(&queued.data);
            drop(queued);
            Some((Ok(event), (receiver, stop)))
        });
    Sse::new(events).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

#[expect(
    clippy::too_many_lines,
    reason = "one stream: backlog, live snapshot, engine statuses, then live changes"
)]
pub async fn forward(
    app: &App,
    scope: &Since,
    sender: &impl EnvelopeSink,
) -> Result<(), ForwardError> {
    let since = scope.since;
    let store = app.hub.store();
    let mut changes = store.subscribe();
    send(
        sender,
        None,
        Event::Hello {
            head: store.head(),
            protocol: artisan_api::PROTOCOL_VERSION,
            forge_version: app.forge_version.clone(),
        },
    )
    .await
    .map_err(sent("hello"))?;
    let mut last = since;
    let (mut completed_items, mut completed_runs) = (HashSet::new(), HashSet::new());
    if let Some(backlog) = store
        .changes_since_scoped(since, BACKLOG_LIMIT, scope.summary, scope.thread.clone())
        .await?
    {
        for envelope in backlog {
            match &envelope.event {
                Event::Item { item } if item.status.is_terminal() => {
                    completed_items.insert(item.id.clone());
                }
                Event::Run { run } if !run.status().is_live() => {
                    completed_runs.insert(run.id.clone());
                }
                Event::Hello { .. }
                | Event::Project { .. }
                | Event::Thread { .. }
                | Event::ThreadRemoved { .. }
                | Event::Item { .. }
                | Event::Run { .. }
                | Event::ItemState { .. }
                | Event::Setting { .. }
                | Event::Live { .. }
                | Event::Suggestion { .. }
                | Event::Engine { .. }
                | Event::EngineInstall { .. }
                | Event::EditorCall { .. }
                | Event::Reset => {}
            }
            last = last.max(envelope.seq.unwrap_or(last));
            if let Some(envelope) = scoped(envelope, scope) {
                sender.send(envelope).await.map_err(sent("backlog"))?;
            }
        }
    } else {
        send(sender, None, Event::Reset)
            .await
            .map_err(sent("reset"))?;
        return Ok(());
    }
    let mut cursor =
        send_live_snapshot(app, scope, sender, last, &completed_items, &completed_runs).await?;
    send_engine_statuses(app, sender)
        .await
        .map_err(sent("engine statuses"))?;
    loop {
        let change = changes.recv().await;
        match change {
            Ok(mut envelope) => {
                if !cursor.accept(&envelope) {
                    continue;
                }
                match &mut envelope.event {
                    Event::Engine { status } => *status = app.hub.engines().status(status.engine),
                    Event::EngineInstall { engine, install } => {
                        *install = app.hub.engines().install_state(*engine);
                    }
                    Event::Hello { .. }
                    | Event::Project { .. }
                    | Event::Thread { .. }
                    | Event::ThreadRemoved { .. }
                    | Event::Item { .. }
                    | Event::Run { .. }
                    | Event::ItemState { .. }
                    | Event::Setting { .. }
                    | Event::Live { .. }
                    | Event::Suggestion { .. }
                    | Event::EditorCall { .. }
                    | Event::Reset => {}
                }
                let reset = matches!(envelope.event, Event::Reset);
                if let Some(envelope) = scoped(envelope, scope) {
                    sender.send(envelope).await.map_err(sent("change"))?;
                }
                if reset {
                    return Ok(());
                }
            }
            Err(RecvError::Lagged(_)) => {
                send(sender, None, Event::Reset)
                    .await
                    .map_err(sent("reset"))?;
                return Ok(());
            }
            Err(RecvError::Closed) => return Ok(()),
        }
    }
}

async fn send_live_snapshot(
    app: &App,
    scope: &Since,
    sender: &impl EnvelopeSink,
    last: Seq,
    completed_items: &HashSet<artisan_api::ItemId>,
    completed_runs: &HashSet<artisan_api::RunId>,
) -> Result<Cursor, ForwardError> {
    let (live_revision, snapshot) = app.hub.distributed_live_snapshot().await?;
    let cursor = Cursor::after_snapshot(last, live_revision, &snapshot);
    for event in snapshot {
        if let Event::Live { target, .. } = &event {
            let completed = match target {
                LiveTarget::Text { item_id }
                | LiveTarget::Shell { item_id }
                | LiveTarget::Progress { item_id } => completed_items.contains(item_id),
                LiveTarget::Reasoning { run_id } | LiveTarget::Status { run_id } => {
                    completed_runs.contains(run_id)
                }
            };
            if completed {
                continue;
            }
        }
        if let Some(envelope) = scoped(
            Envelope {
                seq: None,
                live_revision: 0,
                event,
            },
            scope,
        ) {
            sender.send(envelope).await.map_err(sent("live snapshot"))?;
        }
    }
    Ok(cursor)
}

async fn send_engine_statuses(app: &App, sender: &impl EnvelopeSink) -> Result<(), SinkError> {
    for engine in artisan_api::EngineId::ALL {
        send(
            sender,
            None,
            Event::Engine {
                status: app.hub.engines().status(engine),
            },
        )
        .await?;
    }
    Ok(())
}

pub(crate) struct Cursor {
    last: Seq,
    live_revision: u64,
    covered: HashMap<(ThreadId, LiveTarget), u64>,
}

impl Cursor {
    pub(crate) fn after_snapshot(last: Seq, live_revision: u64, snapshot: &[Event]) -> Self {
        let covered = snapshot
            .iter()
            .filter_map(|event| match event {
                Event::Live {
                    thread_id,
                    target,
                    coverage,
                    ..
                } => Some(((thread_id.clone(), target.clone()), coverage.total)),
                Event::Hello { .. }
                | Event::Project { .. }
                | Event::Thread { .. }
                | Event::ThreadRemoved { .. }
                | Event::Item { .. }
                | Event::Run { .. }
                | Event::ItemState { .. }
                | Event::Setting { .. }
                | Event::Suggestion { .. }
                | Event::Engine { .. }
                | Event::EngineInstall { .. }
                | Event::EditorCall { .. }
                | Event::Reset => None,
            })
            .collect();
        Self {
            last,
            live_revision,
            covered,
        }
    }

    pub(crate) fn accept(&mut self, envelope: &Envelope) -> bool {
        if let Some(seq) = envelope.seq {
            if seq <= self.last {
                return false;
            }
            self.last = seq;
            return true;
        }
        let published_after_snapshot = envelope.live_revision > self.live_revision;
        match &envelope.event {
            Event::Live {
                thread_id,
                target,
                append,
                coverage,
                ..
            } => {
                if !published_after_snapshot {
                    return false;
                }
                let key = (thread_id.clone(), target.clone());
                match self.covered.get(&key) {
                    Some(&total) if *append && coverage.total <= total => false,
                    Some(_) => {
                        self.covered.remove(&key);
                        true
                    }
                    None => true,
                }
            }
            Event::Suggestion { .. } => published_after_snapshot,
            Event::Hello { .. }
            | Event::Project { .. }
            | Event::Thread { .. }
            | Event::ThreadRemoved { .. }
            | Event::Item { .. }
            | Event::Run { .. }
            | Event::ItemState { .. }
            | Event::Setting { .. }
            | Event::Engine { .. }
            | Event::EngineInstall { .. }
            | Event::EditorCall { .. }
            | Event::Reset => true,
        }
    }
}

async fn watch_authorization(
    store: &forge_store::Store,
    device: &DeviceId,
    certificate: &forge_store::DeviceCertificate,
    mut revocations: tokio::sync::broadcast::Receiver<DeviceId>,
) {
    let mut recheck = tokio::time::interval(AUTHORIZATION_RECHECK);
    recheck.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            revoked = revocations.recv() => match revoked {
                Ok(revoked) if &revoked == device => return,
                Ok(_) => {}
                Err(RecvError::Closed) => return,
                Err(RecvError::Lagged(_)) => {
                    if !still_authorized(store, certificate) {
                        return;
                    }
                }
            },
            _ = recheck.tick() => {
                if !still_authorized(store, certificate) {
                    return;
                }
            }
        }
    }
}

fn still_authorized(
    store: &forge_store::Store,
    certificate: &forge_store::DeviceCertificate,
) -> bool {
    match store.is_authorized(certificate) {
        Ok(device) => device.is_some(),
        Err(error) => {
            tracing::warn!(
                error = diagnostics::chain(&error),
                "rechecking an event stream's device"
            );
            false
        }
    }
}

fn scoped(mut envelope: Envelope, scope: &Since) -> Option<Envelope> {
    if !scope.summary {
        return Some(envelope);
    }
    match &envelope.event {
        Event::Item { item } if scope.thread.as_ref() != Some(&item.thread_id) => {
            envelope.event = Event::ItemState {
                id: item.id.clone(),
                thread_id: item.thread_id.clone(),
                status: item.status,
            };
        }
        Event::Live { thread_id, .. } if scope.thread.as_ref() != Some(thread_id) => return None,
        Event::Hello { .. }
        | Event::Project { .. }
        | Event::Thread { .. }
        | Event::ThreadRemoved { .. }
        | Event::Item { .. }
        | Event::Run { .. }
        | Event::ItemState { .. }
        | Event::Setting { .. }
        | Event::Live { .. }
        | Event::Suggestion { .. }
        | Event::Engine { .. }
        | Event::EngineInstall { .. }
        | Event::EditorCall { .. }
        | Event::Reset => {}
    }
    Some(envelope)
}

async fn send(sender: &impl EnvelopeSink, seq: Option<Seq>, event: Event) -> Result<(), SinkError> {
    sender
        .send(Envelope {
            seq,
            live_revision: 0,
            event,
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(text: &str, revision: u64) -> Envelope {
        Envelope {
            seq: None,
            live_revision: revision,
            event: Event::Live {
                thread_id: artisan_api::ThreadId("thread".into()),
                target: LiveTarget::Text {
                    item_id: artisan_api::ItemId("item".into()),
                },
                text: text.into(),
                append: true,
                coverage: artisan_api::LiveCoverage {
                    start: 0,
                    total: text.len() as u64,
                },
            },
        }
    }

    #[tokio::test]
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the test holds its queue and frames until it ends"
    )]
    async fn byte_budget_blocks_even_when_the_entry_queue_has_space() {
        let envelope = live("abc", 1);
        let encoded = serde_json::to_string(&envelope).unwrap().len();
        let (sender, mut receiver) = mpsc::channel(64);
        let queue = Queue {
            sender,
            bytes: Arc::new(Semaphore::new(encoded)),
        };
        queue.send(envelope.clone()).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(10), queue.send(envelope.clone()))
                .await
                .is_err()
        );
        drop(receiver.recv().await.unwrap());
        queue.send(envelope).await.unwrap();
    }

    #[tokio::test]
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the test holds its queue and frames until it ends"
    )]
    async fn oversized_events_send_a_reset_without_retaining_the_large_body() {
        let (sender, mut receiver) = mpsc::channel(64);
        let queue = Queue {
            sender,
            bytes: Arc::new(Semaphore::new(QUEUE_BYTES)),
        };
        assert!(queue.send(live(&"x".repeat(QUEUE_BYTES), 1)).await.is_err());
        let frame = receiver.recv().await.unwrap();
        assert!(matches!(
            serde_json::from_str::<Envelope>(&frame.data).unwrap().event,
            Event::Reset
        ));
        assert_eq!(frame.seq, None);
        assert!(frame.data.len() < 128);
    }

    #[tokio::test]
    async fn queued_live_deltas_covered_by_snapshot_are_not_replayed() {
        let directory = tempfile::tempdir().unwrap();
        let store = forge_store::Store::open(directory.path()).unwrap();
        let mut receiver = store.subscribe();
        store.publish_live(live("before", 0).event);
        // Snapshot sees the accumulated text at exactly this publication boundary.
        let boundary = store.live_revision();
        let snapshot = "before";
        store.publish_live(live("after", 0).event);
        let mut rendered = snapshot.to_owned();
        let mut cursor = Cursor::after_snapshot(0, boundary, &[]);
        for _ in 0..2 {
            let envelope = receiver.recv().await.unwrap();
            if !cursor.accept(&envelope) {
                continue;
            }
            if let Event::Live { text, append, .. } = envelope.event {
                if append {
                    rendered.push_str(&text);
                } else {
                    rendered = text;
                }
            }
        }
        assert_eq!(rendered, "beforeafter");
    }

    #[tokio::test]
    async fn online_revocation_closes_the_authorization_watcher_immediately() {
        let directory = tempfile::tempdir().unwrap();
        let store = forge_store::Store::open(directory.path()).unwrap();
        let device = store
            .add_device(
                "client".into(),
                forge_store::DeviceCertificate::from_der(b"certificate"),
            )
            .await
            .unwrap();
        let revocations = store.subscribe_revocations();
        store.revoke_device(&device).await.unwrap();
        tokio::time::timeout(
            Duration::from_millis(100),
            watch_authorization(
                &store,
                &device,
                &forge_store::DeviceCertificate::from_der(b"certificate"),
                revocations,
            ),
        )
        .await
        .unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn revocation_by_another_process_closes_a_passive_watcher() {
        let directory = tempfile::tempdir().unwrap();
        let store = forge_store::Store::open(directory.path()).unwrap();
        let device = store
            .add_device(
                "client".into(),
                forge_store::DeviceCertificate::from_der(b"certificate"),
            )
            .await
            .unwrap();
        let watcher = tokio::spawn({
            let store = store.clone();
            let device = device.clone();
            let revocations = store.subscribe_revocations();
            async move {
                watch_authorization(
                    &store,
                    &device,
                    &forge_store::DeviceCertificate::from_der(b"certificate"),
                    revocations,
                )
                .await;
            }
        });
        tokio::time::advance(Duration::from_secs(30)).await;
        assert!(
            !watcher.is_finished(),
            "a paired device's stream stays open"
        );
        forge_store::admin::Admin::open(directory.path())
            .unwrap()
            .revoke_device(device.as_str())
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(60), watcher)
            .await
            .expect("a device revoked by `ae forge devices revoke` kept receiving events")
            .unwrap();
    }
}
