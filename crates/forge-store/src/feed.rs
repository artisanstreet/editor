use artisan_api::Envelope;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use tokio::sync::broadcast::error::{RecvError, TryRecvError};
use tokio::sync::watch;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Budget {
    pub bytes: usize,
    pub envelopes: usize,
}

#[derive(Debug)]
struct Held {
    envelope: Envelope,
    bytes: usize,
}

#[derive(Debug)]
struct Ring {
    budget: Budget,
    first: u64,
    held: VecDeque<Held>,
    bytes: usize,
    readers: Vec<Weak<AtomicU64>>,
}

impl Ring {
    fn next(&self) -> u64 {
        self.first + self.held.len() as u64
    }

    fn slowest_reader(&mut self) -> u64 {
        self.readers.retain(|reader| reader.strong_count() > 0);
        self.readers
            .iter()
            .filter_map(Weak::upgrade)
            .map(|cursor| cursor.load(Ordering::Acquire))
            .min()
            .unwrap_or_else(|| self.next())
    }

    fn trim(&mut self) {
        let slowest = self.slowest_reader();
        while let Some(front) = self.held.front() {
            let read_by_all = self.first < slowest;
            let over_budget =
                self.bytes > self.budget.bytes || self.held.len() > self.budget.envelopes;
            if !read_by_all && !over_budget {
                break;
            }
            self.bytes -= front.bytes;
            self.held.pop_front();
            self.first += 1;
        }
    }
}

#[derive(Debug)]
struct Shared {
    ring: Mutex<Ring>,
}

impl Shared {
    fn ring(&self) -> MutexGuard<'_, Ring> {
        self.ring.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[derive(Clone)]
pub(crate) struct Feed {
    shared: Arc<Shared>,
    sent: Arc<watch::Sender<u64>>,
}

impl Feed {
    pub(crate) fn new(budget: Budget) -> Self {
        Self {
            shared: Arc::new(Shared {
                ring: Mutex::new(Ring {
                    budget,
                    first: 0,
                    held: VecDeque::new(),
                    bytes: 0,
                    readers: Vec::new(),
                }),
            }),
            sent: Arc::new(watch::Sender::new(0)),
        }
    }

    pub(crate) fn send(&self, envelope: Envelope) {
        let next = {
            let mut ring = self.shared.ring();
            let bytes = crate::encoded::len(&envelope).min(ring.budget.bytes);
            ring.bytes += bytes;
            ring.held.push_back(Held { envelope, bytes });
            ring.trim();
            ring.next()
        };
        self.sent.send_replace(next);
    }

    pub(crate) fn subscribe(&self) -> Subscription {
        let mut ring = self.shared.ring();
        let cursor = Arc::new(AtomicU64::new(ring.next()));
        ring.readers.push(Arc::downgrade(&cursor));
        drop(ring);
        Subscription {
            shared: Arc::clone(&self.shared),
            cursor,
            sent: self.sent.subscribe(),
        }
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        self.shared.ring().bytes
    }

    pub(crate) fn retained_envelopes(&self) -> usize {
        self.shared.ring().held.len()
    }
}

#[derive(Debug)]
pub struct Subscription {
    shared: Arc<Shared>,
    cursor: Arc<AtomicU64>,
    sent: watch::Receiver<u64>,
}

enum Taken {
    Envelope(Box<Envelope>),
    Lagged(u64),
    Empty,
}

impl Subscription {
    fn take(&self) -> Taken {
        let mut ring = self.shared.ring();
        let cursor = self.cursor.load(Ordering::Acquire);
        if cursor < ring.first {
            self.cursor.store(ring.first, Ordering::Release);
            return Taken::Lagged(ring.first - cursor);
        }
        let offset = usize::try_from(cursor - ring.first)
            .expect("invariant: a feed cursor stays within the ring's length");
        let Some(held) = ring.held.get(offset) else {
            return Taken::Empty;
        };
        let envelope = Box::new(held.envelope.clone());
        self.cursor.store(cursor + 1, Ordering::Release);
        ring.trim();
        Taken::Envelope(envelope)
    }

    pub async fn recv(&mut self) -> Result<Envelope, RecvError> {
        loop {
            self.sent.borrow_and_update();
            match self.take() {
                Taken::Envelope(envelope) => return Ok(*envelope),
                Taken::Lagged(missed) => return Err(RecvError::Lagged(missed)),
                Taken::Empty => {}
            }
            if self.sent.changed().await.is_err() {
                return match self.take() {
                    Taken::Envelope(envelope) => Ok(*envelope),
                    Taken::Lagged(missed) => Err(RecvError::Lagged(missed)),
                    Taken::Empty => Err(RecvError::Closed),
                };
            }
        }
    }

    pub fn try_recv(&self) -> Result<Envelope, TryRecvError> {
        match self.take() {
            Taken::Envelope(envelope) => Ok(*envelope),
            Taken::Lagged(missed) => Err(TryRecvError::Lagged(missed)),
            Taken::Empty if self.sent.has_changed().is_err() => Err(TryRecvError::Closed),
            Taken::Empty => Err(TryRecvError::Empty),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.cursor.load(Ordering::Acquire) >= self.shared.ring().next()
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let mut ring = self.shared.ring();
        let reader = Arc::downgrade(&self.cursor);
        ring.readers.retain(|other| !other.ptr_eq(&reader));
        ring.trim();
    }
}
