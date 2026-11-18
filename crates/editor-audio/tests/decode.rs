use std::path::PathBuf;

use super::*;

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
fn decode(path: &Path) -> Result<Audio, String> {
    collect(StreamDecoder::open(path)?)
}
fn decode_bytes(_: &Path, bytes: Arc<[u8]>) -> Result<Audio, String> {
    collect(StreamDecoder::bytes(bytes)?)
}

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
        let mut decoder = StreamDecoder::new(CountingSource(Arc::clone(&reads))).unwrap();
        assert_eq!(reads.load(Ordering::Relaxed), 0);
        assert_eq!(decoder.duration, Some(Duration::from_secs(3600)));
        assert_eq!(decoder.chunk().unwrap().len(), CHUNK_FRAMES);
        assert_eq!(reads.load(Ordering::Relaxed), CHUNK_FRAMES);
    }
}

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
