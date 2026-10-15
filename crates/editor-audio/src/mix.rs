//! Rendering decoded audio into an output buffer: linear resampling and
//! channel mapping.
//!
//! `PulseAudio` streams at the track's native rate and layout, so it renders
//! with a step of 1 (a plain copy). `cpal` opens the device's own format, so
//! there the step is `track rate / device rate` and channels are mapped.

use crate::decode::Audio;

/// Renders `audio` from frame `*cursor` into the interleaved `out` buffer of
/// `out_channels`, advancing the read position by `step` source frames per
/// output frame (`*phase` carries the fractional part between calls).
///
/// Mono feeds the first two output channels, stereo is averaged for a mono
/// device, and output channels beyond stereo stay silent. Pads with
/// silence past the end and returns whether the track's last frame was read.
#[cfg(test)]
pub(crate) fn render(
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

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "samples narrow to f32 by design, and the whole-frame advance is a small non-negative count"
)]
pub(crate) fn render_mapped<T: Copy>(
    audio: &Audio,
    cursor: &mut usize,
    phase: &mut f64,
    step: f64,
    out: &mut [T],
    out_channels: usize,
    convert: impl Fn(f32) -> T,
) -> bool {
    let frames = audio.frames();
    let channels = audio.channels;
    for frame in out.chunks_mut(out_channels.max(1)) {
        if *cursor >= frames {
            frame.fill(convert(0.0));
            continue;
        }
        let next = (*cursor + 1).min(frames - 1);
        let at = |frame: usize, channel: usize| {
            f64::from(audio.samples[frame * channels + channel.min(channels - 1)])
        };
        let sample = |channel: usize| {
            let mixed = (at(*cursor, channel) * (1.0 - *phase) + at(next, channel) * *phase) as f32;
            if mixed.is_finite() { mixed } else { 0.0 }
        };
        let (left, right) = (sample(0), sample(1));
        match frame {
            [only] => {
                *only = convert(if channels == 1 {
                    left
                } else {
                    f32::midpoint(left, right)
                })
            }
            [first, second, rest @ ..] => {
                *first = convert(left);
                *second = convert(right);
                rest.fill(convert(0.0));
            }
            [] => {}
        }
        *phase += step;
        let whole = phase.floor();
        *phase -= whole;
        *cursor = cursor.saturating_add(whole as usize).min(frames);
    }
    *cursor >= frames
}

#[cfg(test)]
#[expect(clippy::float_cmp, reason = "copied samples are bit-exact")]
mod tests {
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
}

#[cfg(test)]
mod invalid_float_output_tests {
    use super::*;
    #[test]
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
