//! Focused, dependency-free coverage for untrusted math admission.

#![forbid(unsafe_code)]

#[path = "../../modules/frontend/src/math_rendering_policy.rs"]
mod math_rendering_policy;

use math_rendering_policy::{
    MATH_MAX_EXPANSION, MATH_MAX_SIZE, MATH_RENDERER_CONSTRAINTS, MAX_MATH_SOURCE_UTF16_CODE_UNITS,
    MathRenderAdmission, MathRenderMode, MathRenderOutput, MathRenderRejection,
    MathRenderStrictness, admit_math_render, math_renderer_constraints,
    math_source_utf16_code_units,
};

fn admitted(source: &str, mode: MathRenderMode) -> math_rendering_policy::MathRenderRequest<'_> {
    match admit_math_render(source, mode) {
        MathRenderAdmission::Admitted(request) => request,
        MathRenderAdmission::Rejected(rejection) => {
            panic!("expected admission, got {rejection:?}")
        }
    }
}

#[test]
fn ascii_boundary_is_inclusive_at_16384_utf16_units() {
    for (length, should_admit) in [(16_383, true), (16_384, true), (16_385, false)] {
        let source = "x".repeat(length);
        assert_eq!(math_source_utf16_code_units(&source), length);

        match admit_math_render(&source, MathRenderMode::Inline) {
            MathRenderAdmission::Admitted(request) => {
                assert!(should_admit, "source of {length} units was admitted");
                assert_eq!(request.source(), source);
                assert_eq!(request.mode(), MathRenderMode::Inline);
            }
            MathRenderAdmission::Rejected(rejection) => {
                assert!(!should_admit, "source of {length} units was rejected");
                assert_eq!(
                    rejection,
                    MathRenderRejection::SourceTooLong {
                        source_utf16_code_units: length,
                        maximum_utf16_code_units: MAX_MATH_SOURCE_UTF16_CODE_UNITS,
                    }
                );
            }
        }
    }
}

#[test]
fn astral_scalars_count_as_two_utf16_units_not_one_scalar_or_four_bytes() {
    let astral = "😀";

    assert_eq!(astral.len(), 4, "the UTF-8 representation has four bytes");
    assert_eq!(astral.chars().count(), 1, "the source has one scalar value");
    assert_eq!(math_source_utf16_code_units(astral), 2);

    let below = format!("{}a", astral.repeat(8_191));
    let exact = format!("{}aa", astral.repeat(8_191));
    let above = format!("{}a", astral.repeat(8_192));

    assert_eq!(math_source_utf16_code_units(&below), 16_383);
    assert_eq!(math_source_utf16_code_units(&exact), 16_384);
    assert_eq!(math_source_utf16_code_units(&above), 16_385);

    assert!(matches!(
        admit_math_render(&below, MathRenderMode::Inline),
        MathRenderAdmission::Admitted(_)
    ));
    assert!(matches!(
        admit_math_render(&exact, MathRenderMode::Display),
        MathRenderAdmission::Admitted(_)
    ));
    assert!(matches!(
        admit_math_render(&above, MathRenderMode::Display),
        MathRenderAdmission::Rejected(MathRenderRejection::SourceTooLong { .. })
    ));
}

#[test]
fn mode_is_preserved_as_an_explicit_inline_or_display_input() {
    let inline = admitted("x", MathRenderMode::Inline);
    let display = admitted("x", MathRenderMode::Display);

    assert_eq!(inline.mode(), MathRenderMode::Inline);
    assert!(inline.mode().is_inline());
    assert!(!inline.mode().is_display());
    assert_eq!(display.mode(), MathRenderMode::Display);
    assert!(!display.mode().is_inline());
    assert!(display.mode().is_display());
    assert_eq!(
        MathRenderMode::from_display_mode(false),
        MathRenderMode::Inline
    );
    assert_eq!(
        MathRenderMode::from_display_mode(true),
        MathRenderMode::Display
    );
}

#[test]
fn exact_renderer_constraints_are_exposed_without_a_renderer() {
    let constraints = math_renderer_constraints();

    assert_eq!(constraints, MATH_RENDERER_CONSTRAINTS);
    assert_eq!(constraints.max_expansion(), MATH_MAX_EXPANSION);
    assert_eq!(constraints.max_expansion(), 1_000);
    assert_eq!(constraints.max_size(), MATH_MAX_SIZE);
    assert_eq!(constraints.max_size(), 20);
    assert_eq!(constraints.output(), MathRenderOutput::HtmlAndMathml);
    assert_eq!(constraints.strictness(), MathRenderStrictness::Warn);
    assert!(!constraints.throw_on_error());
    assert!(!constraints.trust());
}

#[test]
fn admitted_request_carries_the_fixed_constraints_and_borrows_source() {
    let source = String::from(r"\frac{1}{2}");
    let request = admitted(&source, MathRenderMode::Display);

    assert_eq!(request.source(), source);
    assert_eq!(request.source().as_ptr(), source.as_ptr());
    assert_eq!(request.mode(), MathRenderMode::Display);
    assert_eq!(request.constraints(), MATH_RENDERER_CONSTRAINTS);
}

#[test]
fn over_limit_rejection_reports_utf16_measurement_and_no_request() {
    let source = "x".repeat(MAX_MATH_SOURCE_UTF16_CODE_UNITS + 1);

    let decision = admit_math_render(&source, MathRenderMode::Inline);
    let MathRenderAdmission::Rejected(rejection) = decision else {
        panic!("over-limit source must be rejected");
    };

    assert_eq!(
        rejection,
        MathRenderRejection::SourceTooLong {
            source_utf16_code_units: MAX_MATH_SOURCE_UTF16_CODE_UNITS + 1,
            maximum_utf16_code_units: MAX_MATH_SOURCE_UTF16_CODE_UNITS,
        }
    );
    assert_eq!(
        rejection.source_utf16_code_units(),
        MAX_MATH_SOURCE_UTF16_CODE_UNITS + 1
    );
    assert_eq!(
        rejection.maximum_utf16_code_units(),
        MAX_MATH_SOURCE_UTF16_CODE_UNITS
    );
}

#[test]
fn exactly_maximum_astral_source_is_admitted() {
    let source = "𐀀".repeat(MAX_MATH_SOURCE_UTF16_CODE_UNITS / 2);

    assert_eq!(source.chars().count(), MAX_MATH_SOURCE_UTF16_CODE_UNITS / 2);
    assert_eq!(
        math_source_utf16_code_units(&source),
        MAX_MATH_SOURCE_UTF16_CODE_UNITS
    );
    let request = admitted(&source, MathRenderMode::Inline);
    assert_eq!(request.source(), source);
}
