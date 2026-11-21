use test_support::wait::{LIVENESS, wait_for};

use super::*;

#[test]
fn playing_a_missing_file_fails_naming_it() {
    let path = std::env::temp_dir().join("artisan-audio-missing-track.wav");
    let player = Player::new();
    player.play(&path);
    let snapshot = wait_for("the missing file to stop loading", LIVENESS, || {
        let snapshot = player.snapshot();
        (!matches!(snapshot.state, PlaybackState::Loading)).then_some(snapshot)
    });
    assert_eq!(snapshot.track.as_deref(), Some(path.as_path()));
    let PlaybackState::Failed(message) = snapshot.state else {
        panic!("expected a failure, got {:?}", snapshot.state);
    };
    assert!(
        message.starts_with("Couldn't read artisan-audio-missing-track.wav: "),
        "{message}"
    );
}
