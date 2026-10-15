//! One owned connection: fetch state, follow bounded SSE, resume after failures.

use crate::client::ForgeClient;
use crate::runtime::runtime;
use anyhow::{Context, Result, bail};
use artisan_api::{Envelope, Event, Seq, ThreadId};
use futures::channel::mpsc::Sender;
use futures::{SinkExt as _, StreamExt as _};
use sse_core::{SseDecoder, SseEvent, SseStream, SseStreamError};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, watch};
use tokio_util::task::AbortOnDropHandle;

#[derive(Debug)]
pub enum Update {
    State(Box<artisan_api::State>),
    Event(Box<Envelope>, tokio::sync::OwnedSemaphorePermit),
    Status(ConnectionStatus),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionStatus {
    Connecting,
    Live,
    Offline {
        attempt: u32,
        reason: String,
        code: Option<artisan_api::ErrorCode>,
    },
}

enum End {
    Closed,
    Reset,
    Observation,
}

/// Dropping the returned owner aborts even a stalled network read.
pub fn follow(
    client: ForgeClient,
    mut updates: Sender<Update>,
    retry: Arc<Notify>,
    mut observed: watch::Receiver<Option<ThreadId>>,
) -> AbortOnDropHandle<()> {
    AbortOnDropHandle::new(runtime().spawn(async move {
        let budget = Arc::new(tokio::sync::Semaphore::new(16 * 1024 * 1024));
        let mut since = None;
        let mut attempt = 0_u32;
        loop {
            // Changing the observed thread replaces the stream filter, not the
            // host connection. Only bootstrap and failure recovery are connecting.
            if (since.is_none() || attempt > 0)
                && updates
                    .send(Update::Status(ConnectionStatus::Connecting))
                    .await
                    .is_err()
            {
                return;
            }
            let result = session(
                &client,
                &mut since,
                &mut updates,
                &retry,
                &mut observed,
                &mut attempt,
                &budget,
            )
            .await;
            if updates.is_closed() {
                return;
            }
            let (reason, code) = match result {
                Ok(End::Reset) => {
                    since = None;
                    attempt = 0;
                    continue;
                }
                Ok(End::Observation) => continue,
                Ok(End::Closed) => ("Forge's event stream closed".to_owned(), None),
                Err(error) => (
                    format!("{error:#}"),
                    error
                        .downcast_ref::<artisan_api::ApiError>()
                        .map(|error| error.code),
                ),
            };
            attempt = attempt.saturating_add(1);
            if updates
                .send(Update::Status(ConnectionStatus::Offline {
                    attempt,
                    reason,
                    code,
                }))
                .await
                .is_err()
            {
                return;
            }
            tokio::select! {
                () = tokio::time::sleep(backoff(attempt)) => {},
                () = retry.notified() => {},
                _ = observed.changed() => {},
            }
        }
    }))
}

fn backoff(attempt: u32) -> Duration {
    Duration::from_millis(250 * 2_u64.saturating_pow(attempt.min(6))).min(Duration::from_secs(15))
}

async fn session(
    client: &ForgeClient,
    since: &mut Option<Seq>,
    updates: &mut Sender<Update>,
    retry: &Notify,
    observed: &mut watch::Receiver<Option<ThreadId>>,
    attempt: &mut u32,
    budget: &Arc<tokio::sync::Semaphore>,
) -> Result<End> {
    let head = if let Some(head) = *since {
        head
    } else {
        let state = client.state().await?;
        let head = state.head;
        updates
            .send(Update::State(Box::new(state)))
            .await
            .ok()
            .context("store closed")?;
        *since = Some(head);
        head
    };
    let observation = observed.borrow_and_update().clone();
    let mut query = vec![("summary", "true".to_owned())];
    if let Some(thread) = observation {
        query.push(("thread", thread.to_string()));
    }
    let response = tokio::time::timeout(
        Duration::from_secs(30),
        client
            .raw()
            .get(client.events_url(head))
            .query(&query)
            .header("accept", "text/event-stream")
            .send(),
    )
    .await
    .context("Forge did not open its event stream")?
    .context("open the event stream")?;
    let response = crate::client::check(response).await?;
    updates
        .send(Update::Status(ConnectionStatus::Live))
        .await
        .ok()
        .context("store closed")?;
    *attempt = 0;
    let mut stream = SseStream::with_decoder(SseDecoder::with_limit(
        NonZeroUsize::new(4 * 1024 * 1024).unwrap(),
    ));
    stream.attach(response.bytes_stream());
    loop {
        let event = tokio::select! {
            event = stream.next() => event,
            () = retry.notified() => return Ok(End::Observation),
            _ = observed.changed() => return Ok(End::Observation),
        };
        let Some(event) = event else {
            return Ok(End::Closed);
        };
        let event = match event {
            Ok(event) => event,
            Err(SseStreamError::Inner(error)) => {
                return Err(error).context("read Forge event stream");
            }
            Err(SseStreamError::PayloadTooLarge(error)) => {
                *since = None;
                return Err(error)
                    .context("Forge event exceeded the frame limit; resnapshot required");
            }
        };
        let SseEvent::Message(message) = event else {
            continue;
        };
        let envelope = decode_envelope(&message.data, since)?;
        if matches!(envelope.event, Event::Reset) {
            return Ok(End::Reset);
        }
        if let Event::Hello { protocol, .. } = &envelope.event
            && *protocol != artisan_api::PROTOCOL_VERSION
        {
            *since = None;
            bail!(artisan_api::ApiError {
                code: artisan_api::ErrorCode::VersionMismatch,
                message: "Forge's event protocol changed; update Forge or the Editor".into()
            });
        }
        let seq = envelope.seq;
        let permit = budget
            .clone()
            .acquire_many_owned(message.data.len().max(1) as u32)
            .await
            .context("event queue closed")?;
        updates
            .send(Update::Event(Box::new(envelope), permit))
            .await
            .ok()
            .context("store closed")?;
        if let Some(seq) = seq {
            *since = Some(seq);
        }
    }
}

fn decode_envelope(data: &str, since: &mut Option<Seq>) -> Result<Envelope> {
    serde_json::from_str(data).map_err(|error| {
        *since = None;
        anyhow::Error::from(error).context("Forge sent an invalid event; resnapshot required")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn utf8_and_crlf_survive_every_network_boundary() {
        let event = Envelope {
            seq: Some(19),
            live_revision: 0,
            event: Event::Setting {
                key: "label".into(),
                value: "blå 🦀".into(),
            },
        };
        let data = serde_json::to_string(&event).unwrap();
        let wire = format!("id: 19\r\ndata: {data}\r\n\r\n").into_bytes();
        for split in 1..wire.len() {
            let mut decoder = SseDecoder::new();
            let mut events = Vec::new();
            for bytes in [&wire[..split], &wire[split..]] {
                let mut cursor = std::io::Cursor::new(bytes);
                while let Some(event) = decoder.next(&mut cursor) {
                    events.push(event.unwrap());
                }
            }
            let [SseEvent::Message(message)] = events.as_slice() else {
                panic!("one complete message at split {split}");
            };
            assert_eq!(decode_envelope(&message.data, &mut None).unwrap(), event);
        }
    }
    #[test]
    fn invalid_json_invalidates_the_resume_cursor() {
        let mut since = Some(19);
        assert!(decode_envelope("{broken", &mut since).is_err());
        assert_eq!(since, None);
    }
    #[test]
    fn oversized_stream_reports_an_error() {
        let mut decoder = SseDecoder::with_limit(NonZeroUsize::new(64).unwrap());
        let mut input = std::io::Cursor::new(format!("data: {}\n\n", "x".repeat(128)).into_bytes());
        let mut rejected = false;
        while let Some(event) = decoder.next(&mut input) {
            rejected |= event.is_err();
        }
        assert!(rejected);
    }
}
