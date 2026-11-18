use super::*;
use crate::CHUNK_RAW_BYTES;

fn feed_all(reassembly: &mut Reassembly, channel: &str, bytes: &[u8]) -> Vec<u8> {
    let chunks = crate::chunk_bytes("t-same-name", bytes);
    assert!(chunks.len() > 1);
    let mut done = None;
    for chunk in &chunks {
        let out = reassembly.feed(channel, chunk).expect("feed a valid chunk");
        if out.is_some() {
            assert!(done.is_none(), "transfer completed twice");
            done = out;
        }
    }
    done.expect("transfer never completed")
}

fn empty_chunk(transfer: &str, index: u32, total: u32, total_bytes: u64) -> TunnelChunk {
    TunnelChunk {
        transfer: transfer.to_owned(),
        index,
        total,
        total_bytes,
        bytes_b64u: String::new(),
    }
}

#[test]
fn sequential_large_responses_sharing_a_transfer_name_both_complete() {
    let mut reassembly = Reassembly::new();
    let first: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let second: Vec<u8> = (0..200_000u32).map(|i| (i * 7 % 251) as u8).collect();
    assert_eq!(feed_all(&mut reassembly, "unary", &first), first);
    assert_eq!(feed_all(&mut reassembly, "unary", &second), second);
}

#[test]
fn repeated_stream_chunks_with_one_transfer_name_complete() {
    let mut reassembly = Reassembly::new();
    for round in 0..3u8 {
        let bytes: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8 ^ round).collect();
        assert_eq!(feed_all(&mut reassembly, "events", &bytes), bytes);
    }
}

#[test]
fn chunks_arriving_out_of_order_complete_once_every_part_is_in() {
    let bytes: Vec<u8> = (0..150_000u32).map(|i| (i % 253) as u8).collect();
    let chunks = crate::chunk_bytes("shuffled", &bytes);
    let mut reassembly = Reassembly::new();
    let (last, rest) = chunks.split_first().expect("several chunks");
    for chunk in rest.iter().rev() {
        assert_eq!(reassembly.feed("unary", chunk).expect("feed"), None);
    }
    assert_eq!(reassembly.feed("unary", last).expect("feed"), Some(bytes));
    assert_eq!(
        reassembly
            .feed("unary", &chunks[1])
            .expect("a new transfer starts clean"),
        None
    );
}

#[test]
fn a_repeated_chunk_is_rejected_without_completing_the_transfer() {
    let bytes: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    let chunks = crate::chunk_bytes("twice", &bytes);
    let mut reassembly = Reassembly::new();
    reassembly.feed("unary", &chunks[0]).expect("first part");
    assert!(matches!(
        reassembly.feed("unary", &chunks[0]),
        Err(ReassemblyError::Duplicate { index: 0, ref transfer, .. }) if transfer == "twice"
    ));
    for chunk in &chunks[1..chunks.len() - 1] {
        assert_eq!(reassembly.feed("unary", chunk).expect("feed"), None);
    }
    assert_eq!(
        reassembly
            .feed("unary", &chunks[chunks.len() - 1])
            .expect("last part"),
        Some(bytes)
    );
}

#[test]
fn corrupt_part_count_is_rejected_before_allocation() {
    let mut reassembly = Reassembly::new();
    assert!(matches!(
        reassembly.feed("unary", &empty_chunk("evil", 0, u32::MAX, 1)),
        Err(ReassemblyError::Framing { .. })
    ));
    assert_eq!(
        u64::from(TRANSFER_MAX_PARTS),
        TRANSFER_MAX_BYTES.div_ceil(CHUNK_RAW_BYTES as u64)
    );
    assert!(matches!(
        reassembly.feed("unary", &empty_chunk("evil", 0, TRANSFER_MAX_PARTS + 1, 1)),
        Err(ReassemblyError::Framing { .. })
    ));
}

#[test]
fn in_flight_transfer_count_is_bounded() {
    let mut reassembly = Reassembly::new();
    for n in 0..REASSEMBLY_MAX_IN_FLIGHT {
        assert_eq!(
            reassembly
                .feed("unary", &empty_chunk(&format!("t{n}"), 0, 2, 2))
                .expect("room for another transfer"),
            None
        );
    }
    assert!(matches!(
        reassembly.feed("unary", &empty_chunk("one-too-many", 0, 2, 2)),
        Err(ReassemblyError::TooManyInFlight { .. })
    ));
    reassembly.drop_channel("unary");
    assert_eq!(
        reassembly
            .feed("unary", &empty_chunk("after-close", 0, 2, 2))
            .expect("a closed channel frees its transfers"),
        None
    );
}

#[test]
fn the_largest_blob_upload_fits_one_tunnel_transfer_once_encoded() {
    use base64::Engine as _;
    let blob: Vec<u8> = (0..crate::TUNNEL_BODY_MAX_BYTES)
        .map(|index| u8::try_from(index % 251).unwrap_or(0))
        .collect();
    let request = crate::TunnelRequest {
        id: "upload".into(),
        method: "PUT".into(),
        path: "/blobs".into(),
        body: Some(serde_json::json!({"$bytes": {
            "content_type": "image/webp",
            "b64u": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&blob),
        }})),
    };
    let message = serde_json::to_vec(&request).expect("encode the tunneled upload");
    let mut reassembly = Reassembly::new();
    let delivered = feed_all(&mut reassembly, "unary", &message);
    let delivered: crate::TunnelRequest =
        serde_json::from_slice(&delivered).expect("decode the tunneled upload");
    let (_, body) = crate::request_body_bytes(delivered.body.as_ref()).expect("decode the blob");
    assert_eq!(body, blob);
}
