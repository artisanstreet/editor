//! Bounded provider line reads.
//!
//! One line-delimited read that enforces the caller's byte cap *while*
//! consuming the stream, so an unterminated or oversized provider line can
//! never allocate beyond the bound. The cap counts the line bytes excluding
//! the terminating LF; the LF is carried in the returned line exactly like
//! [`tokio::io::AsyncBufReadExt::read_line`]. Detection consumes at most the
//! offending chunk, and the typed failure lets the caller settle the turn
//! without resynchronizing on a line it will never trust.

#![forbid(unsafe_code)]

use tokio::io::AsyncBufReadExt as _;

/// Why one bounded line read failed.
///
/// Payload-free: no line bytes, path, or operating-system strings are
/// embedded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BoundedLineError {
    /// The operating-system read failed.
    Io,
    /// The line exceeded the caller's byte cap before its newline.
    LineTooLong,
    /// The line bytes were not valid UTF-8.
    InvalidUtf8,
}

/// Reads one newline-terminated line into `line`, bounded by `max_line`.
///
/// `max_line` bounds the line content in bytes; the terminating LF (and an
/// optional CR) count toward the bound. Returns the number of bytes pushed
/// into `line`, with `Ok(0)` meaning clean end of stream. EOF with a partial
/// line returns that partial content, mirroring `read_line`. The caller's
/// line is cleared before any bytes are read, so a typed failure never leaves
/// stale content behind.
///
/// # Errors
///
/// Returns [`BoundedLineError::LineTooLong`] before the over-cap content is
/// copied, [`BoundedLineError::InvalidUtf8`], or [`BoundedLineError::Io`].
pub(crate) async fn read_bounded_line<R>(
    reader: &mut R,
    line: &mut String,
    max_line: usize,
) -> Result<usize, BoundedLineError>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    line.clear();
    let mut bytes: Vec<u8> = Vec::new();
    loop {
        let available = reader.fill_buf().await.map_err(|_| BoundedLineError::Io)?;
        if available.is_empty() {
            return finish_line(line, &bytes);
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        if let Some(newline) = newline {
            if bytes.len() + newline > max_line {
                reader.consume(newline + 1);
                return Err(BoundedLineError::LineTooLong);
            }
            bytes.extend_from_slice(&available[..=newline]);
            reader.consume(newline + 1);
            return finish_line(line, &bytes);
        }
        if bytes.len() + available.len() > max_line {
            let consumed = available.len();
            reader.consume(consumed);
            return Err(BoundedLineError::LineTooLong);
        }
        bytes.extend_from_slice(available);
        let consumed = available.len();
        reader.consume(consumed);
    }
}

/// Moves the bounded byte buffer into the caller's line as UTF-8.
fn finish_line(line: &mut String, bytes: &[u8]) -> Result<usize, BoundedLineError> {
    let text = std::str::from_utf8(bytes).map_err(|_| BoundedLineError::InvalidUtf8)?;
    line.clear();
    line.push_str(text);
    Ok(line.len())
}
