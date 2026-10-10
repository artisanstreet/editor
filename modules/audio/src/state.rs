//! The shared playback state: the track, its decoded audio, and the cursor
//! the output callback advances.
//!
//! Every transition lives here and touches no device, so the player's
//! behaviour is testable without audio hardware. A generation counter makes a
//! newer `play` or `stop` win over work still in flight for an older track.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crate::decode::Audio;
use crate::mix;
use crate::output::{Format, Output};
use crate::{PlaybackState, Snapshot};

/// The state shared by the player, the decoder thread and the output.
#[derive(Default)]
pub(crate) struct Shared {
    inner: Mutex<Inner>,
}

impl Shared {
    /// Locks the state. A panic elsewhere can't leave it inconsistent (every
    /// transition is a few field writes), so a poisoned lock is still used.
    pub(crate) fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The state behind [`Shared`]'s lock.
pub(crate) struct Inner {
    /// Bumped by every `play` and `stop`; work tagged with an older value is stale.
    generation: u64,
    track: Option<PathBuf>,
    state: PlaybackState,
    audio: Option<Arc<Audio>>,
    /// The next frame the output reads.
    cursor: usize,
    /// The open output; dropping it ends the output thread and its stream.
    output: Option<Output>,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            generation: 0,
            track: None,
            state: PlaybackState::Idle,
            audio: None,
            cursor: 0,
            output: None,
        }
    }
}

impl Inner {
    /// Drops the current track and starts loading `track`, returning the
    /// generation its decode belongs to.
    pub(crate) fn begin(&mut self, track: PathBuf) -> u64 {
        self.reset();
        self.track = Some(track);
        self.state = PlaybackState::Loading;
        self.generation
    }

    /// Takes a finished decode. Returns the output format to open when the
    /// decode is still current and succeeded.
    pub(crate) fn loaded(
        &mut self,
        generation: u64,
        decoded: Result<Audio, String>,
    ) -> Option<Format> {
        if generation != self.generation {
            return None;
        }
        match decoded {
            Ok(audio) => {
                let format = Format {
                    channels: audio.channels,
                    rate: audio.rate,
                    title: self.track.as_deref().map(crate::decode::display_name),
                };
                self.audio = Some(Arc::new(audio));
                self.cursor = 0;
                self.state = PlaybackState::Playing;
                Some(format)
            }
            Err(message) => {
                self.state = PlaybackState::Failed(message);
                None
            }
        }
    }

    /// Keeps the output opened for the current track.
    pub(crate) fn attach(&mut self, output: Output) {
        self.output = Some(output);
    }

    /// Fails the current track with `message`, unless `generation` is stale.
    /// Releases the output.
    pub(crate) fn fail(&mut self, generation: u64, message: String) {
        if generation != self.generation || self.state == PlaybackState::Idle {
            return;
        }
        self.state = PlaybackState::Failed(message);
        self.audio = None;
        self.cursor = 0;
        self.output = None;
    }

    /// Holds a playing track at its position.
    pub(crate) fn pause(&mut self) {
        if self.state == PlaybackState::Playing {
            self.state = PlaybackState::Paused;
            self.sync_output();
        }
    }

    /// Continues a paused track, or replays an ended one from the start.
    pub(crate) fn resume(&mut self) {
        match self.state {
            PlaybackState::Paused => {}
            PlaybackState::Ended => {
                self.cursor = 0;
                self.flush_output();
            }
            _ => return,
        }
        self.state = PlaybackState::Playing;
        self.sync_output();
    }

    /// Moves the playhead, clamped to the track's end. Seeking an ended track
    /// back into it leaves it paused there.
    pub(crate) fn seek(&mut self, position: Duration) {
        let Some(audio) = &self.audio else {
            return;
        };
        if !matches!(
            self.state,
            PlaybackState::Playing | PlaybackState::Paused | PlaybackState::Ended
        ) {
            return;
        }
        self.cursor = audio.frame_at(position);
        if self.state == PlaybackState::Ended && self.cursor < audio.frames() {
            self.state = PlaybackState::Paused;
        }
        self.flush_output();
        self.sync_output();
    }

    /// Returns to idle and releases the output.
    pub(crate) fn stop(&mut self) {
        self.reset();
    }

    /// The state as the UI sees it.
    pub(crate) fn snapshot(&self) -> Snapshot {
        Snapshot {
            track: self.track.clone(),
            state: self.state.clone(),
            position: self
                .audio
                .as_ref()
                .map_or(Duration::ZERO, |audio| audio.time_at(self.cursor)),
            duration: self.audio.as_ref().map(|audio| audio.duration()),
        }
    }

    /// Whether the output for `generation` should be running; `None` once it
    /// is stale and should close.
    pub(crate) fn wants_running(&self, generation: u64) -> Option<bool> {
        (generation == self.generation && self.output.is_some())
            .then_some(self.state == PlaybackState::Playing)
    }

    /// Fills `out` for the output of `generation`: the track's next samples
    /// while playing (see [`mix::render`]), silence otherwise. Marks the
    /// track ended once its last frame was read.
    pub(crate) fn fill(
        &mut self,
        generation: u64,
        out: &mut [f32],
        out_channels: usize,
        step: f64,
        phase: &mut f64,
    ) {
        let audio = match &self.audio {
            Some(audio)
                if generation == self.generation && self.state == PlaybackState::Playing =>
            {
                audio
            }
            _ => {
                out.fill(0.0);
                return;
            }
        };
        if mix::render(audio, &mut self.cursor, phase, step, out, out_channels) {
            self.state = PlaybackState::Ended;
            self.sync_output();
        }
    }

    fn reset(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.track = None;
        self.state = PlaybackState::Idle;
        self.audio = None;
        self.cursor = 0;
        self.output = None;
    }

    fn sync_output(&self) {
        if let Some(output) = &self.output {
            output.sync();
        }
    }

    fn flush_output(&self) {
        if let Some(output) = &self.output {
            output.flush();
        }
    }
}

#[cfg(test)]
#[expect(clippy::float_cmp, reason = "copied samples are bit-exact")]
mod tests {
    use super::*;

    /// One second of stereo audio at 1 kHz whose left channel counts frames.
    fn second() -> Audio {
        Audio {
            samples: (0..1000u16).flat_map(|i| [f32::from(i), 0.0]).collect(),
            channels: 2,
            rate: 1000,
        }
    }

    fn playing() -> Inner {
        let mut inner = Inner::default();
        let generation = inner.begin(PathBuf::from("song.wav"));
        assert_eq!(inner.snapshot().state, PlaybackState::Loading);
        assert!(inner.loaded(generation, Ok(second())).is_some());
        inner
    }

    #[test]
    fn decode_moves_loading_to_playing() {
        let snapshot = playing().snapshot();
        assert_eq!(snapshot.state, PlaybackState::Playing);
        assert_eq!(snapshot.track, Some(PathBuf::from("song.wav")));
        assert_eq!(snapshot.position, Duration::ZERO);
        assert_eq!(snapshot.duration, Some(Duration::from_secs(1)));
    }

    #[test]
    fn a_newer_play_wins_over_a_slower_decode() {
        let mut inner = Inner::default();
        let first = inner.begin(PathBuf::from("slow.flac"));
        let second_generation = inner.begin(PathBuf::from("fast.wav"));
        assert!(inner.loaded(second_generation, Ok(second())).is_some());
        assert!(inner.loaded(first, Err("late".to_owned())).is_none());
        inner.fail(first, "late".to_owned());
        let snapshot = inner.snapshot();
        assert_eq!(snapshot.track, Some(PathBuf::from("fast.wav")));
        assert_eq!(snapshot.state, PlaybackState::Playing);
    }

    #[test]
    fn a_failed_decode_reports_its_message() {
        let mut inner = Inner::default();
        let generation = inner.begin(PathBuf::from("notes.txt"));
        assert!(
            inner
                .loaded(generation, Err("Unsupported audio format".to_owned()))
                .is_none()
        );
        assert_eq!(
            inner.snapshot().state,
            PlaybackState::Failed("Unsupported audio format".to_owned())
        );
    }

    #[test]
    fn seek_clamps_to_the_track() {
        let mut inner = playing();
        inner.seek(Duration::from_millis(250));
        assert_eq!(inner.snapshot().position, Duration::from_millis(250));
        inner.seek(Duration::from_secs(60));
        assert_eq!(inner.snapshot().position, Duration::from_secs(1));
    }

    #[test]
    fn pause_holds_and_resume_continues() {
        let mut inner = playing();
        let generation = inner.generation;
        let mut phase = 0.0;
        let mut out = [1.0; 20];
        inner.fill(generation, &mut out, 2, 1.0, &mut phase);
        assert_eq!(out[18], 9.0);
        inner.pause();
        assert_eq!(inner.snapshot().state, PlaybackState::Paused);
        inner.fill(generation, &mut out, 2, 1.0, &mut phase);
        assert!(out.iter().all(|sample| *sample == 0.0));
        assert_eq!(inner.snapshot().position, Duration::from_millis(10));
        inner.resume();
        inner.fill(generation, &mut out, 2, 1.0, &mut phase);
        assert_eq!(out[0], 10.0);
    }

    #[test]
    fn running_out_ends_and_resume_restarts() {
        let mut inner = playing();
        let generation = inner.generation;
        let mut phase = 0.0;
        let mut out = vec![0.0; 2100];
        inner.fill(generation, &mut out, 2, 1.0, &mut phase);
        assert_eq!(inner.snapshot().state, PlaybackState::Ended);
        assert_eq!(out[1998], 999.0);
        assert!(out[2000..].iter().all(|sample| *sample == 0.0));
        inner.resume();
        assert_eq!(inner.snapshot().state, PlaybackState::Playing);
        assert_eq!(inner.snapshot().position, Duration::ZERO);
    }

    #[test]
    fn seeking_back_into_an_ended_track_pauses_there() {
        let mut inner = playing();
        inner.seek(Duration::from_secs(5));
        let generation = inner.generation;
        inner.fill(generation, &mut [0.0; 4], 2, 1.0, &mut 0.0);
        assert_eq!(inner.snapshot().state, PlaybackState::Ended);
        inner.seek(Duration::from_millis(500));
        let snapshot = inner.snapshot();
        assert_eq!(snapshot.state, PlaybackState::Paused);
        assert_eq!(snapshot.position, Duration::from_millis(500));
    }

    #[test]
    fn stale_output_reads_silence() {
        let mut inner = playing();
        let stale = inner.generation;
        let generation = inner.begin(PathBuf::from("next.wav"));
        assert!(inner.loaded(generation, Ok(second())).is_some());
        let mut out = [1.0; 4];
        inner.fill(stale, &mut out, 2, 1.0, &mut 0.0);
        assert_eq!(out, [0.0; 4]);
        assert_eq!(inner.wants_running(stale), None);
    }

    #[test]
    fn stop_clears_everything() {
        let mut inner = playing();
        inner.stop();
        assert_eq!(
            inner.snapshot(),
            Snapshot {
                track: None,
                state: PlaybackState::Idle,
                position: Duration::ZERO,
                duration: None,
            }
        );
    }
}
