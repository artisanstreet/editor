//! `GET /events?since=N`: every stored change after N, then live changes.
//!
//! Subscribing before reading the backlog means nothing falls in between;
//! anything seen twice is skipped by seq (and clients replace by id anyway).

use crate::routes::App;
use artisan_api::{Envelope, Event, Seq};
use axum::extract::{Query, State};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures::Stream;
use serde::Deserialize;
use std::convert::Infallible;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;

/// More stored changes than this since the client's seq: it refetches `/state`.
const BACKLOG_LIMIT: usize = 20_000;

#[derive(Deserialize)]
pub(crate) struct Since {
    #[serde(default)]
    since: Seq,
}

pub(crate) async fn stream(
    State(app): State<App>,
    Query(Since { since }): Query<Since>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let (sender, receiver) = mpsc::channel::<Envelope>(256);
    tokio::spawn(async move {
        if let Err(error) = forward(&app, since, &sender).await {
            tracing::debug!("event stream ended: {error:#}");
        }
    });
    let events = futures::stream::unfold(receiver, |mut receiver| async move {
        let envelope = receiver.recv().await?;
        let mut event = SseEvent::default();
        if let Some(seq) = envelope.seq {
            event = event.id(seq.to_string());
        }
        let event = event
            .json_data(&envelope)
            .unwrap_or_else(|_| SseEvent::default().comment("unencodable"));
        Some((Ok(event), receiver))
    });
    Sse::new(events).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

async fn forward(app: &App, since: Seq, sender: &mpsc::Sender<Envelope>) -> anyhow::Result<()> {
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
    if let Some(backlog) = store.changes_since(since, BACKLOG_LIMIT).await? {
        for envelope in backlog {
            last = last.max(envelope.seq.unwrap_or(last));
            sender.send(envelope).await?;
        }
    } else {
        send(sender, None, Event::Reset).await?;
        return Ok(());
    }
    for event in app.hub.live_snapshot() {
        send(sender, None, event).await?;
    }
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
    loop {
        match changes.recv().await {
            Ok(envelope) => {
                if let Some(seq) = envelope.seq {
                    if seq <= last {
                        continue;
                    }
                    last = seq;
                }
                sender.send(envelope).await?;
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

async fn send(
    sender: &mpsc::Sender<Envelope>,
    seq: Option<Seq>,
    event: Event,
) -> anyhow::Result<()> {
    sender.send(Envelope { seq, event }).await?;
    Ok(())
}
