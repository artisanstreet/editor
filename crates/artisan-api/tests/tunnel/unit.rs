use super::*;

#[test]
fn chunks_round_trip_and_fit_the_noise_plaintext_cap() {
    let bytes: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let chunks = chunk_bytes("t1", &bytes);
    assert!(chunks.len() > 1);
    for (index, chunk) in chunks.iter().enumerate() {
        assert_eq!(chunk.index as usize, index);
        assert_eq!(chunk.total as usize, chunks.len());
        assert_eq!(chunk.total_bytes, bytes.len() as u64);
        let framed = serde_json::to_vec(chunk).unwrap();
        assert!(
            framed.len() <= TUNNEL_PLAINTEXT_BUDGET,
            "chunk {} serializes to {} bytes, over the Noise cap",
            index,
            framed.len()
        );
        assert!(decode_chunk_bytes(chunk).unwrap().len() <= CHUNK_RAW_BYTES);
    }
    let mut reassembled = Vec::new();
    for chunk in &chunks {
        reassembled.extend_from_slice(&decode_chunk_bytes(chunk).unwrap());
    }
    assert_eq!(reassembled, bytes);
}

#[test]
fn corrupt_envelopes_fail_loudly() {
    let bad = TunnelChunk {
        transfer: "t".into(),
        index: 0,
        total: 1,
        total_bytes: 3,
        bytes_b64u: "!!!not-base64!!!".into(),
    };
    assert!(matches!(
        decode_chunk_bytes(&bad),
        Err(BodyDecodeError::InvalidBase64(_))
    ));
    let body = serde_json::json!({"$bytes": {"content_type": "image/webp", "b64u": "%%%"}});
    assert!(matches!(
        request_body_bytes(Some(&body)),
        Err(BodyDecodeError::InvalidBase64(_))
    ));
    let body = serde_json::json!({"$bytes": {"b64u": "aGk"}});
    assert!(matches!(
        request_body_bytes(Some(&body)),
        Err(BodyDecodeError::MalformedEnvelope(_))
    ));
    let body = serde_json::json!({"$bytes": {"content_type": "image/webp", "b64u": "aGk"}});
    assert_eq!(
        request_body_bytes(Some(&body)).unwrap(),
        ("image/webp".to_owned(), b"hi".to_vec())
    );
    let body = serde_json::json!({"text": "hi"});
    let (content_type, raw) = request_body_bytes(Some(&body)).unwrap();
    assert_eq!(content_type, "application/json");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&raw).unwrap(),
        body
    );
}
