use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crate::decode::Audio;
use crate::mix;
use crate::output::{Format, Output};
use crate::stream::{PcmEvent, StreamState};
use crate::{PlaybackState, Snapshot};

#[derive(Default)]
pub struct Shared {
    inner: Mutex<Inner>,
}

impl Shared {
    pub(crate) fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

pub struct Inner {
    generation: u64,
    track: Option<PathBuf>,
    state: PlaybackState,
    audio: Option<Audio>,
    stream: Option<StreamState>,
    reset_phase: bool,
    cursor: usize,
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

    pub(crate) fn begin(&mut self, track: PathBuf) -> u64 {
        self.reset();
        self.track = Some(track);
        self.state = PlaybackState::Loading;
        self.generation
    }

    fn loaded(&mut self, generation: u64, decoded: Result<Audio, String>) -> Option<Format> {
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

    pub(crate) fn attach(&mut self, output: Output) {
        self.output = Some(output);
    }

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

    pub(crate) fn pause(&mut self) {
        if self.state == PlaybackState::Loading {
            self.stop();
        } else if self.state == PlaybackState::Playing {
            self.state = PlaybackState::Paused;
            self.sync_output();
        }
    }

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
            while stream.pcm.try_recv().is_ok() {}
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

    pub(crate) fn stop(&mut self) {
        self.reset();
    }

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

    pub(crate) fn wants_running(&self, generation: u64) -> Option<bool> {
        (generation == self.generation && self.output.is_some())
            .then_some(self.state == PlaybackState::Playing)
    }

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
#[path = "../tests/state.rs"]
mod tests;
