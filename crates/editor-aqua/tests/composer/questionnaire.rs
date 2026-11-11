use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::{Box as UiBox, Key};
use aqua_ui::style::Length;
use artisan_api::{Collection, ItemId, Question, QuestionOption};
use editor_aqua::composer::questionnaire::{ComposerQuestionnaire, QuestionnairePanel};
use editor_aqua::composer::view::questionnaire;
use editor_aqua::ui::theme::ArtisanTheme;
use std::rc::Rc;

#[test]
fn a_long_option_description_wraps_inside_the_card() {
    let option = |id: &str, description: &str| QuestionOption {
        id: id.into(),
        label: format!("Option {id}"),
        description: Some(description.into()),
    };
    let asked = Question {
        id: "growth".into(),
        header: Some("Expansion".into()),
        text: "How should the asylum grow?".into(),
        multi_select: false,
        options: Collection::Inline(vec![
            option("short", "Placed by hand."),
            option(
                "long",
                "Keep the current plan's rooms as landmarks, and generate repeated ward wings, \
                 side corridors and service passages off a main hall, built from data so it is \
                 quick to resize or reroll.",
            ),
        ]),
    };
    let mut panel = QuestionnairePanel::default();
    panel.set_questionnaires(
        vec![ComposerQuestionnaire {
            item: ItemId("question".into()),
            questions: Collection::Inline(vec![asked.clone()]),
        }],
        &mut String::new(),
    );
    let card = questionnaire(
        ArtisanTheme::resolved(),
        &panel,
        Some(&asked),
        None,
        None,
        None,
        Rc::new(|_| {}),
    )
    .unwrap();
    let fonts = SharedFonts::new();
    editor_aqua::ui::fonts::register_bundled_fonts(&fonts);
    let mut app = aqua::UiApp::new(fonts, SharedRuns::new());
    app.set_viewport(1200.0, 600.0, 1.0);
    app.mount(|_| {
        UiBox::new()
            .layout("column")
            .width(Length::Px(420.0))
            .child(card)
            .build()
            .with_key("card")
    })
    .unwrap();
    app.layout().unwrap();
    let rect = |key: &str| {
        let key = aqua_ui::tree::hash_key(Key::from(key));
        let node = app
            .tree()
            .traverse()
            .into_iter()
            .find(|node| {
                app.tree()
                    .get(*node)
                    .is_some_and(|record| record.key == Some(key))
            })
            .unwrap();
        app.tree().rect(node).unwrap()
    };
    let card = rect("card");
    let inside = app.tree().traverse().into_iter().all(|node| {
        app.tree()
            .rect(node)
            .is_none_or(|rect| rect.x + rect.w <= card.x + card.w + 0.5)
    });
    assert!(inside, "nothing runs past the card's right edge");
    let (short, long) = (
        rect("composer-question-option-0"),
        rect("composer-question-option-1"),
    );
    assert!(
        long.h > short.h + 15.0,
        "the long description takes more lines: {long:?} against {short:?}"
    );
}
