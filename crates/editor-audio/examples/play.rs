use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use editor_audio::{PlaybackState, Player};

fn main() -> Result<(), String> {
    let path = match std::env::args_os().nth(1) {
        Some(path) => PathBuf::from(path),
        None => write_tone()?,
    };
    let player = Player::new();
    player.play(&path);
    let started = Instant::now();
    let mut paused = false;
    loop {
        let snapshot = player.snapshot();
        #[expect(
            clippy::print_stdout,
            reason = "this example is a terminal program that reports playback on stdout"
        )]
        {
            println!(
                "{:>5} ms  {:?}  {:?} / {:?}",
                started.elapsed().as_millis(),
                snapshot.state,
                snapshot.position,
                snapshot.duration
            );
        }
        match snapshot.state {
            PlaybackState::Ended => return Ok(()),
            PlaybackState::Failed(message) => return Err(message),
            PlaybackState::Playing if !paused && snapshot.position > Duration::from_millis(400) => {
                player.pause();
                paused = true;
                thread::sleep(Duration::from_millis(300));
                player.resume();
            }
            PlaybackState::Idle
            | PlaybackState::Loading
            | PlaybackState::Playing
            | PlaybackState::Paused => {}
        }
        if started.elapsed() > Duration::from_secs(30) {
            return Err("timed out".to_owned());
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn write_tone() -> Result<PathBuf, String> {
    const RATE: u32 = 48_000;
    let samples: Vec<i16> = (0..RATE)
        .map(|frame| {
            let phase = f64::from(frame) / f64::from(RATE) * 440.0 * std::f64::consts::TAU;
            #[expect(
                clippy::cast_possible_truncation,
                reason = "amplitude stays within i16"
            )]
            let sample = (phase.sin() * 3000.0) as i16;
            sample
        })
        .collect();
    let data = RATE * 2;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    for field in [
        16u32.to_le_bytes(),
        [1, 0, 1, 0],
        RATE.to_le_bytes(),
        (RATE * 2).to_le_bytes(),
        [2, 0, 16, 0],
    ] {
        bytes.extend_from_slice(&field);
    }
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    let path = std::env::temp_dir().join("artisan-audio-tone.wav");
    std::fs::write(&path, bytes)
        .map_err(|error| format!("Couldn't write {}: {error}", path.display()))?;
    Ok(path)
}
