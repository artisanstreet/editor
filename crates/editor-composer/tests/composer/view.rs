use super::model_menu::model_gate_row;
use super::questionnaire::QUESTIONNAIRE_MAX_HEIGHT;
use super::{ComposerAction, ComposerFailure, draft_error, questionnaire};
use crate::composer::picker::gate;
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
                aqua_ui::tree::NodeKind::Box(_)
                | aqua_ui::tree::NodeKind::Media(_)
                | aqua_ui::tree::NodeKind::Scroll(_)
                | aqua_ui::tree::NodeKind::Canvas(_)
                | aqua_ui::tree::NodeKind::Viewport3D(_) => None,
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
            &ArtisanTheme::resolved(),
            &panel,
            None,
            None,
            Some(&editor_core::Problem::new("Couldn't load the question.")),
            None,
            &dispatch,
        )
        .unwrap()
    });
    let said = mounted.said();
    assert!(
        said.iter()
            .any(|text| text == "Couldn't load the question.")
    );
    assert!(said.iter().any(|text| text == "Try again"));
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
            &ArtisanTheme::resolved(),
            &panel,
            Some(&second),
            None,
            None,
            None,
            &dispatch,
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
            &ArtisanTheme::resolved(),
            &panel,
            Some(&first),
            None,
            None,
            None,
            &dispatch,
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
            &ArtisanTheme::resolved(),
            &panel,
            Some(&long),
            None,
            None,
            None,
            &dispatch,
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
            &ArtisanTheme::resolved(),
            ComposerFailure::Notice {
                message: &editor_core::Problem::new("A message can contain at most 8 images."),
            },
            &dispatch,
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

fn engine(sign_in: &str, discovery: &str) -> artisan_api::EngineStatus {
    serde_json::from_value(serde_json::json!({
        "engine": "codex",
        "install": {"state": "installed", "version": "1.0.0", "update": null},
        "sign_in": serde_json::from_str::<serde_json::Value>(sign_in).unwrap(),
        "discovery": serde_json::from_str::<serde_json::Value>(discovery).unwrap(),
        "models": [],
        "default_model": null,
        "usage": null,
    }))
    .unwrap()
}

#[test]
fn an_engine_without_models_offers_its_settings_and_a_signed_out_one_asks_to_sign_in() {
    let theme = ArtisanTheme::resolved();
    let empty = engine(
        r#"{"state":"signed_in","account":null}"#,
        r#"{"state":"fresh","observed_at":0}"#,
    );
    let mut mounted = Mounted::new(|dispatch| {
        model_gate_row(&theme, empty.engine, gate(&empty).unwrap(), &dispatch)
    });
    assert!(
        mounted
            .said()
            .iter()
            .any(|text| text == "Open Codex settings")
    );
    assert_eq!(
        mounted.press("composer-engine-gate-codex"),
        [ComposerAction::OpenEngineSettings(
            artisan_api::EngineId::Codex
        )]
    );
    let signed_out = engine(
        r#"{"state":"signed_out"}"#,
        r#"{"state":"fresh","observed_at":0}"#,
    );
    let mounted = Mounted::new(|dispatch| {
        model_gate_row(
            &theme,
            signed_out.engine,
            gate(&signed_out).unwrap(),
            &dispatch,
        )
    });
    let said = mounted.said();
    assert!(said.iter().any(|text| text == "Sign in to Codex"));
    assert!(!said.iter().any(|text| text.contains("no models")));
}

#[test]
fn an_engine_whose_model_discovery_failed_says_so_and_offers_its_settings() {
    let theme = ArtisanTheme::resolved();
    for discovery in [
        r#"{"state":"unavailable","error":"codex exited with status 1"}"#,
        r#"{"state":"stale","last_success_at":0,"error":"codex exited with status 1"}"#,
    ] {
        let failed = engine(r#"{"state":"signed_in","account":null}"#, discovery);
        let mut mounted = Mounted::new(|dispatch| {
            model_gate_row(&theme, failed.engine, gate(&failed).unwrap(), &dispatch)
        });
        let said = mounted.said();
        assert!(
            said.iter()
                .any(|text| text == "Codex is not ready: model discovery failed."),
            "{discovery}: {said:?}"
        );
        assert!(
            said.iter().any(|text| text == "Open Codex settings"),
            "{discovery}: {said:?}"
        );
        assert_eq!(
            mounted.press("composer-engine-gate-codex"),
            [ComposerAction::OpenEngineSettings(
                artisan_api::EngineId::Codex
            )],
            "{discovery}"
        );
    }
}

#[test]
fn a_gated_engine_explains_itself_on_keyboard_focus_and_to_assistive_tech() {
    use super::model_menu::{engine_reason, engine_tab};
    let signed_out = engine(
        r#"{"state":"signed_out"}"#,
        r#"{"state":"fresh","observed_at":0}"#,
    );
    let reason = engine_reason(&signed_out).expect("a signed-out engine is gated");
    let shown = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&shown);
    let mut mounted = Mounted::new(|dispatch| {
        engine_tab(
            &ArtisanTheme::resolved(),
            signed_out.engine,
            None,
            Some(reason.clone()),
            Rc::new(move |visible| sink.borrow_mut().push(visible)),
            &dispatch,
        )
    });
    let tab = mounted
        .app
        .tree()
        .emit_semantics()
        .into_iter()
        .find(|node| node.label == "Codex")
        .expect("the engine tab is named");
    assert_eq!(tab.description.as_deref(), Some(reason.as_str()));
    let key = KeyEvent {
        key: Key::Named(NamedKey::Tab),
        modifiers: Modifiers::default(),
        repeat: false,
    };
    mounted.app.handle_input(&InputEvent::KeyDown(key));
    mounted.app.handle_input(&InputEvent::KeyUp(key));
    assert_eq!(mounted.app.tree().focused(), Some(tab.node));
    assert_eq!(shown.borrow().as_slice(), &[true]);
}

fn largest_text() -> ArtisanTheme {
    editor_ui::preferences::Preferences {
        theme: editor_ui::theme::ThemeMode::Dark,
        fonts: editor_ui::theme::FontSet::Spline,
        outline_text: true,
        reduced_motion: None,
        high_contrast: None,
        text_scale: Some(2.25),
        streaming: editor_ui::preferences::StreamingMode::Smooth,
        lower_frame_rate: false,
        vsync: true,
        system: editor_ui::preferences::SystemAccess::default(),
    }
    .theme()
}

#[test]
fn a_gated_engine_row_holds_its_reason_and_action_at_the_largest_text_size() {
    let theme = largest_text();
    let signed_out = engine(
        r#"{"state":"signed_out"}"#,
        r#"{"state":"fresh","observed_at":0}"#,
    );
    let mounted = Mounted::new(|dispatch| {
        UiBox::new()
            .layout("column")
            .width(340.0)
            .child(
                model_gate_row(
                    &theme,
                    signed_out.engine,
                    gate(&signed_out).unwrap(),
                    &dispatch,
                )
                .with_key("gate"),
            )
            .build()
    });
    let tree = mounted.app.tree();
    let rect = |key: &str| {
        tree.sole_node_with_key(aqua_ui::tree::hash_key(aqua_ui::element::Key::from(key)))
            .and_then(|node| tree.rect(node))
            .unwrap_or_else(|| panic!("{key} is laid out"))
    };
    let row = rect("gate");
    let action = rect("composer-engine-gate-codex");
    assert!(
        action.y + action.h <= row.y + row.h + 0.5,
        "the sign-in link stays inside its row at 225%: {action:?} in {row:?}"
    );
}

#[test]
fn a_long_project_list_scrolls_to_the_projects_below_the_fold() {
    use crate::composer::picker::{Menu, PickerState};
    use artisan_api::tls::{Identity, fingerprint};
    use editor_core::{ForgeClient, HostConfig, HostId, HostKind};

    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(directory.path());
    let forge = test_support::net::SilentListener::bind();
    let identity = Identity::generate("projects").unwrap();
    let config = HostConfig {
        id: HostId("projects".into()),
        name: "Forge".into(),
        address: forge.address().to_string(),
        fingerprint: fingerprint(identity.certificate_der().unwrap().as_ref()),
        kind: HostKind::Local,
    };
    let client = ForgeClient::new(&config.address, &config.fingerprint, &identity).unwrap();
    test_support::editor::core::connect_client(&mut core, &config, client);
    let host = core.host_mut(&config.id).unwrap();
    for index in 0..14_i64 {
        let id = artisan_api::ProjectId(format!("project-{index}"));
        host.projects.insert(
            id.clone(),
            artisan_api::Project {
                id,
                path: format!("/projects/{index}"),
                name: format!("project {index}"),
                seq: u64::try_from(index).unwrap(),
                created_at: 1_000 - index,
                removed: false,
            },
        );
    }
    let host = core.host(&config.id).unwrap();
    let mut picker = PickerState::default();
    picker.menu = Some(Menu::Project);
    picker.refresh(host, None);
    let mut mounted = Mounted::new(|dispatch| {
        super::project_menu(
            &ArtisanTheme::resolved(),
            host,
            &picker,
            None,
            aqua_core::Size::new(420.0, 600.0),
            &dispatch,
        )
    });
    let scroller = mounted
        .app
        .tree()
        .sole_node_with_key(aqua_ui::tree::hash_key(aqua_ui::element::Key::from(
            super::PROJECT_SCROLL,
        )))
        .unwrap();
    let bounds = mounted.app.tree().rect(scroller).unwrap();
    mounted.app.handle_input(&InputEvent::Scroll {
        at: aqua_core::Point::new(bounds.x + bounds.w / 2.0, bounds.y + bounds.h / 2.0),
        delta: aqua_core::ScrollDelta { x: 0.0, y: 2_000.0 },
    });
    mounted.app.build_frame().unwrap();
    let last = mounted
        .app
        .tree()
        .sole_node_with_key(aqua_ui::tree::hash_key(aqua_ui::element::Key::from(
            "composer-project-project-13",
        )))
        .unwrap();
    let last = mounted.app.tree().visual_rect(last).unwrap();
    assert!(
        last.y >= bounds.y - 0.5 && last.y + last.h <= bounds.y + bounds.h + 0.5,
        "scrolling to the end shows the oldest project inside the list, it sits at {last:?} in a list at {bounds:?}"
    );
}
