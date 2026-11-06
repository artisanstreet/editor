use editor_aqua::ui::rich::AudioFixture;
use editor_audio::{PlaybackState, Snapshot};
use std::path::Path;
use std::time::Duration;

fn snapshot(state: PlaybackState) -> Snapshot {
    Snapshot {
        track: Some("track.wav".into()),
        state,
        position: Duration::from_secs(75),
        duration: Some(Duration::from_secs(100)),
    }
}

#[test]
fn shared_player_renders_only_its_track_and_clamps_position() {
    let mut value = snapshot(PlaybackState::Playing);
    assert_eq!(
        AudioFixture::shown(&value, Path::new("other.wav")),
        ("idle".into(), 0.0, None)
    );
    assert_eq!(
        AudioFixture::shown(&value, Path::new("track.wav")),
        ("playing".into(), 0.75, Some("1:15 / 1:40".into()))
    );
    value.position = Duration::from_secs(200);
    assert_eq!(
        AudioFixture::shown(&value, Path::new("track.wav")),
        ("playing".into(), 1.0, Some("1:40 / 1:40".into()))
    );
}
#[test]
fn loading_ended_and_missing_duration_keep_source_time_semantics() {
    let mut value = snapshot(PlaybackState::Loading);
    assert_eq!(
        AudioFixture::shown(&value, Path::new("track.wav")),
        ("loading".into(), 0.0, Some("Loading…".into()))
    );
    value.state = PlaybackState::Ended;
    value.duration = Some(Duration::from_secs(3725));
    assert_eq!(
        AudioFixture::shown(&value, Path::new("track.wav")),
        ("ended".into(), 1.0, Some("1:02:05".into()))
    );
    value.state = PlaybackState::Paused;
    value.duration = None;
    assert_eq!(
        AudioFixture::shown(&value, Path::new("track.wav")),
        ("paused".into(), 0.0, Some("0:00 / 0:00".into()))
    );
}
#[test]
fn data_identity_uses_payload_bytes_and_rejects_remote_url() {
    let first = AudioFixture::data_track("data:audio/wav;base64,SGVsbG8=").unwrap();
    assert_eq!(
        Some(first.clone()),
        AudioFixture::data_track("data:audio/wav;base64,SGVsbG8=")
    );
    assert_ne!(
        Some(first),
        AudioFixture::data_track("data:audio/wav;base64,V29ybGQ=")
    );
    assert_eq!(
        AudioFixture::data_track("https://example.invalid/a.wav"),
        None
    );
}
