//! The process's one turn to write, and the stalls it makes visible.
//!
//! Every write of the process queues here before it touches SQLite (see
//! [`Repository::begin_write`](super::Repository)), so one write the disk
//! holds up holds up every write behind it. A busy disk can keep a commit in
//! the kernel for tens of seconds; the gate therefore remembers which
//! operation holds it and since when, and reports to the process log (and
//! the flight recorder) when a write waits for its turn, or keeps it, longer
//! than [`STALL_THRESHOLD`]. A waiter reports once when it crosses the
//! threshold, naming the holder, and again with its total wait when its turn
//! comes; a holder reports how long it kept the gate when it lets go.

#![forbid(unsafe_code)]

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// How long a write may wait for, or keep, the gate before it is reported.
pub(crate) const STALL_THRESHOLD: Duration = Duration::from_secs(2);

/// The write turn shared by every clone of one repository.
#[derive(Debug, Default)]
pub(crate) struct WriteGate {
    turn: Arc<tokio::sync::Mutex<()>>,
    holder: Mutex<Option<Holder>>,
}

/// The operation that holds the gate and when it took it.
#[derive(Clone, Copy, Debug)]
struct Holder {
    operation: &'static str,
    since: Instant,
}

impl WriteGate {
    /// Waits for `operation`'s turn to write, in arrival order.
    pub(crate) async fn acquire(self: &Arc<Self>, operation: &'static str) -> WriteGateGuard {
        let waiting_since = Instant::now();
        let turn = Arc::clone(&self.turn).lock_owned();
        tokio::pin!(turn);
        let turn = match tokio::time::timeout(STALL_THRESHOLD, &mut turn).await {
            Ok(turn) => turn,
            Err(_elapsed) => {
                self.report_stalled_wait(operation, waiting_since);
                let turn = turn.await;
                let waited = waiting_since.elapsed();
                eprintln!(
                    "database write `{operation}` got the write gate after waiting {} ms",
                    waited.as_millis()
                );
                artisan_tracing::instant!("database", "write_gate.waited", "operation" => operation, "waited_ms" => waited.as_millis());
                turn
            }
        };
        let since = Instant::now();
        *self.holder.lock().unwrap_or_else(PoisonError::into_inner) =
            Some(Holder { operation, since });
        WriteGateGuard {
            gate: Arc::clone(self),
            operation,
            since,
            _turn: turn,
        }
    }

    /// The operation holding the gate, if any.
    #[cfg(test)]
    pub(crate) fn holder(&self) -> Option<&'static str> {
        self.holder
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .map(|holder| holder.operation)
    }

    fn report_stalled_wait(&self, operation: &'static str, waiting_since: Instant) {
        let holder = *self.holder.lock().unwrap_or_else(PoisonError::into_inner);
        let waited = waiting_since.elapsed().as_millis();
        if let Some(Holder {
            operation: held_by,
            since,
        }) = holder
        {
            let held = since.elapsed().as_millis();
            eprintln!(
                "database write `{operation}` has waited {waited} ms for the write gate, \
                 held by `{held_by}` for {held} ms"
            );
            artisan_tracing::instant!("database", "write_gate.stalled", "operation" => operation, "waited_ms" => waited, "held_by" => held_by, "held_ms" => held);
        } else {
            eprintln!("database write `{operation}` has waited {waited} ms for the write gate");
            artisan_tracing::instant!("database", "write_gate.stalled", "operation" => operation, "waited_ms" => waited);
        }
        artisan_tracing::incident!("database.write_gate.stalled");
    }
}

/// One write's turn; dropping it lets the next writer in.
#[derive(Debug)]
pub(crate) struct WriteGateGuard {
    gate: Arc<WriteGate>,
    operation: &'static str,
    since: Instant,
    _turn: tokio::sync::OwnedMutexGuard<()>,
}

impl Drop for WriteGateGuard {
    fn drop(&mut self) {
        // Cleared while the turn is still held, so a waiter never reads a
        // holder that already let go as the current one.
        *self
            .gate
            .holder
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        let held = self.since.elapsed();
        if held >= STALL_THRESHOLD {
            let operation = self.operation;
            eprintln!(
                "database write `{operation}` held the write gate for {} ms",
                held.as_millis()
            );
            artisan_tracing::instant!("database", "write_gate.held", "operation" => operation, "held_ms" => held.as_millis());
        }
    }
}
