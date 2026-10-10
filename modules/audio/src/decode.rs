//! Decoding a whole audio file into memory with `symphonia`.
//!
//! Tracks linked in chat are short, so the player decodes them completely
//! before playing: seeking is then a cursor move and the output never waits on
//! the disk. Files longer than [`MAX_DURATION`] fail instead of filling memory.

use std::f32::consts::FRAC_1_SQRT_2;
use std::fs::File;
use std::io::ErrorKind;
use std::path::Path;
use std::time::Duration;

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, DecoderOptions};
use symphonia::core::errors::Error as DecodeError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// The longest track the player decodes.
pub(crate) const MAX_DURATION: Duration = Duration::from_mins(30);

const NANOS_PER_SECOND: u128 = 1_000_000_000;

/// A decoded track: interleaved frames of one or two channels at the file's
/// native sample rate.
#[derive(Debug)]
pub(crate) struct Audio {
    /// Interleaved samples, `channels` per frame.
    pub(crate) samples: Vec<f32>,
    /// 1 or 2; wider layouts are folded to stereo while decoding.
    pub(crate) channels: usize,
    /// Frames per second.
    pub(crate) rate: u32,
}

impl Audio {
    /// The number of frames.
    pub(crate) fn frames(&self) -> usize {
        self.samples.len() / self.channels
    }

    /// The track's length.
    pub(crate) fn duration(&self) -> Duration {
        self.time_at(self.frames())
    }

    /// The time at which `frame` plays.
    pub(crate) fn time_at(&self, frame: usize) -> Duration {
        let nanos = frame as u128 * NANOS_PER_SECOND / u128::from(self.rate.max(1));
        Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
    }

    /// The frame playing at `position`, clamped to the end of the track.
    pub(crate) fn frame_at(&self, position: Duration) -> usize {
        let frame = position.as_nanos() * u128::from(self.rate) / NANOS_PER_SECOND;
        usize::try_from(frame)
            .unwrap_or(usize::MAX)
            .min(self.frames())
    }
}

/// The file name of `path`, for messages and the stream title.
pub(crate) fn display_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

/// Decodes the first audio track of `path` into memory.
///
/// # Errors
///
/// A short message naming the cause: the file can't be read, its format or
/// codec isn't supported, it holds no audio, or it is longer than
/// [`MAX_DURATION`].
pub(crate) fn decode(path: &Path) -> Result<Audio, String> {
    let name = display_name(path);
    let file = File::open(path).map_err(|error| format!("Couldn't read {name}: {error}"))?;
    decode_source(&name, Box::new(file), path)
}

/// Decodes the first audio track of `bytes`, an encoded file held in
/// memory; `track` names it in messages and hints its format.
///
/// # Errors
///
/// As [`decode`], minus reading a file.
pub(crate) fn decode_bytes(track: &Path, bytes: std::sync::Arc<[u8]>) -> Result<Audio, String> {
    let name = display_name(track);
    decode_source(&name, Box::new(std::io::Cursor::new(bytes)), track)
}

fn decode_source(
    name: &str,
    media: Box<dyn symphonia::core::io::MediaSource>,
    track: &Path,
) -> Result<Audio, String> {
    let name = name.to_owned();
    let source = MediaSourceStream::new(media, MediaSourceStreamOptions::default());
    let mut hint = Hint::new();
    if let Some(extension) = track.extension().and_then(|extension| extension.to_str()) {
        hint.with_extension(extension);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            source,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|error| describe(&name, &error))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|track| track.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| format!("{name} has no audio track"))?;
    let track_id = track.id;
    let mut rate = track.codec_params.sample_rate;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|error| describe(&name, &error))?;

    let mut samples = Vec::new();
    let mut channels = None;
    let mut buffer: Option<SampleBuffer<f32>> = None;
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(DecodeError::IoError(error)) if error.kind() == ErrorKind::UnexpectedEof => break,
            // A chained stream (a new logical track) starts; the first one is the track.
            Err(DecodeError::ResetRequired) => break,
            Err(error) => return Err(describe(&name, &error)),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let frames = match decoder.decode(&packet) {
            Ok(buffer) => buffer,
            // A corrupt packet is skipped, as players do; the rest still plays.
            Err(DecodeError::DecodeError(_)) => continue,
            Err(error) => return Err(describe(&name, &error)),
        };
        let spec = *frames.spec();
        let count = spec.channels.count();
        if *channels.get_or_insert(count) != count {
            return Err(format!(
                "Couldn't decode {name}: its channel layout changes"
            ));
        }
        let rate = *rate.get_or_insert(spec.rate);
        let needed = frames.capacity() * count;
        let buffer = match &mut buffer {
            Some(buffer) if buffer.capacity() >= needed => buffer,
            slot => slot.insert(SampleBuffer::new(frames.capacity() as u64, spec)),
        };
        buffer.copy_interleaved_ref(frames);
        fold_to_stereo(buffer.samples(), count, &mut samples);
        let limit = u128::from(rate) * MAX_DURATION.as_nanos() / NANOS_PER_SECOND;
        if (samples.len() / count.min(2)) as u128 > limit {
            return Err(format!("{name} is longer than 30 minutes"));
        }
    }

    let channels = channels.ok_or_else(|| format!("{name} contains no audio"))?;
    let rate = rate
        .filter(|rate| *rate > 0)
        .ok_or_else(|| format!("Couldn't decode {name}: unknown sample rate"))?;
    Ok(Audio {
        samples,
        channels: channels.min(2),
        rate,
    })
}

/// Turns a `symphonia` error into a short message naming the cause.
fn describe(name: &str, error: &DecodeError) -> String {
    match error {
        DecodeError::Unsupported(_) => "Unsupported audio format".to_owned(),
        DecodeError::IoError(error) => format!("Couldn't read {name}: {error}"),
        DecodeError::LimitError(limit) => format!("Couldn't decode {name}: {limit}"),
        DecodeError::DecodeError(cause) => format!("Couldn't decode {name}: {cause}"),
        other => format!("Couldn't decode {name}: {other}"),
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
        out.extend_from_slice(frames);
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
        let (left, right) = frame
            .iter()
            .zip(gains.clone())
            .fold((0.0, 0.0), |(left, right), (sample, (l, r))| {
                (left + sample * l, right + sample * r)
            });
        out.push(left / left_total);
        out.push(right / right_total);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::PathBuf;

    use super::*;

    /// Writes a 16-bit PCM WAV of `frames` (interleaved, `channels` wide) to a
    /// fresh temp file and returns its path.
    pub(crate) fn write_wav(name: &str, channels: u16, rate: u32, frames: &[i16]) -> PathBuf {
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

    #[test]
    fn frame_and_time_round_trip_and_clamp() {
        let audio = Audio {
            samples: vec![0.0; 44_100],
            channels: 1,
            rate: 44_100,
        };
        assert_eq!(audio.frame_at(Duration::from_millis(500)), 22_050);
        assert_eq!(audio.time_at(22_050), Duration::from_millis(500));
        assert_eq!(audio.frame_at(Duration::from_secs(9)), 44_100);
    }

    #[test]
    fn five_one_folds_with_centre_on_both_sides() {
        let mut out = Vec::new();
        fold_to_stereo(&[1.0, 0.0, 1.0, 1.0, 0.0, 1.0], 6, &mut out);
        let total = 2.0 + std::f32::consts::FRAC_1_SQRT_2;
        assert!((out[0] - (1.0 + std::f32::consts::FRAC_1_SQRT_2) / total).abs() < 1e-6);
        assert!((out[1] - (std::f32::consts::FRAC_1_SQRT_2 + 1.0) / total).abs() < 1e-6);
    }
}
