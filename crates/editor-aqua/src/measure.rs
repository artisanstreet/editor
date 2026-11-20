#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "window sizes, scale factors and animation values sit far inside f32's range, and layout needs no more than f32 precision"
)]
pub(crate) const fn narrow(value: f64) -> f32 {
    value as f32
}

#[must_use]
#[expect(
    clippy::cast_precision_loss,
    reason = "pixel, row and tile counts stay below 2^24, where every integer is an exact f32"
)]
pub(crate) const fn exact(value: u32) -> f32 {
    value as f32
}

#[must_use]
#[expect(
    clippy::cast_precision_loss,
    reason = "glyph and row indices stay below 2^52, where every integer is an exact f64"
)]
pub(crate) const fn index(value: usize) -> f64 {
    value as f64
}

#[must_use]
#[expect(
    clippy::cast_precision_loss,
    reason = "token counts stay below 2^52, where every integer is an exact f64"
)]
pub(crate) const fn ratio(part: u64, whole: u64) -> f64 {
    part as f64 / whole as f64
}

#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "float-to-integer casts saturate, so negative and NaN pixel counts become 0 and huge ones u32::MAX"
)]
pub(crate) const fn saturate(value: f32) -> u32 {
    value as u32
}

#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "float-to-integer casts saturate, so a negative or NaN row becomes 0"
)]
pub(crate) const fn saturate_row(value: f64) -> usize {
    value as usize
}

#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "float-to-integer casts saturate, so a negative or NaN tick becomes 0"
)]
pub(crate) const fn saturate_tick(value: f64) -> u32 {
    value as u32
}

#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "float-to-integer casts saturate, so an alpha outside 0..=255 clamps to the nearest channel value"
)]
pub(crate) const fn channel(value: f32) -> u8 {
    value as u8
}

#[cfg(all(feature = "capture", windows))]
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "float-to-integer casts saturate, so a negative or NaN pixel extent becomes 0"
)]
pub(crate) const fn saturate_pixels(value: f64) -> u32 {
    value as u32
}

#[cfg(all(windows, feature = "capture"))]
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "float-to-integer casts saturate after rounding, so a negative or NaN duration becomes 0 ms"
)]
pub(crate) fn whole_millis(value: f32) -> u64 {
    value.round() as u64
}
