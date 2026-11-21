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

fn assert_explained(ui: &aqua::UiApp, failure: &ActionFailureDialog, cause: &str) {
    let texts = painted_texts(ui);
    assert!(
        texts.iter().all(|text| !text.contains(cause)),
        "the cause stays out of the headline: {texts:?}"
    );
    assert_eq!(
        failure
            .failure()
            .and_then(|failure| failure.problem().detail()),
        Some(cause),
        "the cause is the notice's detail"
    );
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
            "The browser didn't start.",
            "Details",
            URL,
        ],
    );
    assert_explained(&ui, &failure, "no browser is registered");
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
            "The code wasn't copied.",
            "Details",
        ],
    );
    assert_explained(&ui, &failure, "the clipboard is locked by another app");
    let texts = painted_texts(&ui);
    assert!(
        !texts
            .iter()
            .any(|text| text == "Couldn't open the link" || text == URL),
        "the copy failure replaces the earlier link notice: {texts:?}"
    );
}

#[test]
fn drafts_the_device_could_not_keep_are_explained_and_offer_trying_again_once_per_failure() {
    use editor_aqua::composer::persistence::UnkeptDrafts;
    use editor_aqua::shell::action_failure::KEEP_DRAFTS;

    let mut ui = workspace();
    let theme = ArtisanTheme::resolved();
    let mut failure = ActionFailureDialog::default();
    let unkept = UnkeptDrafts {
        count: 1,
        generation: 1,
        problem: editor_aqua::core::Problem::caused_by(
            "Artisan couldn't save your unsent draft to this device. Try again keeps it.",
            &std::io::Error::other("the disk is full"),
        ),
    };
    failure.offer_drafts(Some(unkept.clone()));
    failure.mount(&mut ui, &theme);
    ui.build_frame().unwrap();
    assert_on_screen(&ui, &["Couldn't keep unsaved drafts", "Try again"]);
    assert_explained(&ui, &failure, "the disk is full");

    click(&mut ui, KEEP_DRAFTS);
    assert_eq!(failure.drain(&mut ui), Some(Effect::KeepDrafts));
    failure.offer_drafts(Some(unkept.clone()));
    assert!(
        failure.failure().is_none(),
        "a failure already shown doesn't reopen on its own"
    );
    failure.retried_drafts(Some(unkept));
    failure.mount(&mut ui, &theme);
    ui.build_frame().unwrap();
    assert_on_screen(&ui, &["Couldn't keep unsaved drafts"]);
}

#[test]
fn the_drafts_notice_closes_once_every_held_draft_is_taken_back_and_stays_closed_until_a_new_one_is_held()
 {
    use editor_aqua::composer::persistence::UnkeptDrafts;

    let mut ui = workspace();
    let theme = ArtisanTheme::resolved();
    let mut failure = ActionFailureDialog::default();
    let held = |count: usize, generation: u64| UnkeptDrafts {
        count,
        generation,
        problem: editor_aqua::core::Problem::caused_by(
            format!("{count} held"),
            &std::io::Error::other("the disk is full"),
        ),
    };
    failure.offer_drafts(Some(held(2, 2)));
    failure.offer_drafts(Some(held(1, 2)));
    assert_eq!(
        failure
            .failure()
            .map(|failure| failure.problem().headline()),
        Some("1 held"),
        "taking one draft back updates the notice to the drafts still held"
    );

    failure.offer_drafts(None);
    failure.mount(&mut ui, &theme);
    ui.build_frame().unwrap();
    assert!(
        failure.failure().is_none(),
        "the notice closes once no draft is held"
    );
    assert!(
        !painted_texts(&ui)
            .iter()
            .any(|text| text == "Couldn't keep unsaved drafts")
    );

    failure.offer_drafts(Some(held(1, 3)));
    failure.mount(&mut ui, &theme);
    ui.build_frame().unwrap();
    click(&mut ui, editor_aqua::shell::action_failure::KEEP_DRAFTS);
    assert_eq!(failure.drain(&mut ui), Some(Effect::KeepDrafts));
    failure.offer_drafts(Some(held(1, 3)));
    assert!(
        failure.failure().is_none(),
        "a dismissed notice doesn't come back for the same drafts"
    );
    failure.offer_drafts(Some(held(2, 4)));
    assert!(
        failure.failure().is_some(),
        "a newly held draft shows the notice again"
    );
}

#[test]
fn a_drafts_notice_still_open_at_sign_out_shows_again_after_signing_in() {
    use editor_aqua::composer::persistence::UnkeptDrafts;

    let mut ui = workspace();
    let theme = ArtisanTheme::resolved();
    let mut failure = ActionFailureDialog::default();
    let unkept = UnkeptDrafts {
        count: 1,
        generation: 1,
        problem: editor_aqua::core::Problem::caused_by(
            "Artisan couldn't save your unsent draft to this device. Try again keeps it.",
            &std::io::Error::other("the disk is full"),
        ),
    };
    failure.offer_drafts(Some(unkept.clone()));
    failure.mount(&mut ui, &theme);
    ui.build_frame().unwrap();

    failure.tear_down(&mut ui);
    assert!(
        failure.failure().is_none(),
        "the sign-in screen has no notice"
    );
    let mut ui = workspace();
    failure.offer_drafts(Some(unkept));
    failure.mount(&mut ui, &theme);
    ui.build_frame().unwrap();

    assert_on_screen(&ui, &["Couldn't keep unsaved drafts", "Try again"]);
}
