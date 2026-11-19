use rodio::{Decoder, Source};
use std::collections::VecDeque;
use std::f32::consts::FRAC_1_SQRT_2;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const CHUNK_FRAMES: usize = 2048;
const NANOS_PER_SECOND: u128 = 1_000_000_000;

#[derive(Debug)]
pub(crate) struct Audio {
    pub(crate) samples: VecDeque<f32>,
    pub(crate) channels: usize,
    pub(crate) rate: u32,
}
impl Audio {
    pub(crate) fn frames(&self) -> usize {
        self.samples.len() / self.channels
    }
    pub(crate) fn duration(&self) -> Duration {
        self.time_at(self.frames())
    }
    pub(crate) fn time_at(&self, frame: usize) -> Duration {
        let nanos = frame as u128 * NANOS_PER_SECOND / u128::from(self.rate.max(1));
        Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
    }
    pub(crate) fn frame_at(&self, position: Duration) -> usize {
        self.absolute_frame_at(position).min(self.frames())
    }
    pub(crate) fn absolute_frame_at(&self, position: Duration) -> usize {
        usize::try_from(position.as_nanos() * u128::from(self.rate) / NANOS_PER_SECOND)
            .unwrap_or(usize::MAX)
    }
}
pub(crate) fn display_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

pub(crate) struct StreamDecoder {
    source: Box<dyn Source + Send>,
    channels: usize,
    rate: u32,
    pub(crate) duration: Option<Duration>,
    input_channels: usize,
}
impl StreamDecoder {
    pub(crate) fn open(path: &Path) -> Result<Self, String> {
        let name = display_name(path);
        let file = File::open(path).map_err(|error| format!("Couldn't read {name}: {error}"))?;
        Self::new(Decoder::try_from(file).map_err(describe)?)
    }
    pub(crate) fn bytes(bytes: Arc<[u8]>) -> Result<Self, String> {
        Self::new(Decoder::try_from(std::io::Cursor::new(bytes)).map_err(describe)?)
    }
    fn new(source: impl Source + Send + 'static) -> Result<Self, String> {
        let input_channels = usize::from(source.channels());
        let rate = source.sample_rate();
        if input_channels == 0 || input_channels > 32 || rate == 0 {
            return Err("Audio has an unsupported channel layout or sample rate".to_owned());
        }
        let duration = source.total_duration();
        Ok(Self {
            source: Box::new(source),
            channels: input_channels.min(2),
            rate,
            duration,
            input_channels,
        })
    }
    pub(crate) fn seek(&mut self, position: Duration) -> Result<(), String> {
        self.source
            .try_seek(position)
            .map_err(|error| format!("Couldn't seek audio: {error}"))
    }
    pub(crate) fn chunk(&mut self) -> Result<Vec<f32>, String> {
        let input: Vec<_> = self
            .source
            .by_ref()
            .take(CHUNK_FRAMES * self.input_channels)
            .collect();
        if usize::from(self.source.channels()) != self.input_channels
            || self.source.sample_rate() != self.rate
        {
            return Err("Audio channel layout or sample rate changes within the track".to_owned());
        }
        let mut samples = Vec::with_capacity(CHUNK_FRAMES * self.channels);
        fold_to_stereo(&input, self.input_channels, &mut samples);
        Ok(samples)
    }
    pub(crate) fn audio(&self, samples: Vec<f32>) -> Audio {
        Audio {
            samples: samples.into(),
            channels: self.channels,
            rate: self.rate,
        }
    }
}
fn describe(error: rodio::decoder::DecoderError) -> String {
    match error {
        rodio::decoder::DecoderError::UnrecognizedFormat
        | rodio::decoder::DecoderError::NoStreams => "Unsupported audio format".to_owned(),
        error @ (rodio::decoder::DecoderError::IoError(_)
        | rodio::decoder::DecoderError::DecodeError(_)
        | rodio::decoder::DecoderError::LimitError(_)
        | rodio::decoder::DecoderError::ResetRequired) => {
            format!("Couldn't decode audio: {error}")
        }
    }
}

const fn sanitize(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

fn fold_to_stereo(frames: &[f32], channels: usize, out: &mut Vec<f32>) {
    if channels <= 2 {
        out.extend(frames.iter().copied().map(sanitize));
        return;
    }
    let gains = (0..channels).map(|channel| match channel {
        0 => (1.0, 0.0),
        1 => (0.0, 1.0),
        2 => (FRAC_1_SQRT_2, FRAC_1_SQRT_2),
        3 => (0.0, 0.0),
        even if even % 2 == 0 => (1.0, 0.0),
        _ => (0.0, 1.0),
    });
    let (left_total, right_total) = gains
        .clone()
        .fold((0.0, 0.0), |(left, right), (l, r)| (left + l, right + r));
    out.reserve(frames.len() / channels * 2);
    for frame in frames.chunks_exact(channels) {
        let (left, right) =
            frame
                .iter()
                .zip(gains.clone())
                .fold((0.0, 0.0), |(left, right), (sample, (l, r))| {
                    let sample = sanitize(*sample);
                    (sample.mul_add(l, left), sample.mul_add(r, right))
                });
        out.push(left / left_total);
        out.push(right / right_total);
    }
}

#[cfg(test)]
#[path = "../tests/decode.rs"]
mod tests;
