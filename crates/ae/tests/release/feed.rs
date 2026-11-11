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
        let source = Source::new(Some(from), None, false).expect("feed source");
        assert_eq!(source.update_feed(), Some(expected));
    }
    let source = Source::new(Some("."), None, false).expect("local build");
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

#[test]
fn a_nightly_build_updates_from_the_nightly_feed_and_a_release_from_the_latest() {
    assert_eq!(
        feed_for("0.0.0-nightly.20261111220000+abcdef123456"),
        NIGHTLY_FEED
    );
    assert_eq!(feed_for("1.4.0"), DEFAULT_FEED);
    assert_eq!(feed_for("0.5.0-dev+abc1234"), DEFAULT_FEED);
}

#[test]
fn the_file_that_installs_a_platform_is_the_editor_on_windows_and_ae_elsewhere() {
    assert_eq!(carrier("windows"), App::Editor);
    assert_eq!(carrier("linux"), App::Ae);
    assert_eq!(carrier("macos"), App::Ae);
}
