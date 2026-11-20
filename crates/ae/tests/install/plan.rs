use super::*;

#[test]
fn every_place_survives_being_handed_to_another_installer() {
    for place in [
        EditorPlace::Here,
        EditorPlace::Windows,
        EditorPlace::Keep,
        EditorPlace::Skip,
    ] {
        assert_eq!(EditorPlace::parse(place.argument()), Ok(place));
    }
    for place in [
        ForgePlace::Here,
        ForgePlace::Wsl("Ubuntu-24.04".to_owned()),
        ForgePlace::Keep,
        ForgePlace::Skip,
    ] {
        assert_eq!(ForgePlace::parse(&place.argument()), Ok(place));
    }
    assert!(ForgePlace::parse("wsl:").is_err(), "a distro needs a name");
}

#[test]
fn a_place_an_app_cannot_go_is_refused_instead_of_skipped() {
    EditorPlace::parse("wsl:Ubuntu").unwrap_err();
    ForgePlace::parse("windows").unwrap_err();
}

#[test]
fn a_remembered_origin_is_a_feed_url_or_a_local_build() {
    assert_eq!(
        Origin::parse("https://example.com/release.json\n"),
        Some(Origin::Feed("https://example.com/release.json".to_owned()))
    );
    assert_eq!(Origin::parse("local\n"), Some(Origin::Local));
    for corrupt in [
        "",
        "\n",
        "https://",
        "/home/a/build",
        "https://a b/release.json",
    ] {
        assert_eq!(Origin::parse(corrupt), None, "{corrupt:?}");
    }
    for origin in [
        Origin::Local,
        Origin::Feed(DEFAULT_FEED_FOR_TESTS.to_owned()),
    ] {
        assert_eq!(Origin::parse(&origin.record()), Some(origin));
    }
}

const DEFAULT_FEED_FOR_TESTS: &str = release::DEFAULT_FEED;

#[cfg(unix)]
#[test]
fn a_running_forge_is_handed_over_only_when_its_database_needs_no_upgrade() {
    let storage = |schema, recall_index| StorageVersion {
        schema,
        recall_index: Some(recall_index),
    };
    assert_eq!(
        forge_upgrade(true, Some(storage(12, 2)), storage(12, 2)),
        ForgeUpgrade::Online
    );
    assert_eq!(
        forge_upgrade(true, Some(storage(11, 2)), storage(12, 2)),
        ForgeUpgrade::Restart
    );
    assert_eq!(
        forge_upgrade(true, Some(storage(12, 1)), storage(12, 2)),
        ForgeUpgrade::Restart
    );
    assert_eq!(
        forge_upgrade(true, None, storage(12, 2)),
        ForgeUpgrade::Restart
    );
    assert_eq!(
        forge_upgrade(false, Some(storage(12, 2)), storage(12, 2)),
        ForgeUpgrade::Restart
    );
}

#[test]
fn purging_later_finds_apps_whose_programs_are_already_gone() {
    let present = [App::Ae];
    let with_data = [App::Forge, App::Editor];

    assert_eq!(removable(&present, &with_data, false), [App::Ae]);
    assert_eq!(
        removable(&present, &with_data, true),
        [App::Ae, App::Forge, App::Editor]
    );
}

#[test]
fn ae_cannot_be_removed_while_apps_it_uninstalls_remain() {
    let present = [App::Ae, App::Forge, App::Editor];

    assert!(leaves_orphans(&[App::Ae], &present));
    assert!(leaves_orphans(&[App::Ae, App::Forge], &present));
    assert!(!leaves_orphans(
        &[App::Ae, App::Forge, App::Editor],
        &present
    ));
    assert!(!leaves_orphans(&[App::Forge], &present));
    assert!(!leaves_orphans(&[App::Ae], &[App::Ae]));
}

#[test]
fn a_purge_that_leaves_no_capturing_app_removes_the_claude_ai_browser_profile() {
    assert!(purges_claude_ai_browser(true, &[]));
    assert!(purges_claude_ai_browser(true, &[App::Forge]));
    assert!(!purges_claude_ai_browser(true, &[App::Editor]));
    assert!(!purges_claude_ai_browser(true, &[App::Ae]));
    assert!(!purges_claude_ai_browser(false, &[]));
}

#[test]
fn kept_data_is_purged_later_only_while_ae_stays_to_do_it() {
    let forge = PathBuf::from("/street/forge/data");
    let editor = PathBuf::from("/street/editor/data");
    let with_data = [(App::Forge, forge.clone()), (App::Editor, editor.clone())];

    assert_eq!(
        kept_data(&[App::Forge], &with_data, false),
        KeptData::PurgeLater
    );
    assert_eq!(
        kept_data(&[App::Ae, App::Forge, App::Editor], &with_data, false),
        KeptData::DeleteByHand(vec![forge, editor])
    );
    assert_eq!(
        kept_data(&[App::Ae, App::Forge, App::Editor], &with_data, true),
        KeptData::Nothing
    );
    assert_eq!(kept_data(&[App::Ae], &with_data, false), KeptData::Nothing);
}
