use super::{Question, QuestionAnswer, QuestionOption};
#[test]
fn duplicate_labels_keep_distinct_choice_identity_and_text_is_not_a_choice() {
    let question = Question {
        id: "q".into(),
        header: None,
        text: "Pick".into(),
        multi_select: false,
        options: vec![
            QuestionOption {
                id: "a".into(),
                label: "Same".into(),
                description: None,
            },
            QuestionOption {
                id: "b".into(),
                label: "Same".into(),
                description: None,
            },
        ]
        .into(),
    };
    let answer = QuestionAnswer::Choices {
        ids: vec!["b".into()].into(),
    };
    question
        .validate_answer(Some(&answer))
        .expect("a known choice id is accepted");
    let encoded = serde_json::to_string(&answer).unwrap();
    assert_eq!(
        serde_json::from_str::<QuestionAnswer>(&encoded).unwrap(),
        answer
    );
    question
        .validate_answer(Some(&QuestionAnswer::Choices {
            ids: vec!["Same".into()].into(),
        }))
        .expect_err("a choice label is not a choice id");
    question
        .validate_answer(Some(&QuestionAnswer::Text {
            text: "Same".into(),
        }))
        .expect("a text answer is accepted");
    question
        .validate_answer(Some(&QuestionAnswer::Choices {
            ids: vec!["a".into(), "b".into()].into(),
        }))
        .expect_err("a single-select question refuses two choices");
    let mut multi = question;
    multi.multi_select = true;
    multi
        .validate_answer(Some(&QuestionAnswer::Choices {
            ids: vec!["a".into(), "a".into()].into(),
        }))
        .expect_err("the same choice twice is refused");
}
