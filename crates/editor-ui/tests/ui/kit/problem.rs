use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::Element;
use aqua_ui::tree::NodeKind;
use editor_core::Problem;
use editor_ui::kit::{Button, ButtonTone, ProblemNotice};
use editor_ui::theme::ArtisanTheme;

const CHAIN: &str = "load thread t1 from mch_1: GET /threads/t1/transcript: reach Forge: error sending request for url (https://127.0.0.1:47900/threads/t1/transcript?limit=40)";

fn mounted(element: Element) -> aqua::UiApp {
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| element).expect("mount the notice");
    app.set_viewport(480.0, 240.0, 1.0);
    app.build_frame().expect("lay out the notice");
    app
}

fn painted(app: &aqua::UiApp) -> Vec<String> {
    app.tree()
        .traverse()
        .into_iter()
        .filter_map(|node| match &app.tree().get(node)?.kind {
            NodeKind::Text(text) => Some(text.text().to_owned()),
            NodeKind::Box(_)
            | NodeKind::Media(_)
            | NodeKind::Scroll(_)
            | NodeKind::Canvas(_)
            | NodeKind::Viewport3D(_) => None,
        })
        .collect()
}

#[test]
fn a_problem_paints_its_headline_and_keeps_the_error_chain_as_a_detail() {
    let theme = ArtisanTheme::resolved();
    let problem = Problem::detailed(
        "Couldn't load this thread. Forge couldn't be reached.",
        CHAIN,
    );
    let app = mounted(
        ProblemNotice::new("problem", theme, &problem)
            .action(Button::text("retry", theme, ButtonTone::Neutral, "Try again").build())
            .build(),
    );
    let texts = painted(&app);
    assert!(
        texts
            .iter()
            .any(|text| text == "Couldn't load this thread. Forge couldn't be reached."),
        "{texts:?}"
    );
    assert!(texts.iter().any(|text| text == "Try again"), "{texts:?}");
    assert!(texts.iter().any(|text| text == "Details"), "{texts:?}");
    assert!(
        texts
            .iter()
            .all(|text| !text.contains("reach Forge") && !text.contains("https://")),
        "the chain is never painted: {texts:?}"
    );
    assert!(
        app.tree()
            .emit_semantics()
            .iter()
            .any(|node| node.description.as_deref() == Some(CHAIN)),
        "assistive tech can still read the chain"
    );
}

#[test]
fn a_problem_without_a_detail_offers_no_details() {
    let theme = ArtisanTheme::resolved();
    let app = mounted(
        ProblemNotice::new(
            "problem",
            theme,
            &Problem::new("This thread isn't on this Forge."),
        )
        .build(),
    );
    let texts = painted(&app);
    assert_eq!(texts, vec!["This thread isn't on this Forge.".to_owned()]);
}
