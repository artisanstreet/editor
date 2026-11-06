use artisan_api::{Collection, ItemId, Question, QuestionAnswer, TextContent};
use editor_aqua::composer::questionnaire::{ComposerQuestionnaire, QuestionnairePanel};
use editor_aqua::conversation::questions::QuestionPages;

fn question(id: &str) -> Question {
    Question {
        id: id.to_owned(),
        header: None,
        text: TextContent::Inline(id.to_owned()),
        multi_select: false,
        options: Collection::Inline(Vec::new()),
    }
}

#[test]
fn current_question_tracks_active_questionnaire_and_step() {
    let mut panel = QuestionnairePanel::default();
    let mut text = String::new();
    panel.set_questionnaires(
        vec![
            ComposerQuestionnaire {
                item: ItemId("first".into()),
                questions: vec![question("one"), question("two")].into(),
            },
            ComposerQuestionnaire {
                item: ItemId("second".into()),
                questions: vec![question("three")].into(),
            },
        ],
        &mut text,
    );
    let pages = QuestionPages::default();
    assert_eq!(pages.current_question(&panel).unwrap().id, "one");
    panel.answer(
        QuestionAnswer::Text {
            text: "answer".into(),
        },
        true,
        &mut text,
    );
    assert_eq!(pages.current_question(&panel).unwrap().id, "two");
    panel.activate(ItemId("second".into()), &mut text);
    assert_eq!(pages.current_question(&panel).unwrap().id, "three");
    panel.activate(ItemId("first".into()), &mut text);
    assert_eq!(pages.current_question(&panel).unwrap().id, "two");
}
