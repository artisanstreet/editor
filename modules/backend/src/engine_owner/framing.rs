//! Bounded incremental SSE framer.
//!
//! Byte-oriented, per-connection framer suitable for later transport wiring.
//! Accepts arbitrarily fragmented byte chunks without converting incomplete
//! UTF-8, recognizes LF lines with one optional CR trimmed, splits fields at
//! the first colon with at most one leading ASCII space stripped, ignores
//! comment and unknown fields, implements `event`/`data`/`id`, joins multiple
//! `data` lines with `\n`, dispatches on blank lines, defaults event to
//! `message`, preserves `id` without inventing cursors, enforces
//! `max_sse_line` on pending-line bytes and `max_sse_event` on current event
//! accumulation with checked arithmetic, validates UTF-8 only after a complete
//! bounded line, and provides explicit EOF finalization that never equates EOF
//! with a terminal provider observation. Typed errors are payload-free.

use thiserror::Error;

/// Typed, payload-free failure of SSE framing.
///
/// `Debug` and `Display` are constant strings; no frame, prompt,
/// authorization, secret, or body bytes are embedded.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub(crate) enum SseFramerError {
    #[error("sse line exceeded the configured limit")]
    LineTooLong,
    #[error("sse event exceeded the configured limit")]
    EventTooLarge,
    #[error("sse bytes were not valid utf-8")]
    InvalidUtf8,
    #[error("sse cap overflowed")]
    UnrepresentableCap,
    #[error("sse framer is poisoned")]
    Poisoned,
}

/// One dispatched SSE event.
///
/// Data is the `data:` payload joined with `\n` per SSE. Event defaults to
/// `message` when not supplied. Id is preserved verbatim when supplied.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SseEvent {
    event: String,
    data: String,
    id: Option<String>,
}

impl SseEvent {
    #[must_use]
    pub(crate) fn event(&self) -> &str {
        &self.event
    }

    #[must_use]
    pub(crate) fn data(&self) -> &str {
        &self.data
    }

    #[must_use]
    pub(crate) fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }
}

/// Explicit EOF result that never equates to a terminal observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SseEof {
    Clean,
}

/// Incremental byte-oriented SSE framer with bounded accumulation.
///
/// Terminal/poisoned after the first hard error; further calls return
/// `Poisoned` deterministically.
pub(crate) struct SseFramer {
    max_line: usize,
    max_event: usize,
    pending: Vec<u8>,
    data: Option<String>,
    data_len: usize,
    event_name: Option<String>,
    event_name_len: usize,
    id: Option<String>,
    id_len: usize,
    poisoned: bool,
    finished: bool,
}

impl SseFramer {
    /// Creates a new framer bounded by `max_line` and `max_event`.
    ///
    /// Both bounds must be `>0` and `checked_add(1)` must not overflow;
    /// otherwise `UnrepresentableCap` is returned without allocating.
    pub(crate) fn new(max_line: usize, max_event: usize) -> Result<Self, SseFramerError> {
        if max_line == 0 || max_event == 0 {
            return Err(SseFramerError::UnrepresentableCap);
        }
        if max_line.checked_add(1).is_none() || max_event.checked_add(1).is_none() {
            return Err(SseFramerError::UnrepresentableCap);
        }
        Ok(Self {
            max_line,
            max_event,
            pending: Vec::new(),
            data: None,
            data_len: 0,
            event_name: None,
            event_name_len: 0,
            id: None,
            id_len: 0,
            poisoned: false,
            finished: false,
        })
    }

    /// Feeds an arbitrarily fragmented chunk, returning complete events.
    ///
    /// Does not build an unbounded temporary buffer from the chunk; each line
    /// is bounded by `max_line` and each event accumulation by `max_event`
    /// before allocation/append with checked arithmetic.
    pub(crate) fn feed(&mut self, chunk: &[u8]) -> Result<Vec<SseEvent>, SseFramerError> {
        if self.poisoned {
            return Err(SseFramerError::Poisoned);
        }
        if self.finished {
            self.poisoned = true;
            return Err(SseFramerError::Poisoned);
        }
        if self.max_line.checked_add(1).is_none() || self.max_event.checked_add(1).is_none() {
            self.poisoned = true;
            return Err(SseFramerError::UnrepresentableCap);
        }
        let mut out = Vec::new();
        let mut cursor = 0;
        while cursor < chunk.len() {
            if let Some(rel) = chunk[cursor..].iter().position(|&b| b == b'\n') {
                let line_len = rel; // bytes before LF in this segment
                let total = self
                    .pending
                    .len()
                    .checked_add(line_len)
                    .ok_or(SseFramerError::UnrepresentableCap)?;
                if total > self.max_line {
                    self.poisoned = true;
                    return Err(SseFramerError::LineTooLong);
                }
                let mut line_bytes = Vec::with_capacity(total);
                line_bytes.extend_from_slice(&self.pending);
                line_bytes.extend_from_slice(&chunk[cursor..cursor + rel]);
                self.pending.clear();
                if line_bytes.last() == Some(&b'\r') {
                    line_bytes.pop();
                }
                let Ok(line_str) = std::str::from_utf8(&line_bytes) else {
                    self.poisoned = true;
                    return Err(SseFramerError::InvalidUtf8);
                };
                let line_str = line_str.to_owned();
                match self.process_line(&line_str) {
                    Ok(Some(ev)) => out.push(ev),
                    Ok(None) => {}
                    Err(e) => {
                        self.poisoned = true;
                        return Err(e);
                    }
                }
                cursor += rel + 1;
            } else {
                let remaining = chunk.len() - cursor;
                let total = self
                    .pending
                    .len()
                    .checked_add(remaining)
                    .ok_or(SseFramerError::UnrepresentableCap)?;
                if total > self.max_line {
                    self.poisoned = true;
                    return Err(SseFramerError::LineTooLong);
                }
                self.pending.extend_from_slice(&chunk[cursor..]);
                break;
            }
        }
        Ok(out)
    }

    /// Explicit EOF finalization.
    ///
    /// Discards any incomplete pending line without dispatching a data-bearing
    /// event. Never produces a terminal provider observation; returns `Clean`
    /// when the framer was not poisoned. After `finish`, further `feed` calls
    /// are poisoned.
    pub(crate) fn finish(&mut self) -> Result<SseEof, SseFramerError> {
        if self.poisoned {
            return Err(SseFramerError::Poisoned);
        }
        if self.finished {
            return Err(SseFramerError::Poisoned);
        }
        // Incomplete pending line is discarded, not dispatched.
        self.pending.clear();
        // Do not dispatch pending data without a blank-line terminator.
        // Reset per-event state to avoid leaking across connections.
        self.data = None;
        self.data_len = 0;
        self.event_name = None;
        self.event_name_len = 0;
        self.id = None;
        self.id_len = 0;
        self.finished = true;
        Ok(SseEof::Clean)
    }

    fn process_line(&mut self, line: &str) -> Result<Option<SseEvent>, SseFramerError> {
        if line.is_empty() {
            // Blank line dispatches complete data-bearing events and resets per-event state.
            if let Some(data) = self.data.take() {
                let event = self
                    .event_name
                    .take()
                    .filter(|name| !name.is_empty())
                    .unwrap_or_else(|| "message".to_owned());
                let id = self.id.take();
                self.data_len = 0;
                self.event_name_len = 0;
                self.id_len = 0;
                let ev = SseEvent { event, data, id };
                return Ok(Some(ev));
            }
            // No data: blank line just resets per-event state (event and id cleared to avoid leaking).
            self.event_name = None;
            self.event_name_len = 0;
            self.id = None;
            self.id_len = 0;
            self.data_len = 0;
            return Ok(None);
        }
        if line.starts_with(':') {
            // Comment line, ignored.
            return Ok(None);
        }
        let (field, mut value) = if let Some(pos) = line.find(':') {
            let (f, rest) = line.split_at(pos);
            let v = &rest[1..];
            (f, v)
        } else {
            (line, "")
        };
        if value.starts_with(' ') {
            value = &value[1..];
        }
        match field {
            "event" => {
                let new_len = value.len();
                let total = self
                    .data_len
                    .checked_add(new_len)
                    .and_then(|v| v.checked_add(self.id_len))
                    .ok_or(SseFramerError::UnrepresentableCap)?;
                if total > self.max_event {
                    return Err(SseFramerError::EventTooLarge);
                }
                self.event_name = Some(value.to_owned());
                self.event_name_len = new_len;
                Ok(None)
            }
            "data" => {
                let new_data_len = if self.data.is_none() {
                    value.len()
                } else {
                    self.data_len
                        .checked_add(1)
                        .and_then(|v| v.checked_add(value.len()))
                        .ok_or(SseFramerError::UnrepresentableCap)?
                };
                let total = new_data_len
                    .checked_add(self.event_name_len)
                    .and_then(|v| v.checked_add(self.id_len))
                    .ok_or(SseFramerError::UnrepresentableCap)?;
                if total > self.max_event {
                    return Err(SseFramerError::EventTooLarge);
                }
                if let Some(existing) = self.data.as_mut() {
                    existing.push('\n');
                    existing.push_str(value);
                } else {
                    self.data = Some(value.to_owned());
                }
                self.data_len = new_data_len;
                Ok(None)
            }
            "id" => {
                let new_len = value.len();
                let total = self
                    .data_len
                    .checked_add(new_len)
                    .and_then(|v| v.checked_add(self.event_name_len))
                    .ok_or(SseFramerError::UnrepresentableCap)?;
                if total > self.max_event {
                    return Err(SseFramerError::EventTooLarge);
                }
                self.id = Some(value.to_owned());
                self.id_len = new_len;
                Ok(None)
            }
            _ => Ok(None),
        }
    }
}

impl std::fmt::Debug for SseFramer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SseFramer")
            .field("max_line", &self.max_line)
            .field("max_event", &self.max_event)
            .field("poisoned", &self.poisoned)
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}
