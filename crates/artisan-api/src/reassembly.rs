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
    /// complete. `channel` scopes the transfer namespace.
    pub fn feed(
        &mut self,
        channel: &str,
        chunk: &crate::TunnelChunk,
    ) -> Result<Option<Vec<u8>>> {
        use crate::{REASSEMBLY_MAX_BYTES_PER_CONN, TRANSFER_MAX_BYTES};
        if chunk.total == 0
            || chunk.index >= chunk.total
            || chunk.total_bytes > TRANSFER_MAX_BYTES
        {
            anyhow::bail!("bad chunk framing");
        }
        let key = format!("{channel}\0{}", chunk.transfer);
        let entry = self.transfers.entry(key).or_insert_with(|| InProgress {
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
        if out.len() as u64 != entry.total_bytes {
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
