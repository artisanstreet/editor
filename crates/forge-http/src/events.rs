//! `GET /events?since=N`: every stored change after N, then live changes.
//!
//! Subscribing before reading the backlog means nothing falls in between;
//! Durable overlap is skipped by seq; memory-only overlap by publication revision.

use crate::routes::App;
use artisan_api::{DeviceId, Envelope, Event, LiveTarget, Seq};
use axum::extract::{Extension, Query, State};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures::Stream;
use serde::Deserialize;
use std::collections::HashSet;
use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};

/// More stored changes than this since the client's seq: it refetches `/state`.
const BACKLOG_LIMIT: usize = 20_000;

const QUEUE_BYTES: usize = 8 * 1024 * 1024;

struct Queued {
    seq: Option<Seq>,
    data: String,
    _bytes: OwnedSemaphorePermit,
}
struct Queue {
    sender: mpsc::Sender<Queued>,
    bytes: Arc<Semaphore>,
}
impl Queue {
    async fn send(&self, envelope: Envelope) -> anyhow::Result<()> {
        let data = serde_json::to_string(&envelope)?;
        let oversized = data.len() > QUEUE_BYTES;
        let data = if oversized {
            serde_json::to_string(&Envelope {
                seq: None,
                live_revision: 0,
                event: Event::Reset,
            })?
        } else {
            data
        };
        let bytes = Arc::clone(&self.bytes)
            .acquire_many_owned(u32::try_from(data.len())?)
            .await?;
        self.sender
            .send(Queued {
                seq: if oversized { None } else { envelope.seq },
                data,
                _bytes: bytes,
            })
            .await?;
        anyhow::ensure!(!oversized, "event exceeds SSE byte budget; client reset");
        Ok(())
    }
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
    pub(crate) const fn tunnel(since: Seq, summary: bool, thread: Option<artisan_api::ThreadId>) -> Self {
        Self {
            since,
            summary,
            thread,
        }
    }

    pub(crate) fn parse_events_path(path: &str) -> Option<Self> {
        let (_, query) = path.split_once('?')?;
        let mut since = 0;
        let mut summary = false;
        let mut thread = None;
        for pair in query.split('&') {
            let (key, value) = pair.split_once('=')?;
            match key {
                "since" => since = value.parse().ok()?,
                "summary" => summary = value == "true",
                "thread" => thread = Some(artisan_api::ThreadId(value.into())),
                _ => return None,
            }
        }
        Some(Self::tunnel(since, summary, thread))
    }
}

/// Forward stored + live envelopes into a bounded tunneled stream.
///
/// Same backlog/live/cursor semantics as [`stream`] (subscribe first, skip
/// durable overlap by seq, memory overlap by publication revision, Reset
/// when the backlog is gone or the broadcast lags). Ends when the receiver
/// is gone (channel closed/cancelled) or a terminal Reset is sent. The
/// caller frames each [`Envelope`] as a `$event` DATA frame; envelopes that
/// would exceed the Noise plaintext cap are chunked by the caller.
#[expect(clippy::too_many_lines, reason = "backlog/live/cursor in one tunnel turn")]
pub async fn forward_tunnel(
    app: &App,
    scope: &Since,
    sender: &tokio::sync::mpsc::Sender<Envelope>,
) -> anyhow::Result<()> {
    let send = async |envelope: Envelope| -> anyhow::Result<()> {
        sender.send(envelope).await.map_err(|_send| anyhow::anyhow!("stream gone"))?;
        Ok(())
    };
    let since = scope.since;
    let store = app.hub.store();
    let mut changes = store.subscribe();
    let head = store.head();
    send(Envelope {
        seq: None,
        live_revision: 0,
        event: Event::Hello {
            head,
            protocol: artisan_api::PROTOCOL_VERSION,
            forge_version: app.forge_version.clone(),
        },
    })
    .await?;
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
                Event::Run { run } if !run.status.is_live() => {
                    completed_runs.insert(run.id.clone());
                }
                Event::Hello { .. }
                | Event::Project { .. }
                | Event::Thread { .. }
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
                send(envelope).await?;
            }
        }
    } else {
        send(Envelope {
            seq: None,
            live_revision: 0,
            event: Event::Reset,
        })
        .await?;
        return Ok(());
    }
    let live_revision =
        send_live_tunnel_snapshot(app, scope, sender, &completed_items, &completed_runs).await?;
    let mut cursor = Cursor {
        last,
        live_revision,
    };
    send_engine_tunnel_statuses(app, sender).await?;
    loop {
        match changes.recv().await {
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
                    | Event::Item { .. }
                    | Event::Run { .. }
                    | Event::ItemState { .. }
                    | Event::Setting { .. }
                    | Event::Live { .. }
                    | Event::Suggestion { .. }
                    | Event::EditorCall { .. }
                    | Event::Reset => {}
                }
                if let Some(envelope) = scoped(envelope, scope) {
                    send(envelope).await?;
                }
            }
            // Fell behind: the client refetches the state and reconnects.
            Err(RecvError::Lagged(_)) => {
                send(Envelope {
                    seq: None,
                    live_revision: 0,
                    event: Event::Reset,
                })
                .await?;
                return Ok(());
            }
            Err(RecvError::Closed) => return Ok(()),
        }
    }
}

async fn send_live_tunnel_snapshot(
    app: &App,
    scope: &Since,
    sender: &tokio::sync::mpsc::Sender<Envelope>,
    completed_items: &HashSet<artisan_api::ItemId>,
    completed_runs: &HashSet<artisan_api::RunId>,
) -> anyhow::Result<u64> {
    let (live_revision, snapshot) = app.hub.live_snapshot();
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
            sender
                .send(envelope)
                .await
                .map_err(|_send| anyhow::anyhow!("stream gone"))?;
        }
    }
    Ok(live_revision)
}

async fn send_engine_tunnel_statuses(
    app: &App,
    sender: &tokio::sync::mpsc::Sender<Envelope>,
) -> anyhow::Result<()> {
    for engine in artisan_api::EngineId::ALL {
        sender
            .send(Envelope {
                seq: None,
                live_revision: 0,
                event: Event::Engine {
                    status: app.hub.engines().status(engine),
                },
            })
            .await
            .map_err(|_send| anyhow::anyhow!("stream gone"))?;
    }
    Ok(())
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
        // Subscribe before rechecking to cover revoke between middleware and stream.
        let revocations = store.subscribe_revocations();
        if store.is_authorized(&client.certificate).ok().flatten().is_none() {
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
                    tracing::warn!(%device, "recording when the device was last seen: {error:#}");
                }
                forward(&app, &scope, &sender).await
            } => {
                if let Err(error) = result {
                    tracing::debug!("event stream ended: {error:#}");
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

async fn forward(app: &App, scope: &Since, sender: &Queue) -> anyhow::Result<()> {
    let since = scope.since;
    let store = app.hub.store();
    let mut changes = store.subscribe();
    let head = store.head();
    send(
        sender,
        None,
        Event::Hello {
            head,
            protocol: artisan_api::PROTOCOL_VERSION,
            forge_version: app.forge_version.clone(),
        },
    )
    .await?;
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
                Event::Run { run } if !run.status.is_live() => {
                    completed_runs.insert(run.id.clone());
                }
                Event::Hello { .. }
                | Event::Project { .. }
                | Event::Thread { .. }
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
                sender.send(envelope).await?;
            }
        }
    } else {
        send(sender, None, Event::Reset).await?;
        return Ok(());
    }
    let live_revision =
        send_live_snapshot(app, scope, sender, &completed_items, &completed_runs).await?;
    let mut cursor = Cursor {
        last,
        live_revision,
    };
    send_engine_statuses(app, sender).await?;
    loop {
        let change = changes.recv().await;
        match change {
            Ok(mut envelope) => {
                if !cursor.accept(&envelope) {
                    continue;
                }
                match &mut envelope.event {
                    Event::Engine { status } => *status = app.hub.engines().status(status.engine),
                    // Queued pre-snapshot progress must never regress the scalar
                    // installation state shown by the initial Engine snapshot.
                    Event::EngineInstall { engine, install } => {
                        *install = app.hub.engines().install_state(*engine);
                    }
                    Event::Hello { .. }
                    | Event::Project { .. }
                    | Event::Thread { .. }
                    | Event::Item { .. }
                    | Event::Run { .. }
                    | Event::ItemState { .. }
                    | Event::Setting { .. }
                    | Event::Live { .. }
                    | Event::Suggestion { .. }
                    | Event::EditorCall { .. }
                    | Event::Reset => {}
                }
                if let Some(envelope) = scoped(envelope, scope) {
                    sender.send(envelope).await?;
                }
            }
            // Fell behind: the client refetches the state and reconnects.
            Err(RecvError::Lagged(_)) => {
                send(sender, None, Event::Reset).await?;
                return Ok(());
            }
            Err(RecvError::Closed) => return Ok(()),
        }
    }
}

/// Sends the live text of every run the backlog didn't see end; returns the
/// snapshot's live revision.
async fn send_live_snapshot(
    app: &App,
    scope: &Since,
    sender: &Queue,
    completed_items: &HashSet<artisan_api::ItemId>,
    completed_runs: &HashSet<artisan_api::RunId>,
) -> anyhow::Result<u64> {
    let (live_revision, snapshot) = app.hub.live_snapshot();
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
            sender.send(envelope).await?;
        }
    }
    Ok(live_revision)
}

/// Sends every engine's current status.
async fn send_engine_statuses(app: &App, sender: &Queue) -> anyhow::Result<()> {
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

struct Cursor {
    last: Seq,
    live_revision: u64,
}
impl Cursor {
    const fn accept(&mut self, envelope: &Envelope) -> bool {
        if let Some(seq) = envelope.seq {
            if seq <= self.last {
                return false;
            }
            self.last = seq;
            true
        } else {
            envelope.live_revision > self.live_revision
        }
    }
}

/// Returns when the device is revoked. The store announces revocations
/// made here at once, and one made by another process (`ae forge devices
/// revoke`) when the next request reaches Forge.
async fn watch_authorization(
    store: &forge_store::Store,
    device: &DeviceId,
    certificate: &str,
    mut revocations: tokio::sync::broadcast::Receiver<DeviceId>,
) {
    loop {
        match revocations.recv().await {
            Ok(revoked) if &revoked == device => return,
            Ok(_) => {}
            Err(RecvError::Closed) => return,
            // Missed announcements: ask directly.
            Err(RecvError::Lagged(_)) => {
                if store.is_authorized(certificate).ok().flatten().is_none() {
                    return;
                }
            }
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

async fn send(sender: &Queue, seq: Option<Seq>, event: Event) -> anyhow::Result<()> {
    sender
        .send(Envelope {
            seq,
            live_revision: 0,
            event,
        })
        .await?;
    Ok(())
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
        let mut cursor = Cursor {
            last: 0,
            live_revision: boundary,
        };
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
            .add_device("client".into(), "certificate".into())
            .await
            .unwrap();
        let revocations = store.subscribe_revocations();
        store.revoke_device(&device).await.unwrap();
        tokio::time::timeout(
            Duration::from_millis(100),
            watch_authorization(&store, &device, "certificate", revocations),
        )
        .await
        .unwrap();
    }
}
