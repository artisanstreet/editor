use super::*;

fn audio(samples: &[f32], channels: usize) -> Audio {
    Audio {
        samples: samples.to_vec().into(),
        channels,
        rate: 1000,
    }
}

#[test]
fn upsampling_interpolates_between_frames() {
    let audio = audio(&[0.0, 1.0], 1);
    let (mut cursor, mut phase, mut out) = (0, 0.0, [9.0; 4]);
    render(&audio, &mut cursor, &mut phase, 0.5, &mut out, 1);
    assert_eq!(out, [0.0, 0.5, 1.0, 1.0]);
}

#[test]
fn downsampling_skips_frames() {
    let audio = audio(&[0.0, 0.1, 0.2, 0.3, 0.4, 0.5], 1);
    let (mut cursor, mut phase, mut out) = (0, 0.0, [9.0; 4]);
    assert!(render(&audio, &mut cursor, &mut phase, 2.0, &mut out, 1));
    assert_eq!(out, [0.0, 0.2, 0.4, 0.0]);
}

#[test]
fn mono_feeds_both_sides_and_extra_channels_stay_silent() {
    let audio = audio(&[0.5], 1);
    let (mut cursor, mut phase, mut out) = (0, 0.0, [9.0; 4]);
    render(&audio, &mut cursor, &mut phase, 1.0, &mut out, 4);
    assert_eq!(out, [0.5, 0.5, 0.0, 0.0]);
}

#[test]
fn stereo_averages_for_a_mono_device() {
    let audio = audio(&[0.2, 0.6], 2);
    let (mut cursor, mut phase, mut out) = (0, 0.0, [9.0; 1]);
    render(&audio, &mut cursor, &mut phase, 1.0, &mut out, 1);
    assert!((out[0] - 0.4).abs() < 1e-6);
}
#[test]
fn mapped_device_samples_preserve_resampling_and_channel_layout() {
    let track = audio(&[0.0, 1.0], 1);
    let (mut cursor, mut phase, mut out) = (0, 0.0, [9.0f64; 8]);
    assert!(render_mapped(
        &track,
        &mut cursor,
        &mut phase,
        0.5,
        &mut out,
        2,
        f64::from
    ));
    assert_eq!(out, [0.0, 0.0, 0.5, 0.5, 1.0, 1.0, 1.0, 1.0]);
    assert_eq!(cursor, 2);
}

fn render(
    audio: &Audio,
    cursor: &mut usize,
    phase: &mut f64,
    step: f64,
    out: &mut [f32],
    out_channels: usize,
) -> bool {
    render_mapped(audio, cursor, phase, step, out, out_channels, |sample| {
        sample
    })
}

mod invalid_float_output_tests {
    use super::*;
    #[test]
    #[expect(clippy::float_cmp, reason = "silence is exactly zero")]
    fn mapped_float_output_never_carries_nan_or_infinity() {
        let audio = Audio {
            samples: [f32::NAN, f32::INFINITY].into(),
            channels: 1,
            rate: 48000,
        };
        let mut out = [9.0f64; 2];
        render_mapped(&audio, &mut 0, &mut 0.0, 1.0, &mut out, 1, f64::from);
        assert_eq!(out, [0.0, 0.0]);
    }
}
