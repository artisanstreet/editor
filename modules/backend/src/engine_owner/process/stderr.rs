//! Counted stderr draining with a start-only diagnostic tail.
//!
//! Bytes are counted toward the caller-supplied cap exactly as before. While
//! the child is starting, the last [`STDERR_TAIL_BYTES`] of the counted bytes
//! (line-bounded) are also kept in memory so a failed start can report the
//! engine's own reason. The tail is released and zeroized as soon as the
//! start is announced ([`StderrCounter::release_diagnostics`]) and is only
//! ever read through the sanitizer ([`StderrCounter::start_diagnostic`]);
//! nothing is printed, logged, or retained for a run that started.

use std::time::Duration;

use tokio::process::ChildStderr;
use zeroize::Zeroize as _;

use super::diagnostic::StartDiagnostic;

/// Bytes of stderr retained for start diagnostics.
pub(crate) const STDERR_TAIL_BYTES: usize = 4096;
/// Upper bound on draining the remaining stderr of a failed start.
pub(crate) const START_DIAGNOSTIC_DRAIN: Duration = Duration::from_millis(250);

/// Counted stderr draining state.
///
/// Bytes are counted toward the caller-supplied `stderr_cap_bytes`; beyond
/// the bounded start tail nothing is retained, printed, or formatted.
/// Terminal states stop further reads but never discard the pipe handle.
pub(crate) struct StderrCounter {
    stderr: Option<ChildStderr>,
    counted: usize,
    cap: usize,
    state: StderrState,
    tail: Option<DiagnosticTail>,
}

/// Current counting state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StderrState {
    /// The stream is open and within the cap.
    Open,
    /// The cap was crossed; reads stopped while the handle stays retained.
    Capped,
    /// Clean end of stream within the cap.
    ClosedWithinCap,
    /// The operating-system read failed.
    Failed,
}

/// What one stderr pump step observed.
pub(crate) enum StderrEvent {
    /// More bytes were counted; the stream stays open within the cap.
    WithinCap,
    /// The cap was crossed; counting stopped permanently.
    CapExceeded,
    /// Clean end of stream within the cap.
    Closed,
    /// The operating-system read failed.
    ReadFailed,
}

impl StderrCounter {
    /// Wraps the child's piped stderr with the caller-supplied cap and an
    /// armed start-diagnostic tail.
    pub(crate) fn new(stderr: Option<ChildStderr>, cap: usize) -> Self {
        Self {
            stderr,
            counted: 0,
            cap,
            state: StderrState::Open,
            tail: Some(DiagnosticTail::default()),
        }
    }

    /// Current counting state.
    #[must_use]
    pub(crate) fn state(&self) -> StderrState {
        self.state
    }

    /// Drops (and zeroizes) the start tail; later bytes are only counted.
    ///
    /// Called once the child has announced its session or readiness: a
    /// started run never retains stderr content.
    pub(crate) fn release_diagnostics(&mut self) {
        self.tail = None;
    }

    /// Whether a start tail is still retained.
    #[cfg(test)]
    pub(crate) fn retains_diagnostics(&self) -> bool {
        self.tail.is_some()
    }

    /// Drains what the failed child already wrote (bounded by `budget`),
    /// then reduces the retained tail to one sanitized diagnostic.
    ///
    /// The tail is consumed either way; a released tail yields `None`.
    pub(crate) async fn start_diagnostic(&mut self, budget: Duration) -> Option<StartDiagnostic> {
        self.tail.as_ref()?;
        let _ = tokio::time::timeout(budget, async {
            while self.state == StderrState::Open {
                let _ = self.pump().await;
            }
        })
        .await;
        let tail = self.tail.take()?;
        StartDiagnostic::from_stderr_tail(tail.bytes())
    }

    /// Performs at most one bounded counting read.
    ///
    /// Bytes beyond the cap are counted and discarded. Returns whether the
    /// cap was crossed or the stream closed.
    pub(crate) async fn pump(&mut self) -> StderrEvent {
        use tokio::io::AsyncReadExt as _;
        match self.state {
            StderrState::Capped => return StderrEvent::CapExceeded,
            StderrState::ClosedWithinCap => return StderrEvent::Closed,
            StderrState::Failed => return StderrEvent::ReadFailed,
            StderrState::Open => {}
        }
        let Some(stderr) = self.stderr.as_mut() else {
            self.state = StderrState::Failed;
            return StderrEvent::ReadFailed;
        };
        let mut buf = [0_u8; 512];
        let window = self.cap.saturating_sub(self.counted).saturating_add(1);
        let window = window.min(buf.len());
        let event = match stderr.read(&mut buf[..window]).await {
            Ok(0) => {
                self.state = StderrState::ClosedWithinCap;
                StderrEvent::Closed
            }
            Ok(count) => {
                self.counted += count;
                if let Some(tail) = self.tail.as_mut() {
                    tail.push(&buf[..count]);
                }
                if self.counted > self.cap {
                    self.state = StderrState::Capped;
                    StderrEvent::CapExceeded
                } else {
                    StderrEvent::WithinCap
                }
            }
            Err(_) => {
                self.state = StderrState::Failed;
                StderrEvent::ReadFailed
            }
        };
        buf.zeroize();
        event
    }
}

/// The last [`STDERR_TAIL_BYTES`] of a starting child's stderr, cut at a
/// line boundary once it has overflowed; zeroized when dropped.
///
/// Capacity covers the tail plus one pump read, so the buffer never
/// reallocates and leaves no unzeroized copy behind.
struct DiagnosticTail {
    bytes: Vec<u8>,
}

impl Default for DiagnosticTail {
    fn default() -> Self {
        Self {
            bytes: Vec::with_capacity(STDERR_TAIL_BYTES + 512),
        }
    }
}

impl DiagnosticTail {
    fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend_from_slice(chunk);
        if self.bytes.len() <= STDERR_TAIL_BYTES {
            return;
        }
        let excess = self.bytes.len() - STDERR_TAIL_BYTES;
        // Drop the overflow, then the partial line it leaves behind.
        let cut = self.bytes[excess..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(excess, |newline| excess + newline + 1);
        self.bytes.copy_within(cut.., 0);
        let kept = self.bytes.len() - cut;
        self.bytes[kept..].zeroize();
        self.bytes.truncate(kept);
    }

    fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl Drop for DiagnosticTail {
    fn drop(&mut self) {
        self.bytes.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_keeps_the_last_bytes_at_a_line_boundary() {
        let mut tail = DiagnosticTail::default();
        let noise = "noise line\n".repeat(STDERR_TAIL_BYTES / 11 + 5);
        tail.push(noise.as_bytes());
        tail.push(b"Error: the real reason\n");
        assert!(tail.bytes().len() <= STDERR_TAIL_BYTES);
        assert!(tail.bytes().starts_with(b"noise line\n"));
        assert!(tail.bytes().ends_with(b"Error: the real reason\n"));
        assert_eq!(
            StartDiagnostic::from_stderr_tail(tail.bytes()).map(StartDiagnostic::into_string),
            Some("the real reason".to_owned())
        );
    }

    #[test]
    fn tail_without_newlines_keeps_the_newest_bytes() {
        let mut tail = DiagnosticTail::default();
        tail.push(&vec![b'a'; STDERR_TAIL_BYTES]);
        tail.push(b"zz");
        assert_eq!(tail.bytes().len(), STDERR_TAIL_BYTES);
        assert!(tail.bytes().ends_with(b"azz"));
    }
}
