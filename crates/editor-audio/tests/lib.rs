use std::time::Instant;

use super::*;

#[test]
fn playing_a_missing_file_fails_naming_it() {
    let path = std::env::temp_dir().join("artisan-audio-missing-track.wav");
    let player = Player::new();
    player.play(&path);
    let deadline = Instant::now() + Duration::from_secs(5);
    let snapshot = loop {
        let snapshot = player.snapshot();
        if !matches!(snapshot.state, PlaybackState::Loading) || Instant::now() > deadline {
            break snapshot;
        }
        thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(snapshot.track.as_deref(), Some(path.as_path()));
    let PlaybackState::Failed(message) = snapshot.state else {
        panic!("expected a failure, got {:?}", snapshot.state);
    };
    assert!(
        message.starts_with("Couldn't read artisan-audio-missing-track.wav: "),
        "{message}"
    );
}
