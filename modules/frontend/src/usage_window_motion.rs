//! Pure motion policy for the usage-window meter.
//!
//! This is the dependency-light Rust counterpart of
//! `modules/frontend/src/lib/identity/usage-window-motion.ts`. The browser
//! adapter remains responsible for reading computed CSS values and the media
//! query; this module only interprets already-obtained token text and the
//! reduced-motion decision. It owns no timer, tween, browser, or renderer.

/// The documented smooth-out CSS curve used when the computed token is absent
/// or invalid.
pub const DEFAULT_MOTION_EASING: CubicBezier = CubicBezier::new(0.22, 1.0, 0.36, 1.0);

/// The duration returned for a non-empty token whose numeric prefix is not
/// parseable, matching the legacy motion policy's 250 ms fallback.
pub const INVALID_DURATION_MILLISECONDS: f64 = 250.0;

const NEWTON_ITERATIONS: usize = 8;
const BISECTION_TOLERANCE: f64 = 1e-5;
const MAX_BISECTION_ITERATIONS: usize = 32;
const RUN_UP_FRACTION: f64 = 0.08;

/// Returns the nonnegative starting value for a usage-window reading.
///
/// The first reading starts eight percent below its target, with at least a
/// one-unit gap, so the value is legible during the short tween instead of
/// appearing to count up from zero. This is the Rust equivalent of the
/// legacy `Math.max(0, target - Math.max(1, Math.round(target * 0.08)))`.
#[must_use]
pub fn run_up_from(target: f64) -> f64 {
    (target - (target * RUN_UP_FRACTION).round().max(1.0)).max(0.0)
}

/// A CSS cubic-bezier timing function's four control points.
///
/// `x1` and `x2` are expected to be in `0..=1` for a CSS token. The
/// constructor is intentionally infallible so callers can exercise the
/// numerical solver with a degenerate but finite curve; strict token parsing
/// performs the CSS x-coordinate validation before constructing one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CubicBezier {
    /// First control point's x coordinate.
    pub x1: f64,
    /// First control point's y coordinate.
    pub y1: f64,
    /// Second control point's x coordinate.
    pub x2: f64,
    /// Second control point's y coordinate.
    pub y2: f64,
}

impl CubicBezier {
    /// Creates a cubic-bezier timing function from its control points.
    #[must_use]
    pub const fn new(x1: f64, y1: f64, x2: f64, y2: f64) -> Self {
        Self { x1, y1, x2, y2 }
    }

    /// Evaluates the curve at normalized input `x`.
    ///
    /// Inputs at or outside either endpoint are returned unchanged, matching
    /// the legacy evaluator's endpoint guard. Interior inputs first use the
    /// eight-step Newton solve for the parametric x coordinate. A flat slope,
    /// non-finite Newton step, or out-of-range final iterate selects the
    /// bounded bisection fallback instead.
    #[must_use]
    pub fn sample(self, x: f64) -> f64 {
        if !x.is_finite() || x <= 0.0 || x >= 1.0 {
            return x;
        }

        Self::sample_curve(self.solve_t(x), self.y1, self.y2)
    }

    /// Alias for [`Self::sample`] when the caller wants the operation named as
    /// an evaluation rather than a sample.
    #[must_use]
    pub fn evaluate(self, x: f64) -> f64 {
        self.sample(x)
    }

    fn solve_t(self, x: f64) -> f64 {
        let mut guess = x;
        let mut use_bisection = false;

        for _ in 0..NEWTON_ITERATIONS {
            let slope = Self::slope(guess, self.x1, self.x2);
            if !slope.is_finite() || slope.abs() <= f64::EPSILON {
                use_bisection = true;
                break;
            }

            let next = guess - (Self::sample_curve(guess, self.x1, self.x2) - x) / slope;
            if !next.is_finite() {
                use_bisection = true;
                break;
            }
            guess = next;
        }

        if !use_bisection && (0.0..=1.0).contains(&guess) {
            return guess;
        }

        let mut low = 0.0;
        let mut high = 1.0;
        let mut mid = x;
        let mut iterations = 0;

        while high - low > BISECTION_TOLERANCE && iterations < MAX_BISECTION_ITERATIONS {
            mid = (low + high) / 2.0;
            if Self::sample_curve(mid, self.x1, self.x2) < x {
                low = mid;
            } else {
                high = mid;
            }
            iterations += 1;
        }

        mid
    }

    fn sample_curve(t: f64, first: f64, second: f64) -> f64 {
        let curve_a = 1.0 - 3.0 * second + 3.0 * first;
        let curve_b = 3.0 * second - 6.0 * first;
        let curve_c = 3.0 * first;
        ((curve_a * t + curve_b) * t + curve_c) * t
    }

    fn slope(t: f64, first: f64, second: f64) -> f64 {
        let curve_a = 1.0 - 3.0 * second + 3.0 * first;
        let curve_b = 3.0 * second - 6.0 * first;
        let curve_c = 3.0 * first;
        3.0 * curve_a * t * t + 2.0 * curve_b * t + curve_c
    }
}

/// Constructs a cubic-bezier evaluator from its four control points.
#[must_use]
pub const fn cubic_bezier(x1: f64, y1: f64, x2: f64, y2: f64) -> CubicBezier {
    CubicBezier::new(x1, y1, x2, y2)
}

/// Borrowed CSS token input for the usage-window easing policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MotionEasingInput<'a> {
    /// The already-read value of `--ease-smooth-out`, if one was available.
    pub css_token: Option<&'a str>,
}

impl<'a> MotionEasingInput<'a> {
    /// Creates easing input without allocating or consulting a browser.
    #[must_use]
    pub const fn new(css_token: Option<&'a str>) -> Self {
        Self { css_token }
    }
}

/// Parses one strict `cubic-bezier(...)` CSS token.
///
/// The whole trimmed input must be exactly one lowercase function token with
/// four comma-separated finite numbers. CSS permits any finite y control
/// point, while x control points must be in `0..=1`; malformed syntax or an
/// invalid x coordinate returns `None` so the caller can use the documented
/// fallback.
#[must_use]
pub fn parse_cubic_bezier_token(token: &str) -> Option<CubicBezier> {
    let token = token.trim();
    let arguments = token.strip_prefix("cubic-bezier(")?.strip_suffix(')')?;
    if arguments.is_empty() {
        return None;
    }

    let mut parts = arguments.split(',');
    let x1 = parse_finite_number(parts.next()?)?;
    let y1 = parse_finite_number(parts.next()?)?;
    let x2 = parse_finite_number(parts.next()?)?;
    let y2 = parse_finite_number(parts.next()?)?;
    if parts.next().is_some() || !(0.0..=1.0).contains(&x1) || !(0.0..=1.0).contains(&x2) {
        return None;
    }

    Some(CubicBezier::new(x1, y1, x2, y2))
}

/// Resolves the usage-window easing token, falling back to the documented
/// `(0.22, 1, 0.36, 1)` curve for absent or invalid input.
#[must_use]
pub fn motion_easing(input: MotionEasingInput<'_>) -> CubicBezier {
    input
        .css_token
        .and_then(parse_cubic_bezier_token)
        .unwrap_or(DEFAULT_MOTION_EASING)
}

/// Borrowed CSS token and reduced-motion input for the usage-window duration
/// policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MotionDurationInput<'a> {
    /// The already-read value of `--duration-fast`, if one was available.
    pub css_token: Option<&'a str>,
    /// Whether `(prefers-reduced-motion: reduce)` matched.
    pub reduced_motion: bool,
}

impl<'a> MotionDurationInput<'a> {
    /// Creates duration input without allocating or consulting a browser.
    #[must_use]
    pub const fn new(css_token: Option<&'a str>, reduced_motion: bool) -> Self {
        Self {
            css_token,
            reduced_motion,
        }
    }
}

/// Resolves a duration token to milliseconds.
///
/// Missing or empty tokens, and reduced motion, resolve to zero. The legacy
/// policy parses the numeric prefix, treats a token ending in lowercase `ms`
/// as milliseconds, and treats every other parseable suffix as seconds. An
/// unparseable non-empty token returns [`INVALID_DURATION_MILLISECONDS`].
#[must_use]
pub fn motion_duration(input: MotionDurationInput<'_>) -> f64 {
    if input.reduced_motion {
        return 0.0;
    }

    let Some(token) = input.css_token else {
        return 0.0;
    };
    let token = token.trim();
    if token.is_empty() {
        return 0.0;
    }

    let Some(parsed) = parse_float_prefix(token) else {
        return INVALID_DURATION_MILLISECONDS;
    };

    if token.ends_with("ms") {
        parsed
    } else {
        parsed * 1000.0
    }
}

fn parse_finite_number(input: &str) -> Option<f64> {
    let value = input.trim().parse::<f64>().ok()?;
    if value.is_finite() { Some(value) } else { None }
}

fn parse_float_prefix(input: &str) -> Option<f64> {
    let input = input.trim_start();
    let bytes = input.as_bytes();
    let mut index = 0;

    if matches!(bytes.get(index), Some(b'+' | b'-')) {
        index += 1;
    }

    if input
        .get(index..)
        .is_some_and(|remaining| remaining.starts_with("Infinity"))
    {
        let end = index + "Infinity".len();
        return input[..end].parse::<f64>().ok();
    }

    let integer_start = index;
    while bytes.get(index).is_some_and(|byte| byte.is_ascii_digit()) {
        index += 1;
    }
    let mut has_digit = index != integer_start;

    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let fraction_start = index;
        while bytes.get(index).is_some_and(|byte| byte.is_ascii_digit()) {
            index += 1;
        }
        has_digit |= index != fraction_start;
    }

    if !has_digit {
        return None;
    }

    let exponent_start = index;
    if matches!(bytes.get(index), Some(b'e' | b'E')) {
        let mut exponent_index = index + 1;
        if matches!(bytes.get(exponent_index), Some(b'+' | b'-')) {
            exponent_index += 1;
        }
        let exponent_digits_start = exponent_index;
        while bytes
            .get(exponent_index)
            .is_some_and(|byte| byte.is_ascii_digit())
        {
            exponent_index += 1;
        }
        if exponent_index != exponent_digits_start {
            index = exponent_index;
        } else {
            index = exponent_start;
        }
    }

    input[..index].parse::<f64>().ok()
}
