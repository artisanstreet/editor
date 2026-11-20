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

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

fn cut_short_before_the_helper(dir: &Path, incoming: &Path, before: &str) {
    let died = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        replace_with(dir, incoming, before.to_owned(), |from, to| {
            assert!(
                !(from.starts_with(incoming) && to.ends_with("helper")),
                "the installer dies before the helper is placed"
            );
            std::fs::rename(from, to)
        })
    }));
    assert!(died.is_err(), "the replacement was cut short");
    assert_eq!(
        read(&dir.join("forge")),
        "new forge",
        "the cut-short install had already placed the new forge"
    );
}

#[test]
fn a_replacement_that_fails_halfway_puts_every_old_file_back() {
    let (_root, dir, incoming) = installed_dir();

    let replaced = replace_with(&dir, &incoming, (), |from, to| {
        if from.starts_with(&incoming) && to.ends_with("helper") {
            Err(io::Error::other("antivirus holds the file"))
        } else {
            std::fs::rename(from, to)
        }
    });

    let Err(Unreplaced::Undone { failed, undone }) = replaced else {
        panic!("expected the old files to be put back, got {replaced:?}");
    };
    assert!(matches!(*failed, ReplaceError::Place { .. }), "{failed:?}");
    assert_eq!(read(&dir.join("forge")), "old forge");
    assert_eq!(read(&dir.join("helper")), "old helper");
    undone.forget().unwrap();
    assert_eq!(names(&dir), [".incoming-test", "forge", "helper"]);
}

#[test]
fn a_replacement_puts_every_new_file_in_place_and_drops_the_old_ones() {
    let (_root, dir, incoming) = installed_dir();

    replace(&dir, &incoming, ()).unwrap().keep().unwrap();

    assert_eq!(read(&dir.join("forge")), "new forge");
    assert_eq!(read(&dir.join("helper")), "new helper");
    assert_eq!(names(&dir), [".incoming-test", "forge", "helper"]);
}

#[test]
fn an_install_cut_short_between_renames_is_put_back_with_what_it_recorded_about_before() {
    let (_root, dir, incoming) = installed_dir();
    cut_short_before_the_helper(&dir, &incoming, "schema 26");

    let left = pending::<String>(&dir)
        .unwrap()
        .expect("the cut-short install left a record");
    assert_eq!(left.before(), "schema 26");
    let undone = left.put_back().unwrap();

    assert_eq!(read(&dir.join("forge")), "old forge");
    assert_eq!(read(&dir.join("helper")), "old helper");
    assert!(undone.put_back_previous("forge"));
    undone.forget().unwrap();
    assert_eq!(names(&dir), [".incoming-test", "forge", "helper"]);
    assert!(pending::<String>(&dir).unwrap().is_none());
}

#[test]
fn files_put_back_without_settling_are_put_back_again_by_the_next_install() {
    let (_root, dir, incoming) = installed_dir();
    cut_short_before_the_helper(&dir, &incoming, "schema 26");
    let unsettled = pending::<String>(&dir)
        .unwrap()
        .unwrap()
        .put_back()
        .unwrap();
    drop(unsettled);

    let again = pending::<String>(&dir)
        .unwrap()
        .expect("the record stays until what came before is restored too");
    assert_eq!(again.before(), "schema 26");
    again.put_back().unwrap().forget().unwrap();

    assert_eq!(read(&dir.join("forge")), "old forge");
    assert_eq!(read(&dir.join("helper")), "old helper");
    assert_eq!(names(&dir), [".incoming-test", "forge", "helper"]);
}

#[test]
fn a_kept_install_leaves_nothing_for_the_next_one_to_put_back() {
    let (_root, dir, incoming) = installed_dir();

    replace(&dir, &incoming, ()).unwrap().keep().unwrap();

    assert!(pending::<()>(&dir).unwrap().is_none());
    assert_eq!(read(&dir.join("forge")), "new forge");
}

#[test]
fn a_record_cut_off_while_being_written_is_cleared_with_the_install_it_belonged_to() {
    let (_root, dir, incoming) = installed_dir();
    cut_short_before_the_helper(&dir, &incoming, "schema 26");
    std::fs::write(dir.join(JOURNAL_DRAFT), "{\"entries\":[").unwrap();

    pending::<String>(&dir)
        .unwrap()
        .unwrap()
        .put_back()
        .unwrap()
        .forget()
        .unwrap();

    assert_eq!(names(&dir), [".incoming-test", "forge", "helper"]);
}

#[test]
fn a_record_naming_files_outside_the_install_folder_is_refused_and_touches_nothing() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("forge");
    std::fs::create_dir_all(&dir).unwrap();
    let outside = root.path().join("notes");
    std::fs::write(&outside, "keep me").unwrap();
    for (name, aside) in [
        ("../notes", r#"".old-0-forge""#),
        ("forge", r#""../notes""#),
    ] {
        std::fs::write(
            dir.join(JOURNAL),
            format!(r#"{{"entries":[{{"name":"{name}","aside":{aside}}}],"before":null}}"#),
        )
        .unwrap();

        let refused = pending::<()>(&dir);

        assert!(
            matches!(
                refused,
                Err(RecoverError::Journal(JournalError::Unreadable { .. }))
            ),
            "{refused:?}"
        );
        assert_eq!(read(&outside), "keep me");
    }
}

#[cfg(unix)]
#[test]
fn files_that_cannot_be_put_back_are_named_with_their_cause_and_tried_again_later() {
    let (_root, dir, incoming) = installed_dir();
    let replaced = replace(&dir, &incoming, ()).unwrap();
    std::fs::remove_file(dir.join("forge")).unwrap();
    std::fs::create_dir(dir.join("forge")).unwrap();
    std::fs::write(dir.join("forge").join("held"), "in the way").unwrap();

    let failures = replaced.undo().unwrap_err();

    match &failures.first {
        RestoreError::PutBack { aside, target, .. } => {
            assert_eq!(aside, &dir.join(".old-0-forge"));
            assert_eq!(target, &dir.join("forge"));
        }
        other @ (RestoreError::Remove { .. }
        | RestoreError::NoAside(_)
        | RestoreError::Displace { .. }) => {
            panic!("expected the forge to fail to go back, got {other:?}")
        }
    }
    assert!(failures.rest.is_empty(), "{failures:?}");
    assert_eq!(
        read(&dir.join("helper")),
        "old helper",
        "every other file still goes back"
    );
    let retried = pending::<()>(&dir).unwrap().unwrap().put_back();
    assert!(
        matches!(retried, Err(RecoverError::Restore { .. })),
        "the record stays, so the next install tries again"
    );
    std::fs::remove_dir_all(dir.join("forge")).unwrap();
    pending::<()>(&dir)
        .unwrap()
        .unwrap()
        .put_back()
        .unwrap()
        .forget()
        .unwrap();
    assert_eq!(read(&dir.join("forge")), "old forge");
}

#[cfg(windows)]
#[test]
fn a_running_program_is_moved_out_of_the_way_when_the_previous_one_is_put_back() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("ae");
    let incoming = dir.join(".incoming-test");
    std::fs::create_dir_all(&incoming).unwrap();
    std::fs::write(dir.join("ae.exe"), "old ae").unwrap();
    let system = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32");
    std::fs::copy(system.join("findstr.exe"), incoming.join("ae.exe")).unwrap();
    let replaced = replace(&dir, &incoming, ()).unwrap();
    let mut running = std::process::Command::new(dir.join("ae.exe"))
        .arg("never typed")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();

    let undone = replaced.undo();

    drop(running.stdin.take());
    running.wait().unwrap();
    undone.unwrap().forget().unwrap();
    assert_eq!(read(&dir.join("ae.exe")), "old ae");
    assert_eq!(names(&dir), [".incoming-test", ".old-1-ae.exe", "ae.exe"]);
}
