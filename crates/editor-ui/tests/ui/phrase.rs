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
