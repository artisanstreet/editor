//! The shared playback state: the track, its decoded audio, and the cursor
//! the output callback advances.
//!
//! Every transition lives here and touches no device, so the player's
//! behaviour is testable without audio hardware. A generation counter makes a
//! newer `play` or `stop` win over work still in flight for an older track.

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crate::decode::Audio;
use crate::mix;
use crate::output::{Format, Output};
use crate::stream::{PcmEvent, StreamState};
use crate::{PlaybackState, Snapshot};

/// The state shared by the player, the decoder thread and the output.
#[derive(Default)]
pub struct Shared {
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
pub struct Inner {
    /// Bumped by every `play` and `stop`; work tagged with an older value is stale.
    generation: u64,
    track: Option<PathBuf>,
    state: PlaybackState,
    audio: Option<Audio>,
    stream: Option<StreamState>,
    reset_phase: bool,
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
            stream: None,
            reset_phase: false,
            cursor: 0,
            output: None,
        }
    }
}

impl Inner {
    pub(crate) const fn is_current(&self, generation: u64) -> bool {
        self.generation == generation
    }

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
                self.audio = Some(audio);
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

    pub(crate) fn stream_ready(
        &mut self,
        generation: u64,
        first: Audio,
        stream: StreamState,
    ) -> Option<Format> {
        let format = self.loaded(generation, Ok(first))?;
        self.stream = Some(stream);
        Some(format)
    }
    pub(crate) fn take_seek(&mut self, generation: u64) -> Option<(u64, Duration)> {
        if !self.is_current(generation) {
            return None;
        }
        let stream = self.stream.as_mut()?;
        stream.seek.take().map(|position| (stream.epoch, position))
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
        self.stream = None;
        self.reset_phase = false;
        self.cursor = 0;
        self.output = None;
    }

    /// Holds a playing track at its position.
    pub(crate) fn pause(&mut self) {
        if self.state == PlaybackState::Loading {
            self.stop();
        } else if self.state == PlaybackState::Playing {
            self.state = PlaybackState::Paused;
            self.sync_output();
        }
    }

    /// Continues a paused track, or replays an ended one from the start.
    pub(crate) fn resume(&mut self) {
        match self.state {
            PlaybackState::Paused => {}
            PlaybackState::Ended => {
                if self.stream.is_some() {
                    self.seek(Duration::ZERO);
                } else {
                    self.cursor = 0;
                    self.flush_output();
                }
                self.reset_phase = true;
            }
            PlaybackState::Idle
            | PlaybackState::Loading
            | PlaybackState::Playing
            | PlaybackState::Failed(_) => return,
        }
        self.state = PlaybackState::Playing;
        self.sync_output();
    }

    /// Moves the playhead, clamped to the track's end. Seeking an ended track
    /// back into it leaves it paused there.
    pub(crate) fn seek(&mut self, position: Duration) {
        if !matches!(
            self.state,
            PlaybackState::Playing | PlaybackState::Paused | PlaybackState::Ended
        ) {
            return;
        }
        let Some(audio) = &mut self.audio else {
            return;
        };
        self.reset_phase = true;
        if let Some(stream) = &mut self.stream {
            let position = stream
                .duration
                .map_or(position, |duration| position.min(duration));
            stream.epoch = stream.epoch.wrapping_add(1);
            stream.seek = Some(position);
            stream.start = audio.absolute_frame_at(position);
            stream.ended = false;
            audio.samples.clear();
            self.cursor = 0;
            // Older buffered chunks cannot satisfy a newer seek.
            while stream.pcm.try_recv().is_ok() {}
            // A wakeup already queued covers this seek too.
            stream.signal.try_send(()).ok();
            if self.state == PlaybackState::Ended {
                self.state = PlaybackState::Paused;
            }
        } else {
            self.cursor = audio.frame_at(position);
            if self.state == PlaybackState::Ended && self.cursor < audio.frames() {
                self.state = PlaybackState::Paused;
            }
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
            position: self.audio.as_ref().map_or(Duration::ZERO, |audio| {
                audio.time_at(
                    self.cursor
                        .saturating_add(self.stream.as_ref().map_or(0, |stream| stream.start)),
                )
            }),
            duration: self.stream.as_ref().map_or_else(
                || self.audio.as_ref().map(super::decode::Audio::duration),
                |stream| stream.duration,
            ),
        }
    }

    /// Whether the output for `generation` should be running; `None` once it
    /// is stale and should close.
    pub(crate) fn wants_running(&self, generation: u64) -> Option<bool> {
        (generation == self.generation && self.output.is_some())
            .then_some(self.state == PlaybackState::Playing)
    }

    /// Fills `out` for the output of `generation`: the track's next samples
    /// while playing (see [`mix::render_mapped`]), silence otherwise. Marks the
    /// track ended once its last frame was read.
    #[cfg(any(test, all(unix, not(target_os = "macos"))))]
    pub(crate) fn fill(
        &mut self,
        generation: u64,
        out: &mut [f32],
        out_channels: usize,
        step: f64,
        phase: &mut f64,
    ) {
        self.fill_mapped(generation, out, out_channels, step, phase, |sample| sample);
    }

    pub(crate) fn fill_mapped<T: Copy>(
        &mut self,
        generation: u64,
        out: &mut [T],
        out_channels: usize,
        step: f64,
        phase: &mut f64,
        convert: impl Fn(f32) -> T,
    ) {
        if generation != self.generation || self.state != PlaybackState::Playing {
            out.fill(convert(0.0));
            return;
        }
        if self.reset_phase {
            *phase = 0.0;
            self.reset_phase = false;
        }
        let Some(audio) = &mut self.audio else {
            out.fill(convert(0.0));
            return;
        };
        let ended = if let Some(stream) = &mut self.stream {
            // Two source frames permit interpolation. A wider resampling step
            // may need more; the bounded producer queue caps this window.
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "the step is a positive resampling ratio of a few frames"
            )]
            let needed = step.ceil().max(1.0) as usize + 1;
            for frame in out.chunks_mut(out_channels.max(1)) {
                while audio.frames().saturating_sub(self.cursor) < needed && !stream.ended {
                    match stream.pcm.try_recv() {
                        Ok(PcmEvent::Data { epoch, samples }) if epoch == stream.epoch => {
                            audio.samples.extend(samples);
                        }
                        Ok(PcmEvent::End { epoch }) if epoch == stream.epoch => {
                            stream.ended = true;
                            stream.duration =
                                Some(audio.time_at(stream.start.saturating_add(audio.frames())));
                        }
                        Ok(_) => {}
                        Err(crossbeam_channel::TryRecvError::Disconnected) => {
                            stream.ended = true;
                        }
                        Err(crossbeam_channel::TryRecvError::Empty) => break,
                    }
                }
                mix::render_mapped(
                    audio,
                    &mut self.cursor,
                    phase,
                    step,
                    frame,
                    out_channels,
                    &convert,
                );
            }
            let ended = stream.ended && self.cursor >= audio.frames();
            audio.samples.drain(..self.cursor * audio.channels);
            stream.start = stream.start.saturating_add(self.cursor);
            self.cursor = 0;
            ended
        } else {
            mix::render_mapped(
                audio,
                &mut self.cursor,
                phase,
                step,
                out,
                out_channels,
                convert,
            )
        };
        if ended {
            self.state = PlaybackState::Ended;
            self.sync_output();
        }
    }

    fn reset(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.track = None;
        self.state = PlaybackState::Idle;
        self.audio = None;
        self.stream = None;
        self.reset_phase = false;
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

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    #[test]
    fn pausing_loading_invalidates_its_decoder_generation() {
        let mut state = Inner::default();
        let generation = state.begin(PathBuf::from("clip.wav"));
        state.pause();
        assert_eq!(state.snapshot().state, PlaybackState::Idle);
        assert!(!state.is_current(generation));
    }
}

#[cfg(test)]
mod streaming_tests {
    use super::*;
    use crossbeam_channel::{Receiver, Sender, bounded};
    fn streaming() -> (Inner, u64, Sender<PcmEvent>, Receiver<()>) {
        let mut inner = Inner::default();
        let generation = inner.begin(PathBuf::from("stream.wav"));
        let (sender, pcm) = bounded(8);
        let (signal, commands) = bounded(1);
        let audio = Audio {
            samples: [0.1, 0.2].into(),
            channels: 1,
            rate: 10,
        };
        inner
            .stream_ready(
                generation,
                audio,
                StreamState::new(pcm, signal, Some(Duration::from_secs(100))),
            )
            .unwrap();
        (inner, generation, sender, commands)
    }
    #[test]
    #[expect(clippy::float_cmp, reason = "samples are copied, never computed")]
    fn a_temporary_underrun_does_not_end_the_track() {
        let (mut inner, generation, sender, _commands) = streaming();
        let mut first = [0.0; 4];
        inner.fill(generation, &mut first, 1, 1.0, &mut 0.0);
        assert_eq!(first, [0.1, 0.2, 0.0, 0.0]);
        assert_eq!(inner.snapshot().state, PlaybackState::Playing);
        sender
            .send(PcmEvent::Data {
                epoch: 0,
                samples: vec![0.3, 0.4],
            })
            .unwrap();
        sender.send(PcmEvent::End { epoch: 0 }).unwrap();
        let mut next = [0.0; 3];
        inner.fill(generation, &mut next, 1, 1.0, &mut 0.0);
        assert_eq!(next, [0.3, 0.4, 0.0]);
        assert_eq!(inner.snapshot().state, PlaybackState::Ended);
        assert_eq!(inner.snapshot().position, Duration::from_millis(400));
        assert_eq!(inner.snapshot().duration, Some(Duration::from_millis(400)));
    }
    #[test]
    #[expect(clippy::float_cmp, reason = "samples are copied, never computed")]
    fn seeks_coalesce_and_old_pcm_cannot_replace_the_new_position() {
        let (mut inner, generation, sender, commands) = streaming();
        inner.seek(Duration::from_secs(2));
        inner.seek(Duration::from_secs(5));
        assert_eq!(commands.len(), 1);
        assert_eq!(
            inner.take_seek(generation),
            Some((2, Duration::from_secs(5)))
        );
        sender
            .send(PcmEvent::Data {
                epoch: 0,
                samples: vec![0.9, 0.9],
            })
            .unwrap();
        sender
            .send(PcmEvent::Data {
                epoch: 2,
                samples: vec![0.5, 0.6],
            })
            .unwrap();
        let mut out = [0.0; 2];
        inner.fill(generation, &mut out, 1, 1.0, &mut 0.0);
        assert_eq!(out, [0.5, 0.6]);
        assert_eq!(inner.snapshot().position, Duration::from_millis(5200));
    }
    #[test]
    fn stopping_releases_both_bounded_channels() {
        let (mut inner, _, sender, commands) = streaming();
        inner.stop();
        assert!(commands.recv().is_err());
        assert!(sender.send(PcmEvent::End { epoch: 0 }).is_err());
    }
}
