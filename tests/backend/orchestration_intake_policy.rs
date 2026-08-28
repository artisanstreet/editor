//! Focused tests for the dependency-free orchestration intake policy.

#![allow(dead_code)]

#[path = "../../modules/backend/src/orchestration_intake_policy.rs"]
mod orchestration_intake_policy;

use std::str::FromStr;

use orchestration_intake_policy::{
    IntakeAssessment, IntakeAssessmentValidationError, IntakePolicy, IntakeResolution,
    IntakeResolutionParseError, IntakeRisk, IntakeRiskParseError, assess,
    validate_intake_assessment,
};

#[test]
fn risk_vocabulary_has_exact_spellings_and_round_trips() {
    let cases = [
        (IntakeRisk::Low, "low"),
        (IntakeRisk::Material, "material"),
        (IntakeRisk::High, "high"),
        (IntakeRisk::Underspecified, "underspecified"),
    ];

    assert_eq!(IntakeRisk::ALL.as_slice(), &cases.map(|(risk, _)| risk));
    for (risk, spelling) in cases {
        assert_eq!(risk.as_str(), spelling);
        assert_eq!(risk.to_string(), spelling);
        assert_eq!(IntakeRisk::parse(spelling), Ok(risk));
        assert_eq!(spelling.parse::<IntakeRisk>(), Ok(risk));
        assert_eq!(IntakeRisk::from_str(spelling), Ok(risk));
    }
}

#[test]
fn resolution_vocabulary_has_exact_spellings_and_round_trips() {
    let cases = [
        (IntakeResolution::Proceed, "proceed"),
        (IntakeResolution::Question, "question"),
    ];

    assert_eq!(
        IntakeResolution::ALL.as_slice(),
        &cases.map(|(resolution, _)| resolution)
    );
    for (resolution, spelling) in cases {
        assert_eq!(resolution.as_str(), spelling);
        assert_eq!(resolution.to_string(), spelling);
        assert_eq!(IntakeResolution::parse(spelling), Ok(resolution));
        assert_eq!(spelling.parse::<IntakeResolution>(), Ok(resolution));
        assert_eq!(IntakeResolution::from_str(spelling), Ok(resolution));
    }
}

#[test]
fn enum_parsing_rejects_rewritten_spellings() {
    assert_eq!(IntakeRisk::parse("LOW"), Err(IntakeRiskParseError));
    assert_eq!(IntakeRisk::parse(" low"), Err(IntakeRiskParseError));
    assert_eq!(IntakeRisk::parse("unknown"), Err(IntakeRiskParseError));
    assert_eq!(
        IntakeResolution::parse("PROCEED"),
        Err(IntakeResolutionParseError)
    );
    assert_eq!(
        IntakeResolution::parse("question "),
        Err(IntakeResolutionParseError)
    );
    assert_eq!(
        IntakeResolution::parse("unknown"),
        Err(IntakeResolutionParseError)
    );
}

#[test]
fn default_assessment_is_low_proceed_and_empty() {
    let expected = IntakeAssessment {
        risk: IntakeRisk::Low,
        resolution: IntakeResolution::Proceed,
        assumptions: Vec::new(),
        question: None,
    };

    assert_eq!(IntakeAssessment::default(), expected);
    assert_eq!(IntakePolicy::assess("ordinary request"), expected);
    assert_eq!(assess(""), expected);
}

#[test]
fn every_input_uses_the_same_default_without_prose_sniffing() {
    let inputs = [
        "",
        "Please rename the file.",
        "remove the emulator and delete the database immediately",
        "!!! DROP EVERYTHING !!!",
        "你好，🦀",
        "\n\t",
    ];

    for input in inputs {
        let assessment = IntakePolicy::assess(input);
        assert_eq!(assessment.risk, IntakeRisk::Low);
        assert_eq!(assessment.resolution, IntakeResolution::Proceed);
        assert!(assessment.assumptions.is_empty());
        assert_eq!(assessment.question, None);
        assert!(assessment.validate().is_ok());
    }
}

#[test]
fn policy_instance_and_free_function_are_pure_equivalents() {
    let policy = IntakePolicy::new();
    assert_eq!(policy, IntakePolicy);
    assert_eq!(
        IntakePolicy::assess("same input"),
        IntakePolicy::assess("same input")
    );
    assert_eq!(assess("same input"), assess("same input"));
}

#[test]
fn valid_proceed_and_question_assessments_preserve_values() {
    let assumptions = vec![
        "first assumption".to_owned(),
        "second assumption".to_owned(),
    ];
    let proceed = IntakeAssessment::proceed(IntakeRisk::Material, assumptions.clone()).unwrap();
    assert_eq!(proceed.risk(), IntakeRisk::Material);
    assert_eq!(proceed.resolution(), IntakeResolution::Proceed);
    assert_eq!(proceed.assumptions(), assumptions.as_slice());
    assert_eq!(proceed.question_text(), None);
    assert_eq!(proceed.validate(), Ok(()));

    let question = IntakeAssessment::question(
        IntakeRisk::Underspecified,
        assumptions.clone(),
        "Which directory should be changed?".to_owned(),
    )
    .unwrap();
    assert_eq!(question.risk(), IntakeRisk::Underspecified);
    assert_eq!(question.resolution(), IntakeResolution::Question);
    assert_eq!(question.assumptions(), assumptions.as_slice());
    assert_eq!(
        question.question_text(),
        Some("Which directory should be changed?")
    );
    assert_eq!(validate_intake_assessment(&question), Ok(()));
}

#[test]
fn assumptions_are_ordered_and_empty_entries_are_rejected_without_rewriting() {
    let assumptions = vec!["first".to_owned(), String::new(), "third".to_owned()];
    let error = IntakeAssessment::try_new(
        IntakeRisk::Low,
        IntakeResolution::Proceed,
        assumptions.clone(),
        None,
    )
    .unwrap_err();
    assert_eq!(
        error,
        IntakeAssessmentValidationError::EmptyAssumption { index: 1 }
    );
    assert_eq!(error.to_string(), "assumption at index 1 must not be empty");

    let valid = IntakeAssessment::proceed(
        IntakeRisk::Low,
        vec!["first".to_owned(), "second".to_owned(), "third".to_owned()],
    )
    .unwrap();
    assert_eq!(
        valid.assumptions(),
        ["first", "second", "third"].map(String::from).as_slice()
    );
}

#[test]
fn whitespace_is_nonempty_and_is_preserved() {
    let assessment = IntakeAssessment::try_new(
        IntakeRisk::High,
        IntakeResolution::Question,
        vec!["  ".to_owned()],
        Some("\t".to_owned()),
    )
    .unwrap();

    assert_eq!(assessment.assumptions(), ["  ".to_owned()].as_slice());
    assert_eq!(assessment.question_text(), Some("\t"));
}

#[test]
fn question_resolution_requires_a_question() {
    assert_eq!(
        IntakeAssessment::try_new(
            IntakeRisk::Low,
            IntakeResolution::Question,
            Vec::new(),
            None,
        ),
        Err(IntakeAssessmentValidationError::QuestionRequired)
    );
}

#[test]
fn question_resolution_rejects_an_empty_question() {
    assert_eq!(
        IntakeAssessment::try_new(
            IntakeRisk::Low,
            IntakeResolution::Question,
            Vec::new(),
            Some(String::new()),
        ),
        Err(IntakeAssessmentValidationError::EmptyQuestion)
    );
}

#[test]
fn proceed_resolution_cannot_retain_a_question() {
    let assessment = IntakeAssessment {
        question: Some("must not survive proceed".to_owned()),
        ..IntakeAssessment::default()
    };
    assert_eq!(
        assessment.validate(),
        Err(IntakeAssessmentValidationError::QuestionNotAllowed)
    );
    assert_eq!(
        assessment.question.as_deref(),
        Some("must not survive proceed")
    );

    assert_eq!(
        IntakeAssessment::try_new(
            IntakeRisk::Low,
            IntakeResolution::Proceed,
            Vec::new(),
            Some("must not survive proceed".to_owned()),
        ),
        Err(IntakeAssessmentValidationError::QuestionNotAllowed)
    );
}

#[test]
fn externally_assembled_assessments_can_be_validated_without_mutation() {
    let assessment = IntakeAssessment {
        risk: IntakeRisk::High,
        resolution: IntakeResolution::Question,
        assumptions: vec!["retain order".to_owned(), "retain text".to_owned()],
        question: Some("Need a boundary".to_owned()),
    };
    let before = assessment.clone();

    assert_eq!(validate_intake_assessment(&assessment), Ok(()));
    assert_eq!(assessment, before);
}

#[test]
fn validation_reports_assumption_failure_before_later_invariants() {
    let assessment = IntakeAssessment {
        risk: IntakeRisk::Low,
        resolution: IntakeResolution::Question,
        assumptions: vec![String::new()],
        question: None,
    };

    assert_eq!(
        assessment.validate(),
        Err(IntakeAssessmentValidationError::EmptyAssumption { index: 0 })
    );
}
