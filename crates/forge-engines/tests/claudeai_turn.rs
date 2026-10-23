//! The claude.ai turn, as it came off the wire.
//!
//! A recorded turn, so the decoder's shape is pinned: the text is written
//! out with the message, the provider's recap is published whole, and a
//! tool the provider ran itself is not the caller's to run.

use forge_engines::claudeai::ClaudeAiDecoder;
use forge_engines::{EngineEvent, LiveKind};

const fn turn() -> &'static str {
    include_str!("../transcripts/claudeai/turn.jsonl")
}

#[test]
fn the_text_is_written_out_with_its_message() {
    let mut decoder = ClaudeAiDecoder::default();
    let mut out = Vec::new();
    for line in turn().lines() {
        let value: serde_json::Value = serde_json::from_str(line).expect("a frame");
        decoder.feed(&value["event"], &mut out);
    }
    let texts: Vec<String> = out
        .iter()
        .filter_map(|event| {
            if let EngineEvent::Item {
                kind: artisan_api::ItemKind::AssistantMessage { text, .. },
                status: artisan_api::Status::Done,
                ..
            } = event
            {
                Some(text.inline().unwrap_or_default().to_owned())
            } else {
                None
            }
        })
        .collect();
    assert!(
        !texts.is_empty(),
        "the text is stored with the message, not left to the live preview"
    );
    for text in &texts {
        assert!(!text.is_empty(), "a message is not stored empty: {text:?}");
    }
}

#[test]
fn the_providers_recap_is_published_whole() {
    let mut decoder = ClaudeAiDecoder::default();
    let mut out = Vec::new();
    for line in turn().lines() {
        let value: serde_json::Value = serde_json::from_str(line).expect("a frame");
        decoder.feed(&value["event"], &mut out);
    }
    let recaps: Vec<String> = out
        .iter()
        .filter_map(|event| {
            if let EngineEvent::Live {
                target: LiveKind::Narration,
                text,
                ..
            } = event
            {
                Some(text.clone())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        recaps,
        vec![
            "Figuring out which directory the request refers to".to_owned(),
            "Exploring the root directory's contents".to_owned(),
        ],
        "the provider's recap reaches the run's line, whole, less its closing full stop"
    );
    for recap in &recaps {
        assert!(
            recap.chars().count() > 20,
            "a recap is a sentence, not a clause: {recap}"
        );
    }
}
