//! Pure admission policy for untrusted conversation math.
//!
//! This is the native counterpart of
//! `modules/frontend/src/lib/components/markdown/math-rendering.ts`. It
//! freezes the source-length and renderer-configuration decisions that a
//! later native renderer must enforce, but it does not parse LaTeX, execute a
//! renderer, or produce HTML.
//!
//! JavaScript's `String.prototype.length` counts UTF-16 code units. Rust's
//! [`str::len`] instead counts UTF-8 bytes, and [`str::chars`] counts Unicode
//! scalar values, so neither is an equivalent boundary check. The admission
//! function uses [`str::encode_utf16`] and counts its code units explicitly:
//! a scalar in the Basic Multilingual Plane contributes one unit, while an
//! astral scalar contributes two units. The 16,384-unit boundary is inclusive.

#![allow(clippy::module_name_repetitions)]
#![forbid(unsafe_code)]

/// Maximum math-source length measured in JavaScript-compatible UTF-16 code
/// units.
pub const MAX_MATH_SOURCE_UTF16_CODE_UNITS: usize = 16_384;

/// Maximum number of macro expansions allowed by the eventual renderer.
pub const MATH_MAX_EXPANSION: u32 = 1_000;

/// Maximum size allowed by the eventual renderer.
pub const MATH_MAX_SIZE: u32 = 20;

/// Whether admitted math is intended for an inline or display context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MathRenderMode {
    /// Math embedded within surrounding prose.
    Inline,
    /// Math rendered as a standalone block.
    Display,
}

impl MathRenderMode {
    /// Converts the legacy renderer's explicit `displayMode` boolean.
    #[must_use]
    pub const fn from_display_mode(display_mode: bool) -> Self {
        if display_mode {
            Self::Display
        } else {
            Self::Inline
        }
    }

    /// Returns the legacy renderer-compatible `displayMode` value.
    #[must_use]
    pub const fn is_display(self) -> bool {
        matches!(self, Self::Display)
    }

    /// Returns whether this request is for inline math.
    #[must_use]
    pub const fn is_inline(self) -> bool {
        matches!(self, Self::Inline)
    }
}

/// Output format required of a later math renderer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MathRenderOutput {
    /// The renderer must provide both HTML and MathML representations.
    HtmlAndMathml,
}

/// Strictness behavior required of a later math renderer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MathRenderStrictness {
    /// Warn about strictness issues without turning them into exceptions.
    Warn,
}

/// Immutable renderer constraints for admitted untrusted math.
///
/// The fields are private so callers cannot construct a request with weaker
/// constraints through this policy. [`MATH_RENDERER_CONSTRAINTS`] is the one
/// fixed configuration, and the accessors expose its values to a later
/// renderer adapter without executing that adapter here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MathRendererConstraints {
    max_expansion: u32,
    max_size: u32,
    output: MathRenderOutput,
    strictness: MathRenderStrictness,
    throw_on_error: bool,
    trust: bool,
}

/// Fixed renderer constraints for all admitted untrusted math.
pub const MATH_RENDERER_CONSTRAINTS: MathRendererConstraints = MathRendererConstraints {
    max_expansion: MATH_MAX_EXPANSION,
    max_size: MATH_MAX_SIZE,
    output: MathRenderOutput::HtmlAndMathml,
    strictness: MathRenderStrictness::Warn,
    throw_on_error: false,
    trust: false,
};

impl MathRendererConstraints {
    /// Returns the maximum expansion count (`maxExpand` in KaTeX).
    #[must_use]
    pub const fn max_expansion(self) -> u32 {
        self.max_expansion
    }

    /// Returns the maximum size (`maxSize` in KaTeX).
    #[must_use]
    pub const fn max_size(self) -> u32 {
        self.max_size
    }

    /// Returns the required combined HTML and MathML output mode.
    #[must_use]
    pub const fn output(self) -> MathRenderOutput {
        self.output
    }

    /// Returns the required strictness behavior.
    #[must_use]
    pub const fn strictness(self) -> MathRenderStrictness {
        self.strictness
    }

    /// Returns whether renderer errors must be raised as exceptions.
    #[must_use]
    pub const fn throw_on_error(self) -> bool {
        self.throw_on_error
    }

    /// Returns whether the renderer may trust input commands or URLs.
    #[must_use]
    pub const fn trust(self) -> bool {
        self.trust
    }
}

/// Returns the fixed constraints that every admitted math request carries.
#[must_use]
pub const fn math_renderer_constraints() -> MathRendererConstraints {
    MATH_RENDERER_CONSTRAINTS
}

/// A renderer request that has passed source-length admission.
///
/// The request borrows the original source and carries the explicit context
/// and immutable renderer constraints needed by a later renderer adapter. It
/// intentionally contains no rendered representation.
#[must_use = "pass admitted requests to a later renderer adapter"]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MathRenderRequest<'source> {
    source: &'source str,
    mode: MathRenderMode,
    constraints: MathRendererConstraints,
}

impl<'source> MathRenderRequest<'source> {
    /// Returns the original, unparsed math source.
    #[must_use]
    pub const fn source(self) -> &'source str {
        self.source
    }

    /// Returns whether the request is inline or display math.
    #[must_use]
    pub const fn mode(self) -> MathRenderMode {
        self.mode
    }

    /// Returns the immutable renderer constraints attached to this request.
    #[must_use]
    pub const fn constraints(self) -> MathRendererConstraints {
        self.constraints
    }
}

/// Why an untrusted math source was rejected before rendering.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MathRenderRejection {
    /// The source exceeded the inclusive UTF-16 code-unit limit.
    SourceTooLong {
        /// Measured source length in JavaScript-compatible UTF-16 code units.
        source_utf16_code_units: usize,
        /// The maximum admitted source length in UTF-16 code units.
        maximum_utf16_code_units: usize,
    },
}

impl MathRenderRejection {
    /// Returns the measured UTF-16 length that caused rejection.
    #[must_use]
    pub const fn source_utf16_code_units(self) -> usize {
        match self {
            Self::SourceTooLong {
                source_utf16_code_units,
                ..
            } => source_utf16_code_units,
        }
    }

    /// Returns the maximum UTF-16 length allowed by the policy.
    #[must_use]
    pub const fn maximum_utf16_code_units(self) -> usize {
        match self {
            Self::SourceTooLong {
                maximum_utf16_code_units,
                ..
            } => maximum_utf16_code_units,
        }
    }
}

/// Typed result of attempting to admit a math render request.
#[must_use = "inspect whether the math request was admitted or rejected"]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MathRenderAdmission<'source> {
    /// The source passed the length policy and is ready for a later renderer.
    Admitted(MathRenderRequest<'source>),
    /// The source was rejected before any renderer could be called.
    Rejected(MathRenderRejection),
}

/// Counts the source's JavaScript-compatible UTF-16 code units.
///
/// This is deliberately not [`str::len`] (UTF-8 bytes) or a Unicode scalar
/// count. It is public so callers and boundary tests can inspect the exact
/// unit used by admission.
#[must_use]
pub fn math_source_utf16_code_units(source: &str) -> usize {
    source.encode_utf16().count()
}

/// Admits an untrusted math source for a later renderer.
///
/// Sources of exactly [`MAX_MATH_SOURCE_UTF16_CODE_UNITS`] UTF-16 code units
/// are admitted. Longer sources are rejected before parsing or rendering.
/// The explicit [`MathRenderMode`] preserves the inline/display decision, and
/// admitted requests always carry [`MATH_RENDERER_CONSTRAINTS`].
#[must_use = "inspect whether the math request was admitted or rejected"]
pub fn admit_math_render(source: &str, mode: MathRenderMode) -> MathRenderAdmission<'_> {
    let source_utf16_code_units = math_source_utf16_code_units(source);
    if source_utf16_code_units > MAX_MATH_SOURCE_UTF16_CODE_UNITS {
        MathRenderAdmission::Rejected(MathRenderRejection::SourceTooLong {
            source_utf16_code_units,
            maximum_utf16_code_units: MAX_MATH_SOURCE_UTF16_CODE_UNITS,
        })
    } else {
        MathRenderAdmission::Admitted(MathRenderRequest {
            source,
            mode,
            constraints: MATH_RENDERER_CONSTRAINTS,
        })
    }
}
