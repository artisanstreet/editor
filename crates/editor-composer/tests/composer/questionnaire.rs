use super::{ComposerQuestionnaire, QuestionnairePanel};
use artisan_api::{Collection, ItemId, Question};

fn asked(item: &str) -> (ItemId, Collection<Question>) {
    (ItemId(item.into()), Collection::Inline(Vec::new()))
}

#[test]
fn a_changed_question_list_is_not_mistaken_for_the_held_one() {
    let mut panel = QuestionnairePanel::default();
    let first = asked("first");
    let mut text = String::new();
    panel.set_questionnaires(
        vec![ComposerQuestionnaire {
            item: first.0.clone(),
            questions: first.1.clone(),
        }],
        &mut text,
    );
    let second = asked("second");
    assert!(panel.holds(std::iter::once((&first.0, &first.1))));
    assert!(!panel.holds(std::iter::once((&second.0, &second.1))));
    assert!(!panel.holds([(&first.0, &first.1), (&second.0, &second.1)].into_iter()));
    assert!(!panel.holds(std::iter::empty()));
}
