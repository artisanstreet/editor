use aqua_ui::element::Box as UiBox;
use editor_aqua::shell::action_failure::{ActionFailureDialog, COPY, CopySubject, Effect};
use editor_aqua::ui::theme::ArtisanTheme;
use test_support::editor::ui::{click, mounted, painted_texts};

const URL: &str = "https://github.com/artisanstreet/editor";

fn workspace() -> aqua::UiApp {
    mounted(
        1000.0,
        700.0,
        UiBox::new()
            .width(1000.0)
            .height(700.0)
            .build()
            .debug_selector("workspace-root"),
    )
}

fn assert_on_screen(ui: &aqua::UiApp, wanted: &[&str]) {
    let texts = painted_texts(ui);
    for wanted in wanted {
        assert!(
            texts.iter().any(|text| text == wanted),
            "{wanted:?} is on screen among {texts:?}"
        );
    }
}

#[test]
fn a_link_the_browser_cannot_open_offers_its_url_to_copy() {
    let mut ui = workspace();
    let mut failure = ActionFailureDialog::default();
    failure.link(
        URL.into(),
        &std::io::Error::other("no browser is registered"),
    );
    failure.mount(&mut ui, &ArtisanTheme::resolved());
    ui.build_frame().unwrap();
    assert_on_screen(
        &ui,
        &[
            "Couldn't open the link",
            "The browser didn't start: no browser is registered.",
            URL,
        ],
    );
    click(&mut ui, COPY);
    assert_eq!(failure.drain(&mut ui), Some(Effect::CopyUrl(URL.into())));
    ui.build_frame().unwrap();
    assert!(failure.failure().is_none());
    assert!(
        !painted_texts(&ui)
            .iter()
            .any(|text| text == "Couldn't open the link"),
        "copying the link closes the notice"
    );
}

#[test]
fn a_copy_the_clipboard_refuses_is_explained_and_replaces_an_earlier_notice() {
    let mut ui = workspace();
    let theme = ArtisanTheme::resolved();
    let mut failure = ActionFailureDialog::default();
    failure.link(
        URL.into(),
        &std::io::Error::other("no browser is registered"),
    );
    failure.mount(&mut ui, &theme);
    ui.build_frame().unwrap();
    failure.copy(
        CopySubject::Code,
        &std::io::Error::other("the clipboard is locked by another app"),
    );
    failure.mount(&mut ui, &theme);
    ui.build_frame().unwrap();
    assert_on_screen(
        &ui,
        &[
            "Couldn't copy to the clipboard",
            "The code wasn't copied: the clipboard is locked by another app.",
        ],
    );
    let texts = painted_texts(&ui);
    assert!(
        !texts
            .iter()
            .any(|text| text == "Couldn't open the link" || text == URL),
        "the copy failure replaces the earlier link notice: {texts:?}"
    );
}
