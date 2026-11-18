use std::collections::HashMap;

use crate::{
    BodyDecodeError, REASSEMBLY_MAX_BYTES_PER_CONN, REASSEMBLY_MAX_IN_FLIGHT, TRANSFER_MAX_BYTES,
    TRANSFER_MAX_PARTS, TunnelChunk,
};

#[derive(Debug, thiserror::Error)]
pub enum ReassemblyError {
    #[error(
        "chunk {index} of {total} in transfer {transfer} on channel {channel} declares {total_bytes} bytes, outside the framing limits"
    )]
    Framing {
        channel: String,
        transfer: String,
        index: u32,
        total: u32,
        total_bytes: u64,
    },
    #[error(
        "transfer {transfer} on channel {channel} would exceed {REASSEMBLY_MAX_IN_FLIGHT} transfers in flight"
    )]
    TooManyInFlight { channel: String, transfer: String },
    #[error(
        "chunk {index} of transfer {transfer} on channel {channel} disagrees with the transfer's size"
    )]
    Mismatch {
        channel: String,
        transfer: String,
        index: u32,
    },
    #[error("chunk {index} of transfer {transfer} on channel {channel} arrived twice")]
    Duplicate {
        channel: String,
        transfer: String,
        index: u32,
    },
    #[error("decode chunk {index} of transfer {transfer} on channel {channel}")]
    Decode {
        channel: String,
        transfer: String,
        index: u32,
        #[source]
        source: BodyDecodeError,
    },
    #[error(
        "chunk {index} of transfer {transfer} on channel {channel} exceeds the reassembly budget"
    )]
    OverBudget {
        channel: String,
        transfer: String,
        index: u32,
    },
    #[error(
        "transfer {transfer} on channel {channel} reassembled {received} bytes, not the {expected} it declared"
    )]
    LengthMismatch {
        channel: String,
        transfer: String,
        expected: u64,
        received: u64,
    },
}

#[derive(Debug)]
struct InProgress {
    total_bytes: u64,
    parts: Vec<Option<Vec<u8>>>,
    filled: usize,
    received_bytes: u64,
}

#[derive(Debug, Default)]
pub struct Reassembly {
    transfers: HashMap<(String, String), InProgress>,
    total_bytes: u64,
}

impl Reassembly {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn feed(
        &mut self,
        channel: &str,
        chunk: &TunnelChunk,
    ) -> Result<Option<Vec<u8>>, ReassemblyError> {
        let index = chunk.index;
        let named = || (channel.to_owned(), chunk.transfer.clone());
        if chunk.total == 0
            || chunk.total > TRANSFER_MAX_PARTS
            || chunk.index >= chunk.total
            || chunk.total_bytes > TRANSFER_MAX_BYTES
        {
            let (channel, transfer) = named();
            return Err(ReassemblyError::Framing {
                channel,
                transfer,
                index,
                total: chunk.total,
                total_bytes: chunk.total_bytes,
            });
        }
        let key = named();
        if !self.transfers.contains_key(&key) && self.transfers.len() >= REASSEMBLY_MAX_IN_FLIGHT {
            let (channel, transfer) = key;
            return Err(ReassemblyError::TooManyInFlight { channel, transfer });
        }
        let entry = self
            .transfers
            .entry(key.clone())
            .or_insert_with(|| InProgress {
                total_bytes: chunk.total_bytes,
                parts: vec![None; chunk.total as usize],
                filled: 0,
                received_bytes: 0,
            });
        if entry.parts.len() != chunk.total as usize || entry.total_bytes != chunk.total_bytes {
            let (channel, transfer) = key;
            return Err(ReassemblyError::Mismatch {
                channel,
                transfer,
                index,
            });
        }
        let slot = &mut entry.parts[index as usize];
        if slot.is_some() {
            let (channel, transfer) = key;
            return Err(ReassemblyError::Duplicate {
                channel,
                transfer,
                index,
            });
        }
        let bytes = match crate::decode_chunk_bytes(chunk) {
            Ok(bytes) => bytes,
            Err(source) => {
                let (channel, transfer) = key;
                return Err(ReassemblyError::Decode {
                    channel,
                    transfer,
                    index,
                    source,
                });
            }
        };
        let length = bytes.len() as u64;
        let received_bytes = entry.received_bytes.saturating_add(length);
        let total_bytes = self.total_bytes.saturating_add(length);
        if received_bytes > entry.total_bytes || total_bytes > REASSEMBLY_MAX_BYTES_PER_CONN {
            let (channel, transfer) = key;
            return Err(ReassemblyError::OverBudget {
                channel,
                transfer,
                index,
            });
        }
        *slot = Some(bytes);
        entry.filled += 1;
        entry.received_bytes = received_bytes;
        self.total_bytes = total_bytes;
        if entry.filled < entry.parts.len() {
            return Ok(None);
        }
        let finished = self
            .transfers
            .remove(&key)
            .expect("invariant: the transfer being completed is in flight");
        self.total_bytes = self.total_bytes.saturating_sub(finished.received_bytes);
        if finished.received_bytes != finished.total_bytes {
            let (channel, transfer) = key;
            return Err(ReassemblyError::LengthMismatch {
                channel,
                transfer,
                expected: finished.total_bytes,
                received: finished.received_bytes,
            });
        }
        let mut out = Vec::with_capacity(finished.parts.iter().flatten().map(Vec::len).sum());
        for part in finished.parts.into_iter().flatten() {
            out.extend_from_slice(&part);
        }
        Ok(Some(out))
    }

    pub fn drop_channel(&mut self, channel: &str) {
        self.transfers.retain(|(owner, _), _| owner != channel);
        self.total_bytes = self
            .transfers
            .values()
            .map(|transfer| transfer.received_bytes)
            .fold(0, u64::saturating_add);
    }
}

#[cfg(test)]
#[path = "../tests/api/reassembly.rs"]
mod tests;
