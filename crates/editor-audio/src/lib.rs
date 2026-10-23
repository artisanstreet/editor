//! App-wide playback of local audio files linked in chat transcripts.
//!
//! One [`Player`] owns the app's single audio output and plays one track at a
//! time. A maintained decoder feeds a bounded PCM queue on a background thread,
//! so playback starts after one chunk and seeks never retain the full track.
//! Platform output remains `PulseAudio`'s native
//! protocol on Linux and the BSDs (`PipeWire` and `WSLg` serve it too), `cpal`
//! on Windows and macOS.

mod decode;
mod mix;
mod output;
mod state;
mod stream;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use state::Shared;

/// A cheap, cloneable handle to the app's one audio output. No device or
/// server is touched until the first `play`.
#[derive(Clone)]
pub struct Player {
    owner: Arc<Owner>,
}

/// The single owner of the shared state; dropping the last [`Player`] clone
/// stops playback, which ends the output thread and releases the stream.
struct Owner {
    shared: Arc<Shared>,
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.shared.lock().stop();
    }
}

impl Player {
    /// Creates an idle player.
    #[must_use]
    pub fn new() -> Self {
        Self {
            owner: Arc::new(Owner {
                shared: Arc::new(Shared::default()),
            }),
        }
    }

    /// Stops whatever plays, then decodes `path` on a background thread and plays it from the start.
    pub fn play(&self, path: &Path) {
        let path = path.to_path_buf();
        self.start(path.clone(), move || decode::StreamDecoder::open(&path));
    }

    /// Stops whatever plays, then decodes `bytes`, an encoded audio file held
    /// in memory, and plays it from the start. `track` identifies it in
    /// [`Snapshot::track`] and hints its format by its extension.
    pub fn play_bytes(&self, track: &Path, bytes: Arc<[u8]>) {
        self.start(track.to_path_buf(), move || {
            decode::StreamDecoder::bytes(bytes)
        });
    }

    fn start(
        &self,
        track: PathBuf,
        decode: impl FnOnce() -> Result<decode::StreamDecoder, String> + Send + 'static,
    ) {
        let shared = Arc::clone(&self.owner.shared);
        let generation = shared.lock().begin(track);
        let decoder_shared = Arc::clone(&shared);
        let spawned = thread::Builder::new()
            .name("artisan-audio-decode".to_owned())
            .spawn(move || match decode() {
                Ok(decoder) => stream::play(&decoder_shared, generation, decoder),
                Err(error) => decoder_shared.lock().fail(generation, error),
            });
        if let Err(error) = spawned {
            shared.lock().fail(
                generation,
                format!("Couldn't start the audio decoder: {error}"),
            );
        }
    }

    /// Pauses a playing track, keeping its position.
    pub fn pause(&self) {
        self.owner.shared.lock().pause();
    }

    /// Continues a paused track; after it ended, restarts it from the beginning.
    pub fn resume(&self) {
        self.owner.shared.lock().resume();
    }

    /// Moves the playhead, clamped to the track; works while playing, paused or ended.
    pub fn seek(&self, position: Duration) {
        self.owner.shared.lock().seek(position);
    }

    /// Stops playback and returns to idle, releasing the output stream.
    pub fn stop(&self) {
        self.owner.shared.lock().stop();
    }

    /// The current track, state and position.
    #[must_use]
    pub fn snapshot(&self) -> Snapshot {
        self.owner.shared.lock().snapshot()
    }
}

impl Default for Player {
    fn default() -> Self {
        Self::new()
    }
}

/// What the player is doing, for rendering transport controls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// The file last asked to play, until `stop`.
    pub track: Option<PathBuf>,
    /// Where playback stands.
    pub state: PlaybackState,
    /// The playhead from the start of the track.
    pub position: Duration,
    /// The decoder's known track length, or measured EOF when available.
    pub duration: Option<Duration>,
}

/// Where playback stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaybackState {
    /// Nothing is loaded.
    Idle,
    /// The track header and first audio chunk are being decoded.
    Loading,
    /// The track is audible.
    Playing,
    /// The track is held at its position.
    Paused,
    /// The track played to its end.
    Ended,
    /// The track couldn't be decoded or played; the message names the cause.
    Failed(String),
}

#[cfg(test)]
mod tests {
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
}
