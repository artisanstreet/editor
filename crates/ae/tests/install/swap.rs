use super::*;

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

    replace(&dir, &incoming).unwrap().keep().unwrap();

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
fn an_install_cut_short_between_renames_is_put_back_by_the_next_one() {
    let (_root, dir, incoming) = installed_dir();

    let died = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        replace_with(&dir, &incoming, |from, to| {
            assert!(
                !(from.starts_with(&incoming) && to.ends_with("helper")),
                "the installer dies before the helper is placed"
            );
            std::fs::rename(from, to)
        })
    }));

    assert!(died.is_err(), "the replacement was cut short");
    assert_eq!(
        std::fs::read_to_string(dir.join("forge")).unwrap(),
        "new forge",
        "the cut-short install had already placed the new forge"
    );
    assert_eq!(recover(&dir).unwrap(), Recovered::PutBack);
    assert_eq!(
        std::fs::read_to_string(dir.join("forge")).unwrap(),
        "old forge"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("helper")).unwrap(),
        "old helper"
    );
    assert_eq!(names(&dir), [".incoming-test", "forge", "helper"]);
    assert_eq!(recover(&dir).unwrap(), Recovered::Nothing);
}

#[test]
fn a_kept_install_leaves_nothing_for_the_next_one_to_put_back() {
    let (_root, dir, incoming) = installed_dir();

    replace(&dir, &incoming).unwrap().keep().unwrap();

    assert_eq!(recover(&dir).unwrap(), Recovered::Nothing);
    assert_eq!(
        std::fs::read_to_string(dir.join("forge")).unwrap(),
        "new forge"
    );
}

#[cfg(unix)]
#[test]
fn files_that_cannot_be_put_back_are_named_with_their_cause_and_tried_again_later() {
    let (_root, dir, incoming) = installed_dir();
    let replaced = replace(&dir, &incoming).unwrap();
    std::fs::remove_file(dir.join("forge")).unwrap();
    std::fs::create_dir(dir.join("forge")).unwrap();
    std::fs::write(dir.join("forge").join("held"), "in the way").unwrap();

    let failures = replaced.undo().unwrap_err();

    match &failures.first {
        RestoreError::PutBack { aside, target, .. } => {
            assert_eq!(aside, &dir.join(".old-0-forge"));
            assert_eq!(target, &dir.join("forge"));
        }
        other @ (RestoreError::Remove { .. } | RestoreError::Journal(_)) => {
            panic!("expected the forge to fail to go back, got {other:?}")
        }
    }
    assert!(failures.rest.is_empty(), "{failures:?}");
    assert_eq!(
        std::fs::read_to_string(dir.join("helper")).unwrap(),
        "old helper",
        "every other file still goes back"
    );
    assert!(
        matches!(recover(&dir), Err(RecoverError::Restore { .. })),
        "the record stays, so the next install tries again"
    );
    std::fs::remove_dir_all(dir.join("forge")).unwrap();
    assert_eq!(recover(&dir).unwrap(), Recovered::PutBack);
    assert_eq!(
        std::fs::read_to_string(dir.join("forge")).unwrap(),
        "old forge"
    );
}
