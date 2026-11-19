pub const TOLERANCE: f64 = 1e-3;

#[track_caller]
pub fn check_near(actual: f64, expected: f64, message: std::fmt::Arguments<'_>) {
    assert!(
        (actual - expected).abs() <= TOLERANCE,
        "{actual} is not within {TOLERANCE} of {expected}: {message}"
    );
}

#[macro_export]
macro_rules! assert_near {
    ($actual:expr, $expected:expr $(,)?) => {
        $crate::float::check_near(
            f64::from($actual),
            f64::from($expected),
            format_args!("{} vs {}", stringify!($actual), stringify!($expected)),
        )
    };
    ($actual:expr, $expected:expr, $($message:tt)+) => {
        $crate::float::check_near(
            f64::from($actual),
            f64::from($expected),
            format_args!($($message)+),
        )
    };
}

#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "test fixtures narrow window sizes and scale factors that f32 holds exactly"
)]
pub const fn narrow(value: f64) -> f32 {
    value as f32
}

#[must_use]
#[expect(
    clippy::cast_precision_loss,
    reason = "test pixel and error counts stay below 2^52, where every integer is an exact f64"
)]
pub const fn ratio(part: u64, whole: u64) -> f64 {
    part as f64 / whole as f64
}
