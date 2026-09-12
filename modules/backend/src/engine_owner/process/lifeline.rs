//! The sole stdin lifeline writer for one engine child.
//!
//! The writer is removed from the [`EngineChild`] immediately after spawn and
//! then travels whole with the child custody: no other `ChildStdin` handle
//! ever exists beside it. Closing the lifeline is therefore the single EOF
//! source, and every abnormal teardown closes it before waiting or killing.
//! The type is itself an [`AsyncWrite`] so protocol writers borrow the same
//! handle instead of duplicating it.

#![forbid(unsafe_code)]

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::AsyncWrite;
use tokio::process::ChildStdin;

use super::EngineChild;

/// The taken sole stdin writer kept open for the whole operation.
///
/// The writer is removed from the [`EngineChild`] immediately after spawn so the
/// child's own `wait()` can never close it implicitly; it closes exactly
/// when the owner drops it.
pub(crate) struct LifelineWriter(Option<ChildStdin>);

impl LifelineWriter {
    /// Takes the child's piped stdin as the sole lifeline writer.
    pub(crate) fn take(child: &mut EngineChild) -> Self {
        Self(child.stdin.take())
    }

    /// Closes the sole writer end explicitly.
    ///
    /// Every abnormal teardown calls this before waiting or killing.
    pub(crate) fn close(&mut self) {
        self.0 = None;
    }

    /// Returns whether the lifeline still holds the live writer.
    #[allow(dead_code)]
    pub(crate) fn is_open(&self) -> bool {
        self.0.is_some()
    }
}

impl AsyncWrite for LifelineWriter {
    fn poll_write(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut().0.as_mut() {
            Some(writer) => Pin::new(writer).poll_write(context, buffer),
            None => Poll::Ready(Err(closed_error())),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut().0.as_mut() {
            Some(writer) => Pin::new(writer).poll_flush(context),
            None => Poll::Ready(Ok(())),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.get_mut().close();
        Poll::Ready(Ok(()))
    }
}

/// Typed, payload-free write failure for an already-closed lifeline.
fn closed_error() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "lifeline writer is closed")
}
