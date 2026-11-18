use crate::decode::Audio;

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "samples narrow to f32 by design, and the whole-frame advance is a small non-negative count"
)]
pub fn render_mapped<T: Copy>(
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
            let mixed =
                at(*cursor, channel).mul_add(1.0 - *phase, at(next, channel) * *phase) as f32;
            if mixed.is_finite() { mixed } else { 0.0 }
        };
        let (left, right) = (sample(0), sample(1));
        match frame {
            [only] => {
                *only = convert(if channels == 1 {
                    left
                } else {
                    f32::midpoint(left, right)
                });
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
#[path = "../tests/mix.rs"]
mod tests;
