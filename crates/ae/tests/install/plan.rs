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
    assert!(EditorPlace::parse("wsl:Ubuntu").is_err());
    assert!(ForgePlace::parse("windows").is_err());
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
    assert_eq!(forge_upgrade(true, Some(12), 12), ForgeUpgrade::Online);
    assert_eq!(forge_upgrade(true, Some(11), 12), ForgeUpgrade::Restart);
    assert_eq!(forge_upgrade(true, None, 12), ForgeUpgrade::Restart);
    assert_eq!(forge_upgrade(false, Some(12), 12), ForgeUpgrade::Restart);
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

fn installed_dir() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("forge");
    let incoming = dir.join(".incoming-test");
    std::fs::create_dir_all(&incoming).unwrap();
    for (name, old, new) in [
        ("forge", "old forge", "new forge"),
        ("helper", "old helper", "new helper"),
    ] {
        std::fs::write(dir.join(name), old).unwrap();
        std::fs::write(incoming.join(name), new).unwrap();
    }
    (root, dir, incoming)
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn a_replacement_that_fails_halfway_puts_every_old_file_back() {
    let (_root, dir, incoming) = installed_dir();

    let replaced = replace_with(&dir, &incoming, |from, to| {
        if from.starts_with(&incoming) && to.ends_with("helper") {
            Err(io::Error::other("antivirus holds the file"))
        } else {
            std::fs::rename(from, to)
        }
    });

    assert!(
        matches!(replaced, Err(ReplaceError::Place { .. })),
        "{replaced:?}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("forge")).unwrap(),
        "old forge"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("helper")).unwrap(),
        "old helper"
    );
    assert_eq!(names(&dir), [".incoming-test", "forge", "helper"]);
}

#[test]
fn a_replacement_puts_every_new_file_in_place_and_drops_the_old_ones() {
    let (_root, dir, incoming) = installed_dir();

    replace(&dir, &incoming).unwrap();

    assert_eq!(
        std::fs::read_to_string(dir.join("forge")).unwrap(),
        "new forge"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("helper")).unwrap(),
        "new helper"
    );
    assert_eq!(names(&dir), [".incoming-test", "forge", "helper"]);
}

#[test]
fn a_purge_that_leaves_no_capturing_app_removes_the_claude_ai_browser_profile() {
    assert!(purges_claude_ai_browser(true, &[]));
    assert!(purges_claude_ai_browser(true, &[App::Forge]));
    assert!(!purges_claude_ai_browser(true, &[App::Editor]));
    assert!(!purges_claude_ai_browser(true, &[App::Ae]));
    assert!(!purges_claude_ai_browser(false, &[]));
}
