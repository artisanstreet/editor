use aqua_ui::element::Box as UiBox;
use editor_aqua::shell::link_failure::{COPY, Effect, LinkFailureDialog};
use editor_aqua::ui::theme::ArtisanTheme;
use test_support::editor::ui::{click, mounted, painted_texts};

const URL: &str = "https://github.com/artisanstreet/editor";

#[test]
fn a_link_the_browser_cannot_open_offers_its_url_to_copy() {
    let mut ui = mounted(
        1000.0,
        700.0,
        UiBox::new()
            .width(1000.0)
            .height(700.0)
            .build()
            .debug_selector("workspace-root"),
    );
    let mut failure = LinkFailureDialog::default();
    failure.show(URL.into(), &std::io::Error::other("no browser is registered"));
    failure.mount(&mut ui, ArtisanTheme::resolved());
    ui.build_frame().unwrap();
    let texts = painted_texts(&ui);
    for wanted in [
        "Couldn't open the link",
        "The browser didn't start: no browser is registered.",
        URL,
    ] {
        assert!(
            texts.iter().any(|text| text == wanted),
            "{wanted:?} is on screen among {texts:?}"
        );
    }
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
