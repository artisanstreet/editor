use editor_aqua::ui::phrase::{PhraseMotion, WordMotion};

#[test]
fn phrase_tracks_number_rolls_extensions_and_settling() {
    let mut phrase = PhraseMotion::default();
    let (initial, animating) = phrase.words("a task", 0.0, false);
    assert!(!animating);
    assert!(
        initial
            .unwrap()
            .iter()
            .all(|word| word.motion == WordMotion::Still)
    );
    let (updated, animating) = phrase.words("2 tasks", 1.0, false);
    assert!(animating);
    let updated = updated.unwrap();
    assert_eq!(
        updated[0].motion,
        WordMotion::Roll {
            from: "a".to_owned(),
            up: true
        }
    );
    assert_eq!(updated[1].motion, WordMotion::Extend { kept: 4 });
    let (settled, animating) = phrase.words("2 tasks", 1.7, false);
    assert!(!animating);
    assert!(
        settled
            .unwrap()
            .iter()
            .all(|word| word.motion == WordMotion::Still)
    );
    let (reduced, animating) = phrase.words("3 tasks", 2.0, true);
    assert!(reduced.is_none());
    assert!(!animating);
    let (restored, animating) = phrase.words("3 tasks", 2.1, false);
    assert!(!animating);
    assert!(
        restored
            .unwrap()
            .iter()
            .all(|word| word.motion == WordMotion::Still)
    );
}
