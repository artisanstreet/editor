//! Bounded chunk reassembly shared by tunnel endpoints.
//!
//! Both directions chunk large payloads into [`TunnelChunk`] frames
//! (cap 60 KiB Noise plaintext each). Reassembly is bounded per transfer
//! AND per connection, so base64 expansion can never grow reception
//! without limit. Fail-closed on framing violations and budget overruns.

use anyhow::{Context, Result};
use std::collections::HashMap;

/// One in-progress transfer.
struct InProgress {
    total: u32,
    total_bytes: u64,
    parts: Vec<Option<Vec<u8>>>,
    received_bytes: u64,
}

/// Bounded reassembly for one connection (or one channel; the caller keys
/// transfers accordingly).
#[derive(Default)]
pub struct Reassembly {
    transfers: HashMap<String, InProgress>,
    total_bytes: u64,
}

impl Reassembly {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one chunk; returns reassembled bytes when the transfer is
    /// complete. `channel` scopes the transfer namespace. Completed
    /// transfers are removed and their budget released, so a later
    /// transfer reusing the same name starts clean instead of colliding
    /// with drained state.
    pub fn feed(
        &mut self,
        channel: &str,
        chunk: &crate::TunnelChunk,
    ) -> Result<Option<Vec<u8>>> {
        use crate::{
            REASSEMBLY_MAX_BYTES_PER_CONN, REASSEMBLY_MAX_IN_FLIGHT, TRANSFER_MAX_BYTES,
            TRANSFER_MAX_PARTS,
        };
        if chunk.total == 0
            || chunk.total > TRANSFER_MAX_PARTS
            || chunk.index >= chunk.total
            || chunk.total_bytes > TRANSFER_MAX_BYTES
        {
            anyhow::bail!("bad chunk framing");
        }
        let key = format!("{channel}\0{}", chunk.transfer);
        if !self.transfers.contains_key(&key)
            && self.transfers.len() >= REASSEMBLY_MAX_IN_FLIGHT
        {
            anyhow::bail!("too many in-flight chunk transfers");
        }
        let entry = self.transfers.entry(key.clone()).or_insert_with(|| InProgress {
            total: chunk.total,
            total_bytes: chunk.total_bytes,
            parts: vec![None; chunk.total as usize],
            received_bytes: 0,
        });
        if entry.total != chunk.total || entry.total_bytes != chunk.total_bytes {
            anyhow::bail!("chunk transfer mismatch");
        }
        let slot = entry
            .parts
            .get_mut(chunk.index as usize)
            .context("bad chunk index")?;
        if slot.is_some() {
            anyhow::bail!("duplicate chunk");
        }
        let bytes = crate::decode_chunk_bytes(chunk).context("bad chunk bytes")?;
        entry.received_bytes = entry.received_bytes.saturating_add(bytes.len() as u64);
        self.total_bytes = self.total_bytes.saturating_add(bytes.len() as u64);
        if entry.received_bytes > entry.total_bytes
            || self.total_bytes > REASSEMBLY_MAX_BYTES_PER_CONN
        {
            anyhow::bail!("chunk transfer over budget");
        }
        *slot = Some(bytes);
        if entry.parts.iter().any(Option::is_none) {
            return Ok(None);
        }
        let mut out = Vec::with_capacity(entry.total_bytes as usize);
        for part in entry.parts.drain(..).flatten() {
            out.extend_from_slice(&part);
        }
        // The entry is drained: remove it and release its budget whether or
        // not the length check passes, so no poisoned state lingers.
        let expected = entry.total_bytes;
        let finished = entry.received_bytes;
        self.transfers.remove(&key);
        self.total_bytes = self.total_bytes.saturating_sub(finished);
        if out.len() as u64 != expected {
            anyhow::bail!("chunk transfer length mismatch");
        }
        Ok(Some(out))
    }

    /// Drop every transfer for a closed channel.
    pub fn drop_channel(&mut self, channel: &str) {
        self.transfers
            .retain(|key, _| !key.starts_with(channel) || key.as_bytes().get(channel.len()) != Some(&0));
        let mut total = 0_u64;
        for transfer in self.transfers.values() {
            total = total.saturating_add(transfer.received_bytes);
        }
        self.total_bytes = total;
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "test fixtures; failures fail the test"
)]
mod tests {
    use super::*;
    use crate::{CHUNK_RAW_BYTES, TRANSFER_MAX_PARTS};

    fn feed_all(reassembly: &mut Reassembly, channel: &str, bytes: &[u8]) -> Vec<u8> {
        let chunks = crate::chunk_bytes("t-same-name", bytes);
        assert!(chunks.len() > 1);
        let mut done = None;
        for chunk in &chunks {
            let out = reassembly.feed(channel, chunk).unwrap();
            if out.is_some() {
                assert!(done.is_none(), "transfer completed twice");
                done = out;
            }
        }
        done.expect("transfer never completed")
    }

    #[test]
    fn sequential_large_responses_sharing_a_transfer_name_both_complete() {
        // Mirrors the old constant "{channel}:resp" naming: two large
        // responses in a row on one channel. Without completed-transfer
        // cleanup the second collided with the first's drained state.
        let mut reassembly = Reassembly::new();
        let first: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let second: Vec<u8> = (0..200_000u32).map(|i| (i * 7 % 251) as u8).collect();
        assert_eq!(feed_all(&mut reassembly, "unary", &first), first);
        assert_eq!(feed_all(&mut reassembly, "unary", &second), second);
    }

    #[test]
    fn repeated_stream_chunks_with_one_transfer_name_complete() {
        // Same shape for "{channel}:stream" envelopes: several large
        // envelopes back to back must each reassemble.
        let mut reassembly = Reassembly::new();
        for round in 0..3u8 {
            let bytes: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8 ^ round).collect();
            assert_eq!(feed_all(&mut reassembly, "events", &bytes), bytes);
        }
    }

    #[test]
    fn corrupt_part_count_is_rejected_before_allocation() {
        // total far beyond TRANSFER_MAX_PARTS must fail framing without
        // attempting a giant parts vector.
        let mut reassembly = Reassembly::new();
        let chunk = crate::TunnelChunk {
            transfer: "evil".to_owned(),
            index: 0,
            total: u32::MAX,
            total_bytes: 1,
            bytes_b64u: String::new(),
        };
        assert!(reassembly.feed("unary", &chunk).is_err());
        // A part count just over the cap fails too; the cap itself is
        // reachable by real 32 MiB payloads.
        assert_eq!(
            TRANSFER_MAX_PARTS,
            u32::try_from(crate::TRANSFER_MAX_BYTES.div_ceil(CHUNK_RAW_BYTES as u64)).unwrap()
        );
        let chunk = crate::TunnelChunk {
            transfer: "evil".to_owned(),
            index: 0,
            total: TRANSFER_MAX_PARTS + 1,
            total_bytes: 1,
            bytes_b64u: String::new(),
        };
        assert!(reassembly.feed("unary", &chunk).is_err());
    }

    #[test]
    fn in_flight_transfer_count_is_bounded() {
        let mut reassembly = Reassembly::new();
        for n in 0..crate::REASSEMBLY_MAX_IN_FLIGHT {
            let chunk = crate::TunnelChunk {
                transfer: format!("t{n}"),
                index: 0,
                total: 2,
                total_bytes: 2,
                bytes_b64u: String::new(),
            };
            // total_bytes 2 as one empty part: stays in flight, no completion.
            let _ = reassembly.feed("unary", &chunk);
        }
        let chunk = crate::TunnelChunk {
            transfer: "one-too-many".to_owned(),
            index: 0,
            total: 2,
            total_bytes: 2,
            bytes_b64u: String::new(),
        };
        assert!(reassembly.feed("unary", &chunk).is_err());
    }
}
