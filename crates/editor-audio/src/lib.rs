#![expect(
    clippy::field_scoped_visibility_modifiers,
    clippy::inconsistent_struct_constructor,
    clippy::unused_result_ok,
    missing_debug_implementations,
    unreachable_pub,
    unused_qualifications,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#![allow(
    clippy::assertions_on_result_states,
    reason = "pre-existing violations that only occur in some builds, cleared in plan.md phase 2"
)]

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

#[derive(Clone)]
pub struct Player {
    owner: Arc<Owner>,
}

struct Owner {
    shared: Arc<Shared>,
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.shared.lock().stop();
    }
}

impl Player {
    #[must_use]
    pub fn new() -> Self {
        Self {
            owner: Arc::new(Owner {
                shared: Arc::new(Shared::default()),
            }),
        }
    }

    pub fn play(&self, path: &Path) {
        let path = path.to_path_buf();
        self.start(path.clone(), move || decode::StreamDecoder::open(&path));
    }

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

    pub fn pause(&self) {
        self.owner.shared.lock().pause();
    }

    pub fn resume(&self) {
        self.owner.shared.lock().resume();
    }

    pub fn seek(&self, position: Duration) {
        self.owner.shared.lock().seek(position);
    }

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub track: Option<PathBuf>,
    pub state: PlaybackState,
    pub position: Duration,
    pub duration: Option<Duration>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlaybackState {
    Idle,
    Loading,
    Playing,
    Paused,
    Ended,
    Failed(String),
}

#[cfg(test)]
#[path = "../tests/lib.rs"]
mod tests;
