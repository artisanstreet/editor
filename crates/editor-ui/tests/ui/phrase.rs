use super::*;

#[test]
fn a_phrase_that_fits_keeps_every_word() {
    assert_eq!(
        phrase_fit(&[30.0, 80.0, 120.0], 120.0, 10.0),
        PhraseFit::Whole
    );
    assert_eq!(phrase_fit(&[], 0.0, 10.0), PhraseFit::Whole);
}

#[test]
fn an_overflowing_phrase_ends_after_the_last_word_that_leaves_room_for_the_ellipsis() {
    assert_eq!(
        phrase_fit(&[30.0, 80.0, 120.0, 180.0], 125.0, 10.0),
        PhraseFit::Cut { words: 2, at: 80.0 },
        "the third word fits alone but not beside the ellipsis"
    );
    assert_eq!(
        phrase_fit(&[30.0, 80.0], 20.0, 10.0),
        PhraseFit::Cut { words: 0, at: 0.0 },
        "no word fits, so only the ellipsis remains"
    );
}

#[test]
fn a_shorter_phrase_forgets_a_cut_past_its_end() {
    let mut phrase = PhraseMotion::default();
    phrase.words("one two three four", 0.0, false);
    phrase.fit().set(PhraseFit::Cut { words: 3, at: 90.0 });
    phrase.words("one two", 1.0, false);
    assert_eq!(
        phrase.fit().get(),
        PhraseFit::Whole,
        "the stale cut is gone"
    );
}

fn rendered_labels(words: &[PhraseWord], text: &str) -> Vec<String> {
    use aqua_text::{SharedFonts, SharedRuns};
    let typography = TextStyle::default();
    let view = retained_phrase_or_text(
        Some(words),
        text.to_owned(),
        typography,
        crate::ui::theme::ArtisanTheme::resolved()
            .colors
            .text
            .to_reference_text_color(),
        20.0,
        PhraseOverflow {
            fit: Rc::default(),
            refit: None,
        },
    );
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| view).unwrap();
    app.set_viewport(400.0, 100.0, 1.0);
    app.build_frame().unwrap();
    app.tree()
        .emit_semantics()
        .into_iter()
        .map(|node| node.label)
        .filter(|label| !label.trim().is_empty())
        .collect()
}

#[test]
fn a_rolling_phrase_reads_as_its_current_text_once() {
    let mut phrase = PhraseMotion::default();
    phrase.words("Read 19 files", 0.0, false);
    let (words, animating) = phrase.words("Read 23 files", 0.1, false);
    let words = words.unwrap();
    assert!(animating);
    assert!(matches!(words[1].motion, WordMotion::Roll { .. }));
    assert_eq!(
        rendered_labels(&words, "Read 23 files"),
        vec!["Read 23 files".to_owned()],
        "the reel digits, measurement copies and ellipsis stay out of the tree"
    );
}

#[test]
fn an_extending_phrase_reads_as_its_current_text_once() {
    let mut phrase = PhraseMotion::default();
    phrase.words("Searching", 0.0, false);
    let (words, _) = phrase.words("Searching files", 0.1, false);
    assert_eq!(
        rendered_labels(&words.unwrap(), "Searching files"),
        vec!["Searching files".to_owned()]
    );
}
