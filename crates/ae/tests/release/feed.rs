use super::*;

#[test]
fn updates_keep_nightly_and_custom_feeds_but_release_pins_follow_stable() {
    for (from, expected) in [
        (
            "https://github.com/artisanstreet/editor/releases/download/nightly/release.json",
            "https://github.com/artisanstreet/editor/releases/download/nightly/release.json",
        ),
        (
            "https://example.com/releases/release.json",
            "https://example.com/releases/release.json",
        ),
        (
            "https://github.com/artisanstreet/editor/releases/download/v1.2.3/release.json",
            DEFAULT_FEED,
        ),
    ] {
        let source = Source::new(Some(from), None).expect("feed source");
        assert_eq!(source.update_feed(), Some(expected));
    }
    let source = Source::new(Some("."), None).expect("local build");
    assert_eq!(source.update_feed(), None);
}

#[test]
fn nightly_timestamps_order_updates_even_when_the_base_version_is_unchanged() {
    assert!(
        newer(
            "0.0.0-nightly.20261111031700+abcdef",
            "0.0.0-nightly.20261110031700+123456"
        ),
        "a later nightly updates an earlier nightly"
    );
    assert!(
        !newer(
            "0.0.0-nightly.20261110031700+123456",
            "0.0.0-nightly.20261111031700+abcdef"
        ),
        "an earlier nightly does not replace a later nightly"
    );
}
