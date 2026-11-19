use crate::core::client::{CallError, ForgeClient, ResponseError};
use crate::core::delivery::MappedSender;
use crate::core::host::HostMessage;
use crate::core::hosts::HostId;
use crate::core::runtime::runtime;
use artisan_api::{Envelope, Event, Seq, ThreadId};
use futures::StreamExt as _;
use sse_core::{SseDecoder, SseEvent, SseStream, SseStreamError};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, watch};
use tokio_util::task::AbortOnDropHandle;

const FRAME_LIMIT: NonZeroUsize = NonZeroUsize::new(4 * 1024 * 1024).unwrap();
const EVENT_BUDGET: usize = 16 * 1024 * 1024;
const HEALTHY_STREAM: Duration = Duration::from_secs(30);
const _: () = assert!(
    FRAME_LIMIT.get() <= EVENT_BUDGET,
    "an admitted event must fit the event budget"
);

#[derive(Debug)]
pub enum Update {
    State(Box<artisan_api::State>),
    Event(Box<Envelope>, tokio::sync::OwnedSemaphorePermit),
    Status(ConnectionStatus),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectionStatus {
    Connecting,
    Reconnecting {
        attempt: u32,
    },
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
    Dropped,
}

#[derive(Clone, Copy, Debug)]
pub struct Deadlines {
    pub open: Duration,
    pub heartbeat: Duration,
}

impl Deadlines {
    pub const FORGE: Self = Self {
        open: Duration::from_secs(30),
        heartbeat: Duration::from_secs(45),
    };
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("read Forge's state")]
    State(#[source] CallError),
    #[error("Forge did not open its event stream within {} seconds", within.as_secs_f32())]
    OpenTimedOut { within: Duration },
    #[error("open the event stream")]
    Open(#[source] reqwest::Error),
    #[error("Forge refused the event stream")]
    Refused(#[source] ResponseError),
    #[error("read Forge's event stream")]
    Read(#[source] reqwest::Error),
    #[error("Forge's event stream sent nothing, not even a heartbeat, for {} seconds", silent_for.as_secs())]
    Silent { silent_for: Duration },
    #[error("a Forge event exceeded the {limit}-byte frame limit; resnapshot required")]
    FrameTooLarge { limit: usize },
    #[error("Forge sent an invalid event; resnapshot required")]
    InvalidEvent(#[source] serde_json::Error),
    #[error("Forge's event protocol changed to {protocol}; update Forge or the Editor")]
    ProtocolChanged { protocol: u32 },
    #[error("subscribe to Forge's events through the Relay")]
    Subscribe(#[source] artisan_account::relay_client::RelayCallError),
    #[error("read Forge's events through the Relay")]
    RelayEvent(#[source] artisan_account::relay_client::EventStreamError),
    #[error("measure a Relay event")]
    Encode(#[source] serde_json::Error),
}

impl SessionError {
    fn code(&self) -> Option<artisan_api::ErrorCode> {
        match self {
            Self::State(error) => error.api_code(),
            Self::Refused(error) => Some(error.code()),
            Self::ProtocolChanged { .. } => Some(artisan_api::ErrorCode::VersionMismatch),
            Self::Subscribe(error) => match &*error.failure {
                artisan_account::relay_client::CallFailure::Refused(error) => Some(error.code),
                artisan_account::relay_client::CallFailure::Closed
                | artisan_account::relay_client::CallFailure::Open(_)
                | artisan_account::relay_client::CallFailure::Exchange(_)
                | artisan_account::relay_client::CallFailure::TimedOut
                | artisan_account::relay_client::CallFailure::Decode(_)
                | artisan_account::relay_client::CallFailure::Bytes(_)
                | artisan_account::relay_client::CallFailure::TooLarge { .. } => None,
            },
            Self::OpenTimedOut { .. }
            | Self::Open(_)
            | Self::Read(_)
            | Self::Silent { .. }
            | Self::FrameTooLarge { .. }
            | Self::InvalidEvent(_)
            | Self::RelayEvent(_)
            | Self::Encode(_) => None,
        }
    }
}

pub trait UpdateSink: Send + Sync {
    fn deliver(&self, update: Update) -> impl Future<Output = bool> + Send;
    fn receiver_gone(&self) -> bool;
}

impl<U: Send> UpdateSink for MappedSender<Update, U> {
    async fn deliver(&self, update: Update) -> bool {
        self.send(update).await.is_ok()
    }

    fn receiver_gone(&self) -> bool {
        self.is_closed()
    }
}

impl UpdateSink for tokio::sync::mpsc::Sender<Update> {
    async fn deliver(&self, update: Update) -> bool {
        self.send(update).await.is_ok()
    }

    fn receiver_gone(&self) -> bool {
        self.is_closed()
    }
}

pub(super) fn follow(
    host: &HostId,
    client: ForgeClient,
    updates: MappedSender<Update, HostMessage>,
    retry: Arc<Notify>,
    observed: watch::Receiver<Option<ThreadId>>,
) -> AbortOnDropHandle<()> {
    let span = tracing::info_span!("host_events", %host);
    AbortOnDropHandle::new(runtime().spawn(tracing::Instrument::instrument(
        run(client, updates, retry, observed, Deadlines::FORGE),
        span,
    )))
}

pub async fn run(
    client: ForgeClient,
    mut updates: impl UpdateSink,
    retry: Arc<Notify>,
    mut observed: watch::Receiver<Option<ThreadId>>,
    deadlines: Deadlines,
) {
    let budget = Arc::new(tokio::sync::Semaphore::new(EVENT_BUDGET));
    let mut since = None;
    let mut attempt = 0_u32;
    loop {
        let connecting = match attempt {
            0 => ConnectionStatus::Connecting,
            attempt => ConnectionStatus::Reconnecting { attempt },
        };
        if (since.is_none() || attempt > 0) && !updates.deliver(Update::Status(connecting)).await {
            return;
        }
        let mut live_since = None;
        let result = session(
            &client,
            &mut since,
            &mut updates,
            &retry,
            &mut observed,
            &mut live_since,
            &budget,
            deadlines,
        )
        .await;
        if updates.receiver_gone() {
            return;
        }
        let (reason, code) = match result {
            Ok(End::Reset) => {
                since = None;
                attempt = 0;
                continue;
            }
            Ok(End::Observation) => continue,
            Ok(End::Dropped) => return,
            Ok(End::Closed) => ("Forge's event stream closed".to_owned(), None),
            Err(error) => (::diagnostics::chain(&error), error.code()),
        };
        if live_since.is_some_and(|live: tokio::time::Instant| live.elapsed() >= HEALTHY_STREAM) {
            attempt = 0;
        }
        attempt = attempt.saturating_add(1);
        if !updates
            .deliver(Update::Status(ConnectionStatus::Offline {
                attempt,
                reason,
                code,
            }))
            .await
        {
            return;
        }
        tokio::select! {
            () = tokio::time::sleep(backoff(attempt)) => {},
            () = retry.notified() => {},
            _ = observed.changed() => {},
        }
    }
}

fn backoff(attempt: u32) -> Duration {
    Duration::from_millis(250 * 2_u64.saturating_pow(attempt.min(6))).min(Duration::from_secs(15))
}

async fn session(
    client: &ForgeClient,
    since: &mut Option<Seq>,
    updates: &mut impl UpdateSink,
    retry: &Notify,
    observed: &mut watch::Receiver<Option<ThreadId>>,
    live_since: &mut Option<tokio::time::Instant>,
    budget: &Arc<tokio::sync::Semaphore>,
    deadlines: Deadlines,
) -> Result<End, SessionError> {
    let head = if let Some(head) = *since {
        head
    } else {
        let state = client.state().await.map_err(SessionError::State)?;
        let head = state.head;
        if !updates.deliver(Update::State(Box::new(state))).await {
            return Ok(End::Dropped);
        }
        *since = Some(head);
        head
    };
    if client.is_relay() {
        return relay_session(
            client, &head, since, updates, retry, observed, live_since, budget,
        )
        .await;
    }
    let observation = observed.borrow_and_update().clone();
    let mut query = vec![("summary", "true".to_owned())];
    if let Some(thread) = observation {
        query.push(("thread", thread.to_string()));
    }
    let response = tokio::time::timeout(deadlines.open, async {
        let response = client
            .raw()
            .get(client.events_url(head))
            .query(&query)
            .header("accept", "text/event-stream")
            .send()
            .await
            .map_err(SessionError::Open)?;
        crate::core::client::check(response)
            .await
            .map_err(SessionError::Refused)
    })
    .await
    .map_err(|_elapsed| SessionError::OpenTimedOut {
        within: deadlines.open,
    })??;
    if !updates
        .deliver(Update::Status(ConnectionStatus::Live))
        .await
    {
        return Ok(End::Dropped);
    }
    *live_since = Some(tokio::time::Instant::now());
    let mut stream = SseStream::with_decoder(SseDecoder::with_limit(FRAME_LIMIT));
    stream.attach(with_heartbeat(response.bytes_stream(), deadlines.heartbeat));
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
            Err(SseStreamError::Inner(Transport::Read(error))) => {
                return Err(SessionError::Read(error));
            }
            Err(SseStreamError::Inner(Transport::Silent)) => {
                return Err(SessionError::Silent {
                    silent_for: deadlines.heartbeat,
                });
            }
            Err(SseStreamError::PayloadTooLarge(_)) => {
                *since = None;
                return Err(SessionError::FrameTooLarge {
                    limit: FRAME_LIMIT.get(),
                });
            }
        };
        let SseEvent::Message(message) = event else {
            continue;
        };
        let envelope = decode_envelope(&message.data, since)?;
        let Some(admitted) =
            admit(envelope, message.data.len(), since, budget, retry, observed).await?
        else {
            return Ok(End::Observation);
        };
        match admitted {
            Admitted::Reset => return Ok(End::Reset),
            Admitted::Event(envelope, permit) => {
                let seq = envelope.seq;
                if !updates.deliver(Update::Event(envelope, permit)).await {
                    return Ok(End::Dropped);
                }
                if let Some(seq) = seq {
                    *since = Some(seq);
                }
            }
        }
    }
}

enum Transport {
    Read(reqwest::Error),
    Silent,
}

impl std::fmt::Display for Transport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read(error) => error.fmt(formatter),
            Self::Silent => formatter.write_str("no heartbeat"),
        }
    }
}

fn with_heartbeat<B: Send + 'static>(
    bytes: impl futures::Stream<Item = reqwest::Result<B>> + Send + 'static,
    deadline: Duration,
) -> futures::stream::BoxStream<'static, Result<B, Transport>> {
    futures::stream::unfold(Some(Box::pin(bytes)), move |bytes| async move {
        let mut bytes = bytes?;
        match tokio::time::timeout(deadline, bytes.next()).await {
            Ok(Some(Ok(chunk))) => Some((Ok(chunk), Some(bytes))),
            Ok(Some(Err(error))) => Some((Err(Transport::Read(error)), Some(bytes))),
            Ok(None) => None,
            Err(_elapsed) => Some((Err(Transport::Silent), None)),
        }
    })
    .boxed()
}

enum Admitted {
    Reset,
    Event(Box<Envelope>, tokio::sync::OwnedSemaphorePermit),
}

pub fn admission_bytes(bytes: usize) -> Result<u32, SessionError> {
    if bytes > FRAME_LIMIT.get() {
        return Err(SessionError::FrameTooLarge {
            limit: FRAME_LIMIT.get(),
        });
    }
    Ok(u32::try_from(bytes.max(1))
        .expect("invariant: an admitted event fits the 4 MiB frame limit"))
}

async fn admit(
    envelope: Envelope,
    bytes: usize,
    since: &mut Option<Seq>,
    budget: &Arc<tokio::sync::Semaphore>,
    retry: &Notify,
    observed: &mut watch::Receiver<Option<ThreadId>>,
) -> Result<Option<Admitted>, SessionError> {
    if matches!(envelope.event, Event::Reset) {
        return Ok(Some(Admitted::Reset));
    }
    if let Event::Hello { protocol, .. } = &envelope.event
        && *protocol != artisan_api::PROTOCOL_VERSION
    {
        *since = None;
        return Err(SessionError::ProtocolChanged {
            protocol: *protocol,
        });
    }
    let permits = admission_bytes(bytes).inspect_err(|_too_large| *since = None)?;
    let permit = tokio::select! {
        permit = Arc::clone(budget).acquire_many_owned(permits) => {
            permit.expect("invariant: the event budget semaphore is never closed")
        }
        () = retry.notified() => return Ok(None),
        _ = observed.changed() => return Ok(None),
    };
    Ok(Some(Admitted::Event(Box::new(envelope), permit)))
}

#[expect(
    clippy::too_many_arguments,
    reason = "session, cursor, channels, and budget travel together; boxing them would churn the hot reconnect path"
)]
async fn relay_session(
    client: &ForgeClient,
    head: &Seq,
    since: &mut Option<Seq>,
    updates: &mut impl UpdateSink,
    retry: &Notify,
    observed: &mut watch::Receiver<Option<ThreadId>>,
    live_since: &mut Option<tokio::time::Instant>,
    budget: &Arc<tokio::sync::Semaphore>,
) -> Result<End, SessionError> {
    let relay = Arc::clone(
        client
            .relay()
            .expect("invariant: a relay session runs only for a Relay client"),
    );
    let observation = observed.borrow_and_update().clone();
    let mut path = format!("/events?since={head}&summary=true");
    if let Some(thread) = observation {
        path.push_str("&thread=");
        path.push_str(&thread.to_string());
    }
    let mut stream = relay
        .subscribe(&path)
        .await
        .map_err(SessionError::Subscribe)?;
    if !updates
        .deliver(Update::Status(ConnectionStatus::Live))
        .await
    {
        return Ok(End::Dropped);
    }
    *live_since = Some(tokio::time::Instant::now());
    loop {
        let next = tokio::select! {
            next = stream.next_event() => next,
            () = retry.notified() => {
                unsubscribe(&mut stream).await;
                return Ok(End::Observation);
            },
            _ = observed.changed() => {
                unsubscribe(&mut stream).await;
                return Ok(End::Observation);
            },
        };
        let Some(envelope) = next.map_err(SessionError::RelayEvent)? else {
            return Ok(End::Closed);
        };
        let bytes = serde_json::to_vec(&envelope)
            .map_err(SessionError::Encode)?
            .len();
        let Some(admitted) = admit(envelope, bytes, since, budget, retry, observed).await? else {
            unsubscribe(&mut stream).await;
            return Ok(End::Observation);
        };
        match admitted {
            Admitted::Reset => return Ok(End::Reset),
            Admitted::Event(envelope, permit) => {
                let seq = envelope.seq;
                if !updates.deliver(Update::Event(envelope, permit)).await {
                    return Ok(End::Dropped);
                }
                if let Some(seq) = seq {
                    *since = Some(seq);
                }
            }
        }
    }
}

async fn unsubscribe(stream: &mut artisan_account::relay_client::RelayEventStream) {
    if let Err(error) = stream.unsubscribe().await {
        tracing::debug!(
            error = %::diagnostics::chain(&error),
            "unsubscribe from Forge's events through the Relay"
        );
    }
}

fn decode_envelope(data: &str, since: &mut Option<Seq>) -> Result<Envelope, SessionError> {
    serde_json::from_str(data).map_err(|error| {
        *since = None;
        SessionError::InvalidEvent(error)
    })
}
