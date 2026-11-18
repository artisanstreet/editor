use super::{
    ComposerAction, ComposerFailure, QUESTIONNAIRE_MAX_HEIGHT, draft_error, questionnaire,
};
use crate::composer::questionnaire::{ComposerQuestionnaire, QuestionnairePanel};
use aqua_core::{InputEvent, Key, KeyEvent, Modifiers, NamedKey};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::{Box as UiBox, Element};
use artisan_api::{Collection, ItemId, Question, QuestionAnswer, QuestionOption};
use editor_ui::theme::ArtisanTheme;
use std::{cell::RefCell, rc::Rc};

struct Mounted {
    app: aqua::UiApp,
    actions: Rc<RefCell<Vec<ComposerAction>>>,
}

impl Mounted {
    fn new(build: impl FnOnce(super::Dispatch) -> Element) -> Self {
        let actions = Rc::new(RefCell::new(Vec::new()));
        let sink = Rc::clone(&actions);
        let element = build(Rc::new(move |action| sink.borrow_mut().push(action)));
        let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
        app.set_viewport(420.0, 600.0, 1.0);
        app.mount(|_| {
            UiBox::new()
                .layout("column")
                .width(420.0)
                .height(600.0)
                .child(element)
                .child(
                    UiBox::new()
                        .width(420.0)
                        .height(48.0)
                        .build()
                        .with_key("prompt"),
                )
                .build()
        })
        .unwrap();
        app.build_frame().unwrap();
        Self { app, actions }
    }

    fn said(&self) -> Vec<String> {
        self.app
            .tree()
            .traverse()
            .into_iter()
            .filter_map(|node| match &self.app.tree().get(node)?.kind {
                aqua_ui::tree::NodeKind::Text(text) => Some(text.text().to_owned()),
                _ => None,
            })
            .collect()
    }

    fn press(&mut self, key: &str) -> Vec<ComposerAction> {
        assert!(self.app.focus_key(key), "{key} is a focusable control");
        let enter = KeyEvent {
            key: Key::Named(NamedKey::Enter),
            modifiers: Modifiers::default(),
            repeat: false,
        };
        self.app.handle_input(&InputEvent::KeyDown(enter));
        self.app.handle_input(&InputEvent::KeyUp(enter));
        std::mem::take(&mut *self.actions.borrow_mut())
    }
}

fn question(id: &str, options: usize) -> Question {
    Question {
        id: id.into(),
        header: Some(format!("Header {id}")),
        text: format!("Question {id}?").into(),
        multi_select: false,
        options: Collection::Inline(
            (0..options)
                .map(|index| QuestionOption {
                    id: format!("{id}-{index}"),
                    label: format!("Option {index}"),
                    description: None,
                })
                .collect(),
        ),
    }
}

fn panel(questionnaires: Vec<Vec<Question>>) -> QuestionnairePanel {
    let mut panel = QuestionnairePanel::default();
    panel.set_questionnaires(
        questionnaires
            .into_iter()
            .enumerate()
            .map(|(index, questions)| ComposerQuestionnaire {
                item: ItemId(format!("item-{index}")),
                questions: Collection::Inline(questions),
            })
            .collect(),
        &mut String::new(),
    );
    panel
}

#[test]
fn a_question_that_failed_to_load_can_still_be_dismissed_or_switched() {
    let panel = panel(vec![vec![question("a", 2)], vec![question("b", 2)]]);
    let mut mounted = Mounted::new(|dispatch| {
        questionnaire(
            ArtisanTheme::resolved(),
            &panel,
            None,
            None,
            Some("Couldn't load the question."),
            None,
            dispatch,
        )
        .unwrap()
    });
    let said = mounted.said();
    assert!(
        said.iter()
            .any(|text| text == "Couldn't load the question.")
    );
    assert!(said.iter().any(|text| text == "Retry"));
    assert!(said.iter().any(|text| text == "Question b?"));
    assert_eq!(
        mounted.press("composer-question-dismiss"),
        [ComposerAction::DismissQuestionnaire]
    );
    assert_eq!(
        mounted.press("composer-questionnaire-0"),
        [ComposerAction::ActivateQuestionnaire(ItemId(
            "item-1".into()
        ))]
    );
}

#[test]
fn back_returns_to_the_previous_question_with_its_choice_restored() {
    let first = question("a", 2);
    let second = question("b", 2);
    let mut panel = panel(vec![vec![first.clone(), second.clone()]]);
    let mut text = String::new();
    let answer = QuestionAnswer::Choices {
        ids: Collection::Inline(vec!["a-1".into()]),
    };
    assert!(panel.answer(answer, true, &mut text).is_none());
    let mut mounted = Mounted::new(|dispatch| {
        questionnaire(
            ArtisanTheme::resolved(),
            &panel,
            Some(&second),
            None,
            None,
            None,
            dispatch,
        )
        .unwrap()
    });
    assert!(mounted.said().iter().any(|text| text == "Question b?"));
    assert_eq!(
        mounted.press("composer-question-back"),
        [ComposerAction::QuestionBack]
    );
    assert!(panel.back(&mut text));
    assert_eq!(
        (panel.step, panel.ticked.as_slice()),
        (0, ["a-1".to_owned()].as_slice())
    );
    let first_shown = Mounted::new(|dispatch| {
        questionnaire(
            ArtisanTheme::resolved(),
            &panel,
            Some(&first),
            None,
            None,
            None,
            dispatch,
        )
        .unwrap()
    });
    let said = first_shown.said();
    assert!(said.iter().any(|text| text == "Question a?"));
    assert!(!said.iter().any(|text| text == "Back"));
}

#[test]
fn a_long_questionnaire_scrolls_and_leaves_the_prompt_on_screen() {
    let long = question("a", 40);
    let panel = panel(vec![vec![long.clone()]]);
    let mounted = Mounted::new(|dispatch| {
        questionnaire(
            ArtisanTheme::resolved(),
            &panel,
            Some(&long),
            None,
            None,
            None,
            dispatch,
        )
        .unwrap()
    });
    let tree = mounted.app.tree();
    let prompt = tree
        .sole_node_with_key(aqua_ui::tree::hash_key(aqua_ui::element::Key::from(
            "prompt",
        )))
        .unwrap();
    let prompt = tree.rect(prompt).unwrap();
    assert!(
        prompt.y + prompt.h <= 600.0,
        "the prompt stays inside the window under a long questionnaire"
    );
    let card = tree
        .sole_node_with_key(aqua_ui::tree::hash_key(aqua_ui::element::Key::from(
            "composer-questionnaire",
        )))
        .unwrap();
    assert!(tree.rect(card).unwrap().h <= QUESTIONNAIRE_MAX_HEIGHT);
}

#[test]
fn a_composer_notice_explains_itself_and_can_be_dismissed() {
    let mut mounted = Mounted::new(|dispatch| {
        draft_error(
            ArtisanTheme::resolved(),
            ComposerFailure::Notice {
                message: "A message can contain at most 8 images.",
            },
            dispatch,
        )
    });
    assert!(
        mounted
            .said()
            .iter()
            .any(|text| text == "A message can contain at most 8 images.")
    );
    assert_eq!(
        mounted.press("notice-dismiss"),
        [ComposerAction::DismissNotice]
    );
}
