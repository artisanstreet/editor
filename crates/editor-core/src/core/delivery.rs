use aqua::mailbox::{Mailbox, SendError};
use std::{
    num::NonZeroUsize,
    sync::{Arc, Mutex},
};
use tokio::sync::mpsc;

#[derive(Clone, Debug, Default)]
pub struct AdmissionOrder(Arc<Mutex<u64>>);

impl AdmissionOrder {
    pub(crate) fn inspect<R>(&self, inspect: impl FnOnce() -> R) -> R {
        let _guard = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inspect()
    }
}

struct Admitted<T> {
    sequence: u64,
    message: T,
    _permit: tokio::sync::OwnedSemaphorePermit,
}

pub struct Delivery<T> {
    wake: Option<Mailbox<()>>,
    receiver: mpsc::Receiver<Admitted<T>>,
    front: Option<Admitted<T>>,
    sender: DeliverySender<T>,
}

pub struct OwnedDelivery<T> {
    delivery: Delivery<T>,
    mailbox: Mailbox<()>,
}

impl<T> OwnedDelivery<T> {
    pub fn new(capacity: NonZeroUsize) -> Self {
        let mailbox = Mailbox::new(NonZeroUsize::MIN);
        Self {
            delivery: Delivery::with_wake(capacity, mailbox.sender()),
            mailbox,
        }
    }

    pub const fn mailbox(&self) -> &Mailbox<()> {
        &self.mailbox
    }
}

impl<T> std::ops::Deref for OwnedDelivery<T> {
    type Target = Delivery<T>;

    fn deref(&self) -> &Delivery<T> {
        &self.delivery
    }
}

impl<T> std::ops::DerefMut for OwnedDelivery<T> {
    fn deref_mut(&mut self) -> &mut Delivery<T> {
        &mut self.delivery
    }
}

pub struct DeliverySender<T> {
    wake: aqua::mailbox::Sender<()>,
    sender: mpsc::Sender<Admitted<T>>,
    order: AdmissionOrder,
    slots: Arc<tokio::sync::Semaphore>,
}

pub struct MappedSender<T, U> {
    sender: DeliverySender<U>,
    map: fn(T) -> U,
}
impl<T, U> Clone for MappedSender<T, U> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            map: self.map,
        }
    }
}
impl<T, U> MappedSender<T, U> {
    pub(crate) fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }
    pub(crate) async fn send(&self, message: T) -> Result<(), mpsc::error::SendError<U>> {
        self.sender.send((self.map)(message)).await
    }
    pub(crate) async fn offer(&self, message: T) {
        self.sender.offer((self.map)(message)).await;
    }
}
impl<T> Clone for DeliverySender<T> {
    fn clone(&self) -> Self {
        Self {
            wake: self.wake.clone(),
            sender: self.sender.clone(),
            order: self.order.clone(),
            slots: Arc::clone(&self.slots),
        }
    }
}

impl<T> Delivery<T> {
    pub fn new(capacity: NonZeroUsize) -> Self {
        let wake = Mailbox::new(NonZeroUsize::MIN);
        let mut delivery = Self::with_order(capacity, wake.sender(), AdmissionOrder::default());
        delivery.wake = Some(wake);
        delivery
    }
    pub fn with_wake(capacity: NonZeroUsize, wake: aqua::mailbox::Sender<()>) -> Self {
        Self::with_order(capacity, wake, AdmissionOrder::default())
    }
    pub fn with_order(
        capacity: NonZeroUsize,
        wake: aqua::mailbox::Sender<()>,
        order: AdmissionOrder,
    ) -> Self {
        let (sender, receiver) = mpsc::channel(capacity.get());
        Self {
            wake: None,
            receiver,
            front: None,
            sender: DeliverySender {
                wake,
                sender,
                order,
                slots: Arc::new(tokio::sync::Semaphore::new(capacity.get())),
            },
        }
    }
    #[cfg(feature = "test-support")]
    pub(crate) fn reset_for_tests(&mut self) {
        let wake = self.sender.wake.clone();
        let order = self.sender.order.clone();
        let capacity = NonZeroUsize::new(self.receiver.max_capacity())
            .expect("invariant: a delivery channel was built with a non-zero capacity");
        *self = Self::with_order(capacity, wake, order);
    }
    pub fn wake_mailbox(&self) -> Option<&Mailbox<()>> {
        self.wake.as_ref()
    }
    pub fn sender(&self) -> DeliverySender<T> {
        self.sender.clone()
    }
    pub fn waker(&self) -> aqua::mailbox::Wake {
        let wake = self.sender.wake.clone();
        Arc::new(move || match wake.send(()) {
            Ok(()) | Err(SendError::Full(()) | SendError::Closed(())) => {}
        })
    }
    pub fn wake(&self) {
        match self.sender.wake.send(()) {
            Ok(()) | Err(SendError::Full(()) | SendError::Closed(())) => {}
        }
    }
    pub fn peek_order(&mut self) -> Option<u64> {
        if self.front.is_none() {
            self.front = self.receiver.try_recv().ok();
        }
        self.front.as_ref().map(|item| item.sequence)
    }
    pub fn drain_one(&mut self) -> Option<T> {
        let item = self
            .front
            .take()
            .or_else(|| self.receiver.try_recv().ok())?;
        Some(item.message)
    }
    pub fn drain(&mut self) -> Vec<T> {
        if let Some(wake) = &mut self.wake {
            wake.drain();
        }
        let mut messages = Vec::new();
        for _ in 0..self.receiver.max_capacity() {
            match self.front.take().or_else(|| self.receiver.try_recv().ok()) {
                Some(item) => messages.push(item.message),
                None => return messages,
            }
        }
        self.wake();
        messages
    }
}
impl<T> DeliverySender<T> {
    pub(crate) fn map<U>(&self, map: fn(U) -> T) -> MappedSender<U, T> {
        MappedSender {
            sender: self.clone(),
            map,
        }
    }
    fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }
    pub async fn offer(&self, message: T) {
        if self.send(message).await.is_err() {
            tracing::debug!("the delivery's owner has been dropped, so nobody is left to receive");
        }
    }
    pub async fn send(&self, message: T) -> Result<(), mpsc::error::SendError<T>> {
        let slot = Arc::clone(&self.slots)
            .acquire_owned()
            .await
            .expect("invariant: a delivery never closes its admission slots");
        let Ok(permit) = self.sender.reserve().await else {
            return Err(mpsc::error::SendError(message));
        };
        {
            let mut sequence = self
                .order
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let current = *sequence;
            *sequence = sequence
                .checked_add(1)
                .expect("transition sequence exhausted");
            permit.send(Admitted {
                sequence: current,
                message,
                _permit: slot,
            });
        }
        match self.wake.send(()) {
            Ok(()) | Err(SendError::Full(()) | SendError::Closed(())) => {}
        }
        Ok(())
    }
}

impl<T> std::fmt::Debug for Delivery<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Delivery")
            .field("capacity", &self.receiver.max_capacity())
            .field("queued", &self.receiver.len())
            .finish_non_exhaustive()
    }
}

impl<T> std::fmt::Debug for OwnedDelivery<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnedDelivery")
            .field("delivery", &self.delivery)
            .finish_non_exhaustive()
    }
}

impl<T> std::fmt::Debug for DeliverySender<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeliverySender")
            .field("closed", &self.sender.is_closed())
            .finish_non_exhaustive()
    }
}

impl<T, U> std::fmt::Debug for MappedSender<T, U> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MappedSender")
            .field("sender", &self.sender)
            .finish_non_exhaustive()
    }
}
