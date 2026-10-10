//! Concurrent service of one connection's requests.
//!
//! Every request arrives on its own bidirectional stream, so one request
//! that waits on the disk (a write queued behind a commit the kernel holds
//! for tens of seconds) need not hold up the others. The connection answers
//! up to [`MAX_CONCURRENT_REQUESTS`] requests at once, all inside the
//! connection's own task, in three lanes:
//!
//! - **Subscription requests** (Subscribe, Unsubscribe) change which threads
//!   the delivery driver pushes. Each holds the driver for its whole stage —
//!   answer, reply, activation, and the activated thread's first push — so
//!   it never interleaves with a wake scan or with another subscription
//!   request, and they take the driver in the order their streams were
//!   accepted ([`AcceptOrder`]). The registry's lease fencing therefore sees
//!   exactly the order it saw when the connection was served one request at
//!   a time: a scan never records a batch under a lease a concurrent
//!   Subscribe just replaced, and a Subscribe's response always crosses the
//!   wire before its replay. These requests only read the database and stay
//!   bounded by the request limit; a failure fails the connection as before.
//! - **Lifecycle requests** run alone: one waits until every request already
//!   being answered has replied, and no request starts its answer while it
//!   runs, so an accepted stop never cuts another request short.
//! - **Every other request is independent.** Its answer takes as long as the
//!   handler takes — a write queued behind a slow disk is delayed, never
//!   failed by the request limit — while reading the request and writing
//!   its reply, which wait on the peer, stay bounded by that limit. One
//!   independent request that fails resets its own stream and is reported;
//!   the connection keeps serving the rest. A peer that is gone is noticed
//!   by the connection itself (its idle timeout), which ends the service
//!   through the accept.
//!
//! After its reply every request still hands its follow-up to the driver
//! (the listing it served becomes what later pushes are compared against,
//! and host-state changes are looked for), and wake scans run as one more
//! task beside the requests, one at a time.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;
use std::convert::Infallible;
use std::sync::{Arc, Mutex as StdMutex, PoisonError};
use std::time::{Duration, Instant};

use artisan_domain::{ConversationRequest, ErrorChain, RequestId};
use artisan_protocol::{ClientRequest, ProtocolVersion};
use artisan_transport::{
    CancelHandle, DeadlineError, IncomingRequest, OperationKind, receive_server_request,
    run_with_deadline, send_server_reply,
};
use futures::stream::{FuturesUnordered, StreamExt};
use quinn::{Connection, RecvStream, SendStream};
use tokio::sync::{Mutex, MutexGuard, Notify, RwLock, RwLockReadGuard, RwLockWriteGuard};

use super::{
    ForgeConnection, LifecycleWitness, RequestDispatchOutcome, RequestStageError, ServerFrameStamp,
    StageStreams, answer, complete_request_receipt, delivery_stage_failure, reply_envelope,
};
use crate::conversation_delivery_driver::{ConversationDeliveryDriver, ConversationWakes};
use crate::lifecycle_control::LifecycleController;
use crate::request_handler::{ConversationConnectionContext, RequestHandler};

/// Requests answered at once on one connection. Matches the bidirectional
/// streams the transport lets a peer keep open, so every stream a peer
/// opens is accepted and none waits behind a slow one.
pub(super) const MAX_CONCURRENT_REQUESTS: usize = 16;

/// An independent request whose answer takes longer than this is reported,
/// so a slow disk shows up in the journal by the request it delayed.
const SLOW_REQUEST: Duration = Duration::from_secs(2);

/// How one connection's service ended.
pub(super) enum ServiceEnd {
    /// The request budget was reached and every request finished.
    Capacity { completed: u32 },
    /// The connection failed, or the caller cancelled it.
    Failed {
        completed: u32,
        source: DeadlineError<RequestStageError>,
    },
}

/// One connection's requests in flight and what they share.
pub(super) struct RequestLanes<'a, F> {
    connection: &'a Connection,
    handler: &'a RequestHandler,
    lifecycle: &'a LifecycleController,
    witness: LifecycleWitness,
    protocol_version: ProtocolVersion,
    limit: Duration,
    cancel: &'a CancelHandle,
    context: Arc<ConversationConnectionContext>,
    /// The delivery driver, and with it the subscription lane.
    driver: Mutex<ConversationDeliveryDriver>,
    /// Read by every request while it answers; written by a lifecycle
    /// request, which therefore runs alone.
    exclusive: RwLock<()>,
    order: AcceptOrder,
    stamp: StdMutex<F>,
}

impl<'a, F> RequestLanes<'a, F>
where
    F: FnMut() -> Result<ServerFrameStamp, RequestStageError>,
{
    pub(super) fn new(
        owner: &'a ForgeConnection<'_, '_, '_, '_>,
        driver: ConversationDeliveryDriver,
        stamp: F,
    ) -> Self {
        Self {
            connection: &owner.connection,
            handler: owner.handler,
            lifecycle: owner.lifecycle,
            witness: owner.lifecycle_witness,
            protocol_version: owner.protocol_version,
            limit: owner.limits.next_request,
            cancel: owner.cancel,
            context: driver.context(),
            driver: Mutex::new(driver),
            exclusive: RwLock::new(()),
            order: AcceptOrder::default(),
            stamp: StdMutex::new(stamp),
        }
    }

    /// Hands the driver back for cleanup once the service ended.
    pub(super) fn into_driver(self) -> ConversationDeliveryDriver {
        self.driver.into_inner()
    }

    /// Accepts and answers requests, and scans on every wake, until the
    /// request budget is reached, the connection fails, or the caller
    /// cancels. Requests and scans still in flight when it fails are
    /// dropped with their stream guards, which reset unfinished replies.
    pub(super) async fn serve(&self, capacity: u32, mut wakes: ConversationWakes) -> ServiceEnd {
        let mut tasks = FuturesUnordered::new();
        let (mut admitted, mut completed) = (0_u32, 0_u32);
        let (mut answering, mut scanning) = (0_usize, false);
        loop {
            if admitted >= capacity && answering == 0 && !scanning {
                return ServiceEnd::Capacity { completed };
            }
            let accepting = admitted < capacity && answering < MAX_CONCURRENT_REQUESTS;
            // Ordinary desktop idle is not a request failure: waiting for
            // the next stream or wake has no deadline. Started requests keep
            // their bounds at their own sites below.
            tokio::select! {
                biased;
                () = self.cancel.wait() => {
                    return ServiceEnd::Failed {
                        completed,
                        source: DeadlineError::Cancelled {
                            operation: OperationKind::Receive,
                        },
                    };
                }
                Some(end) = tasks.next() => match end {
                    TaskEnd::Request(RequestEnd::Served) => {
                        answering -= 1;
                        completed += 1;
                    }
                    TaskEnd::Request(RequestEnd::Dropped) => answering -= 1,
                    TaskEnd::Request(RequestEnd::Fatal(source))
                    | TaskEnd::Scan(Err(source)) => {
                        return ServiceEnd::Failed { completed, source };
                    }
                    TaskEnd::Scan(Ok(())) => scanning = false,
                },
                accepted = self.connection.accept_bi(), if accepting => match accepted {
                    Ok((send, recv)) => {
                        admitted += 1;
                        answering += 1;
                        let ticket = self.order.issue();
                        tasks.push(self.run(Task::Request { ticket, send, recv }));
                    }
                    Err(source) => {
                        return ServiceEnd::Failed {
                            completed,
                            source: DeadlineError::Peer {
                                operation: OperationKind::Receive,
                                error: RequestStageError::Accept { source },
                            },
                        };
                    }
                },
                woken = wakes.wait(), if !scanning => match woken {
                    Ok(()) => {
                        scanning = true;
                        tasks.push(self.run(Task::Scan));
                    }
                    Err(error) => {
                        return ServiceEnd::Failed {
                            completed,
                            source: delivery_stage_failure(OperationKind::Receive, error),
                        };
                    }
                },
            }
        }
    }

    /// One task of the connection; a single future type for both kinds.
    async fn run(&self, task: Task<'_>) -> TaskEnd {
        match task {
            Task::Request { ticket, send, recv } => {
                TaskEnd::Request(self.serve_request(ticket, send, recv).await)
            }
            Task::Scan => TaskEnd::Scan(self.scan().await),
        }
    }

    fn stamp(&self) -> Result<ServerFrameStamp, RequestStageError> {
        let mut stamp = self.stamp.lock().unwrap_or_else(PoisonError::into_inner);
        (*stamp)()
    }

    /// Re-reads every active subscription after a coalesced wake.
    async fn scan(&self) -> Result<(), DeadlineError<RequestStageError>> {
        let mut driver = self.driver.lock().await;
        #[cfg(feature = "flight-recorder")]
        let _trace = artisan_tracing::span!("forge.request", "delivery.wake");
        driver
            .deliver_wake(&mut || self.stamp(), self.limit, self.cancel)
            .await
    }

    async fn serve_request(
        &self,
        ticket: Ticket<'_>,
        send: SendStream,
        recv: RecvStream,
    ) -> RequestEnd {
        // Spans the whole stage: reading the request, handling it (its own
        // `request.with_receipt` span) and sending the reply, so a stalled
        // stage's dump shows which of the three it is stuck in.
        #[cfg(feature = "flight-recorder")]
        let _trace = artisan_tracing::span!("forge.request", "request.stage");
        let frame = match self.stamp() {
            Ok(frame) => frame,
            Err(error) => {
                return RequestEnd::Fatal(DeadlineError::Peer {
                    operation: OperationKind::Send,
                    error,
                });
            }
        };
        let mut streams = StageStreams::new();
        let (_send, recv) = streams.install(send, recv);
        let incoming = match run_with_deadline(
            OperationKind::Receive,
            self.limit,
            self.cancel,
            receive_server_request::<Infallible>(recv),
        )
        .await
        {
            Ok(incoming) => incoming,
            Err(error) => return self.dropped(None, "reading the request", &error),
        };
        match lane(&incoming.request) {
            Lane::Subscription => {
                let driver = ticket.take_turn(&self.driver).await;
                let answering = self.exclusive.read().await;
                self.serve_in_order(frame, incoming, streams, Hold::Driver(driver, answering))
                    .await
            }
            Lane::Exclusive => {
                drop(ticket);
                let alone = self.exclusive.write().await;
                self.serve_in_order(frame, incoming, streams, Hold::Alone(alone))
                    .await
            }
            Lane::Independent => {
                drop(ticket);
                self.serve_independent(frame, incoming, streams).await
            }
        }
    }

    /// Answers a subscription or lifecycle request under the request limit,
    /// as the serial connection did; any failure fails the connection.
    async fn serve_in_order(
        &self,
        frame: ServerFrameStamp,
        incoming: IncomingRequest,
        mut streams: StageStreams,
        hold: Hold<'_>,
    ) -> RequestEnd {
        let IncomingRequest {
            request_id,
            request,
            ..
        } = incoming;
        let stage = run_with_deadline(OperationKind::Receive, self.limit, self.cancel, async {
            let (answered, receipt) = answer(
                self.handler,
                self.lifecycle,
                self.witness,
                Some(&self.context),
                &request_id,
                request,
            )
            .await;
            let reply = reply_envelope(self.protocol_version, frame, answered);
            send_server_reply::<Infallible>(streams.reply_stream(), &reply, &request_id).await?;
            complete_request_receipt(
                self.handler,
                Some(&self.context),
                &mut streams,
                receipt,
                self.cancel,
            )
            .await
        })
        .await;
        match stage {
            Ok(outcome) => {
                // The stage completed: hand both directions back for
                // ordinary drops without cleanup actions.
                streams.release();
                // A lifecycle request lets the others answer again before
                // it waits for the driver: a subscription request may hold
                // the driver while it waits for its turn to answer.
                let driver = match hold {
                    Hold::Driver(driver, _answering) => Some(driver),
                    Hold::Alone(alone) => {
                        drop(alone);
                        None
                    }
                };
                self.follow_up(outcome, driver).await
            }
            Err(source) => RequestEnd::Fatal(source),
        }
    }

    /// Answers an independent request beside the others. The answer is not
    /// bounded; reading and replying are.
    async fn serve_independent(
        &self,
        frame: ServerFrameStamp,
        incoming: IncomingRequest,
        mut streams: StageStreams,
    ) -> RequestEnd {
        let IncomingRequest {
            request_id,
            request,
            ..
        } = incoming;
        let family = family(&request);
        let answering = self.exclusive.read().await;
        let started = Instant::now();
        let (answered, receipt) = answer(
            self.handler,
            self.lifecycle,
            self.witness,
            Some(&self.context),
            &request_id,
            request,
        )
        .await;
        let took = started.elapsed();
        if took >= SLOW_REQUEST {
            eprintln!(
                "Forge request {request_id} ({family}) took {} ms to answer",
                took.as_millis()
            );
            #[cfg(feature = "flight-recorder")]
            artisan_tracing::instant!("forge.request", "request.slow", "request_id" => request_id.as_str(), "family" => family, "took_ms" => took.as_millis());
        }
        let reply = reply_envelope(self.protocol_version, frame, answered);
        if let Err(error) = run_with_deadline(
            OperationKind::Send,
            self.limit,
            self.cancel,
            send_server_reply::<Infallible>(streams.reply_stream(), &reply, &request_id),
        )
        .await
        {
            return self.dropped(Some((&request_id, family)), "sending the reply", &error);
        }
        drop(answering);
        match complete_request_receipt(
            self.handler,
            Some(&self.context),
            &mut streams,
            receipt,
            self.cancel,
        )
        .await
        {
            Ok(outcome) => {
                streams.release();
                self.follow_up(outcome, None).await
            }
            Err(error) => RequestEnd::Fatal(DeadlineError::Peer {
                operation: OperationKind::Receive,
                error,
            }),
        }
    }

    /// Applies one answered request's follow-up on the driver, after its
    /// reply finished, exactly as the serial connection did after each
    /// request.
    async fn follow_up(
        &self,
        outcome: RequestDispatchOutcome,
        driver: Option<MutexGuard<'_, ConversationDeliveryDriver>>,
    ) -> RequestEnd {
        let mut driver = match driver {
            Some(driver) => driver,
            None => self.driver.lock().await,
        };
        #[cfg(feature = "flight-recorder")]
        let _trace = artisan_tracing::span!("forge.request", "delivery.after_request");
        match driver
            .handle_request(outcome, &mut || self.stamp(), self.limit, self.cancel)
            .await
        {
            Ok(()) => RequestEnd::Served,
            Err(source) => RequestEnd::Fatal(source),
        }
    }

    /// Ends one request that failed on its own; its stream guard resets the
    /// stream and the connection keeps serving. Requests that fail because
    /// the connection itself is gone, or is being cancelled, are not
    /// reported one by one: the service ends with that cause.
    fn dropped<E>(
        &self,
        request: Option<(&RequestId, &'static str)>,
        step: &'static str,
        error: &E,
    ) -> RequestEnd
    where
        E: std::error::Error + 'static,
    {
        if self.connection.close_reason().is_some() || self.cancel.is_cancelled() {
            return RequestEnd::Dropped;
        }
        match request {
            Some((request_id, family)) => eprintln!(
                "Forge request {request_id} ({family}) failed {step}: {}",
                ErrorChain(error)
            ),
            None => eprintln!("Forge request failed {step}: {}", ErrorChain(error)),
        }
        #[cfg(feature = "flight-recorder")]
        artisan_tracing::instant!("forge.request", "request.dropped", "step" => step);
        RequestEnd::Dropped
    }
}

enum Task<'o> {
    Request {
        ticket: Ticket<'o>,
        send: SendStream,
        recv: RecvStream,
    },
    Scan,
}

/// What an in-order request holds while it is answered.
enum Hold<'l> {
    /// A subscription request: the driver, and a share of the answering.
    Driver(
        MutexGuard<'l, ConversationDeliveryDriver>,
        RwLockReadGuard<'l, ()>,
    ),
    /// A lifecycle request: all of the answering.
    Alone(RwLockWriteGuard<'l, ()>),
}

enum TaskEnd {
    Request(RequestEnd),
    Scan(Result<(), DeadlineError<RequestStageError>>),
}

enum RequestEnd {
    /// Answered, replied, and followed up.
    Served,
    /// Failed on its own; the connection serves on.
    Dropped,
    /// Failed the connection.
    Fatal(DeadlineError<RequestStageError>),
}

enum Lane {
    Independent,
    Subscription,
    Exclusive,
}

fn lane(request: &ClientRequest) -> Lane {
    match request {
        ClientRequest::Conversation(
            ConversationRequest::Subscribe(_) | ConversationRequest::Unsubscribe(_),
        ) => Lane::Subscription,
        ClientRequest::Lifecycle(_) => Lane::Exclusive,
        _ => Lane::Independent,
    }
}

/// The payload-free family of a request, for reports.
const fn family(request: &ClientRequest) -> &'static str {
    match request {
        ClientRequest::Query(_) => "query",
        ClientRequest::Command(_) => "command",
        ClientRequest::Conversation(_) => "conversation",
        ClientRequest::Lifecycle(_) => "lifecycle",
        ClientRequest::ValidateDirectory(_) => "validate directory",
        ClientRequest::PickDirectory => "pick directory",
        ClientRequest::ResolveRichLink(_) => "resolve rich link",
        ClientRequest::QueryProjectRepository(_) => "query project repository",
    }
}

/// Keeps subscription requests in the order their streams were accepted.
///
/// Every accepted stream holds a ticket until it is known not to need the
/// subscription lane (its request was read and is not a subscription
/// request, or it failed). A subscription request takes the driver only once
/// no earlier ticket is outstanding, and gives its ticket up only after it
/// holds the driver, so the next one queues behind it.
#[derive(Default)]
struct AcceptOrder {
    state: StdMutex<OrderState>,
    changed: Notify,
}

#[derive(Default)]
struct OrderState {
    next: u64,
    outstanding: BTreeSet<u64>,
}

impl AcceptOrder {
    fn issue(&self) -> Ticket<'_> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let number = state.next;
        state.next += 1;
        state.outstanding.insert(number);
        Ticket {
            order: self,
            number,
        }
    }

    fn is_first(&self, number: u64) -> bool {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .outstanding
            .first()
            == Some(&number)
    }

    fn give_up(&self, number: u64) {
        let removed = self
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .outstanding
            .remove(&number);
        if removed {
            self.changed.notify_waiters();
        }
    }
}

/// One accepted stream's place in [`AcceptOrder`]; given up on drop.
struct Ticket<'o> {
    order: &'o AcceptOrder,
    number: u64,
}

impl Ticket<'_> {
    /// Waits until every earlier stream is settled or holds the lane, then
    /// takes the lane.
    async fn take_turn<T>(self, lane: &Mutex<T>) -> MutexGuard<'_, T> {
        loop {
            let changed = self.order.changed.notified();
            tokio::pin!(changed);
            // Registered before the check, so a give-up between the check
            // and the wait still wakes this one.
            changed.as_mut().enable();
            if self.order.is_first(self.number) {
                break;
            }
            changed.await;
        }
        let turn = lane.lock().await;
        drop(self);
        turn
    }
}

impl Drop for Ticket<'_> {
    fn drop(&mut self) {
        self.order.give_up(self.number);
    }
}
