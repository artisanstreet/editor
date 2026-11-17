use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

pub fn poll_once<F: Future + ?Sized>(future: Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}
