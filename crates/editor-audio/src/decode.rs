//! Maintained streaming decoder. One chunk is decoded on the producer thread;
//! codec probing, packet buffering and seeking belong to rodio/Symphonia.
use rodio::{Decoder, Source};
use std::collections::VecDeque;
use std::f32::consts::FRAC_1_SQRT_2;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

pub(crate) const CHUNK_FRAMES: usize = 2048;
const NANOS_PER_SECOND: u128 = 1_000_000_000;

/// The small interleaved window currently available to the output callback.
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
    pub(crate) channels: usize,
    pub(crate) rate: u32,
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
        Ok(Self {
            duration: source.total_duration(),
            source: Box::new(source),
            channels: input_channels.min(2),
            rate,
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
        error => format!("Couldn't decode audio: {error}"),
    }
}

// Fixtures collect a short source only. Production never materializes a track.
#[cfg(test)]
fn collect(mut decoder: StreamDecoder) -> Result<Audio, String> {
    let mut samples = Vec::new();
    loop {
        let chunk = decoder.chunk()?;
        if chunk.is_empty() {
            break;
        }
        samples.extend(chunk);
    }
    Ok(decoder.audio(samples))
}
#[cfg(test)]
fn decode(path: &Path) -> Result<Audio, String> {
    collect(StreamDecoder::open(path)?)
}
#[cfg(test)]
fn decode_bytes(_: &Path, bytes: Arc<[u8]>) -> Result<Audio, String> {
    collect(StreamDecoder::bytes(bytes)?)
}

/// Decoded float samples are normalized before gain arithmetic or output.
pub(crate) fn sanitize(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

/// Appends interleaved `frames` of `channels` to `out`, folding layouts wider
/// than stereo down to stereo.
///
/// Wide layouts follow `symphonia`'s channel order (front left, front right,
/// centre, LFE, then left/right pairs): the centre goes to both sides at
/// -3 dB, the LFE is dropped, and each side is normalised by its total gain so
/// the fold can't clip.
pub(crate) fn fold_to_stereo(frames: &[f32], channels: usize, out: &mut Vec<f32>) {
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
                    (left + sample * l, right + sample * r)
                });
        out.push(left / left_total);
        out.push(right / right_total);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// Writes a 16-bit PCM WAV of `frames` (interleaved, `channels` wide) to a
    /// fresh temp file and returns its path.
    fn write_wav(name: &str, channels: u16, rate: u32, frames: &[i16]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("artisan-audio-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(name);
        let data = u32::try_from(frames.len() * 2).expect("small wav");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&channels.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
        bytes.extend_from_slice(&(channels * 2).to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data.to_le_bytes());
        for sample in frames {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        std::fs::write(&path, bytes).expect("write wav");
        path
    }

    #[test]
    fn decodes_a_stereo_wav() {
        let frames: Vec<i16> = (0..4000).flat_map(|i| [i16::MAX / 2, -(i % 100)]).collect();
        let path = write_wav("stereo.wav", 2, 8000, &frames);
        let audio = decode(&path).expect("decodes");
        assert_eq!(audio.channels, 2);
        assert_eq!(audio.rate, 8000);
        assert_eq!(audio.frames(), 4000);
        assert_eq!(audio.duration(), Duration::from_millis(500));
        assert!((audio.samples[0] - 0.5).abs() < 0.001);
        assert!((audio.samples[2 * 99 + 1] + 99.0 / 32768.0).abs() < 1e-6);
    }

    #[test]
    fn decodes_the_same_audio_from_memory() {
        let frames: Vec<i16> = (0..800).flat_map(|i| [i16::MAX / 2, -(i % 100)]).collect();
        let path = write_wav("memory.wav", 2, 8000, &frames);
        let bytes: std::sync::Arc<[u8]> = std::fs::read(&path).expect("read").into();
        // The track's name only labels it: no file exists at it.
        let audio = decode_bytes(Path::new("from-forge/clip.wav"), bytes).expect("decodes");
        assert_eq!(audio.channels, 2);
        assert_eq!(audio.rate, 8000);
        assert_eq!(audio.frames(), 800);
        assert!(decode_bytes(Path::new("clip.wav"), b"not audio".as_slice().into()).is_err());
    }

    #[test]
    fn folds_a_wide_wav_to_stereo() {
        let frames: Vec<i16> = (0..100).flat_map(|_| [1000, 0, 0, 0, 0, 0]).collect();
        let audio = decode(&write_wav("wide.wav", 6, 8000, &frames)).expect("decodes");
        assert_eq!(audio.channels, 2);
        assert_eq!(audio.frames(), 100);
        assert!(audio.samples[0] > 0.0);
        assert!(audio.samples[1].abs() < f32::EPSILON);
    }

    #[test]
    fn rejects_a_non_audio_file() {
        let path = write_wav("notes.txt", 1, 8000, &[]);
        std::fs::write(&path, "just some notes, not audio").expect("write");
        assert_eq!(
            decode(&path).expect_err("fails"),
            "Unsupported audio format"
        );
    }

    #[test]
    fn missing_files_name_the_file() {
        let error = decode(Path::new("/nonexistent/song.mp3")).expect_err("fails");
        assert!(error.starts_with("Couldn't read song.mp3: "), "{error}");
    }
}

#[cfg(test)]
mod stream_budget_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct CountingSource(Arc<AtomicUsize>);
    impl Iterator for CountingSource {
        type Item = f32;
        fn next(&mut self) -> Option<f32> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Some(0.25)
        }
    }
    impl Source for CountingSource {
        fn current_span_len(&self) -> Option<usize> {
            None
        }
        fn channels(&self) -> u16 {
            1
        }
        fn sample_rate(&self) -> u32 {
            48000
        }
        fn total_duration(&self) -> Option<Duration> {
            Some(Duration::from_secs(3600))
        }
    }
    #[test]
    fn a_long_source_decodes_only_one_bounded_chunk() {
        let reads = Arc::new(AtomicUsize::new(0));
        let mut decoder = StreamDecoder::new(CountingSource(reads.clone())).unwrap();
        assert_eq!(reads.load(Ordering::Relaxed), 0);
        assert_eq!(decoder.duration, Some(Duration::from_secs(3600)));
        assert_eq!(decoder.chunk().unwrap().len(), CHUNK_FRAMES);
        assert_eq!(reads.load(Ordering::Relaxed), CHUNK_FRAMES);
    }
}

#[cfg(test)]
mod sample_boundary_tests {
    use super::*;
    #[test]
    fn decoded_nonfinite_samples_are_silent_in_each_layout() {
        for channels in [1, 2, 6] {
            let mut samples = Vec::new();
            fold_to_stereo(&[f32::NAN; 12], channels, &mut samples);
            assert!(samples.iter().all(|sample| *sample == 0.0));
            samples.clear();
            fold_to_stereo(&[f32::INFINITY; 12], channels, &mut samples);
            assert!(samples.iter().all(|sample| *sample == 0.0));
            samples.clear();
            fold_to_stereo(&[f32::MAX; 12], channels, &mut samples);
            assert!(
                samples
                    .iter()
                    .all(|sample| sample.is_finite() && sample.abs() <= 1.0)
            );
        }
    }
}
