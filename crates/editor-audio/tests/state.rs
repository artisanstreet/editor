#![expect(clippy::float_cmp, reason = "copied samples are bit-exact")]

use super::*;
use crossbeam_channel::{Receiver, Sender, bounded};

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

#[test]
fn pausing_loading_invalidates_its_decoder_generation() {
    let mut state = Inner::default();
    let generation = state.begin(PathBuf::from("clip.wav"));
    state.pause();
    assert_eq!(state.snapshot().state, PlaybackState::Idle);
    assert!(!state.is_current(generation));
}

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
