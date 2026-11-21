use super::*;

#[test]
fn a_chosen_folder_is_the_root_until_it_is_forgotten() {
    let folder = tempfile::tempdir().expect("temp folder");
    let default = folder.path().join("Artisan Street");
    assert_eq!(
        chosen_root(&default).expect("no pointer"),
        None,
        "no file keeps the default"
    );

    let elsewhere = folder.path().join("somewhere else ");
    point(&default, &elsewhere).expect("choose a folder");
    assert_eq!(
        chosen_root(&default).expect("read the pointer"),
        Some(elsewhere),
        "a folder name ending in a space survives"
    );
    assert!(
        !default.join(LOCATION_DRAFT).exists(),
        "the pointer is replaced in one step"
    );

    assert!(
        matches!(
            point(&default, Path::new("relative/folder")),
            Err(ChooseRootError::NotAbsolute { .. })
        ),
        "a relative folder is refused"
    );
    forget(&default).expect("forget the chosen folder");
    assert_eq!(chosen_root(&default).expect("no pointer"), None);
    assert!(!default.exists(), "the empty default folder is gone");
}

#[test]
fn a_damaged_location_file_is_an_error_not_the_default() {
    let folder = tempfile::tempdir().expect("temp folder");
    std::fs::write(folder.path().join(LOCATION), "relative/folder\n").expect("location file");
    assert!(matches!(
        chosen_root(folder.path()),
        Err(RootError::PointerNotAbsolute { named, .. }) if named == Path::new("relative/folder")
    ));

    std::fs::write(folder.path().join(LOCATION), [0xff, 0xfe, b'\n']).expect("location file");
    assert!(matches!(
        chosen_root(folder.path()),
        Err(RootError::PointerNotUtf8 { .. })
    ));
}

#[test]
fn an_unreadable_location_file_is_an_error_not_the_default() {
    let folder = tempfile::tempdir().expect("temp folder");
    std::fs::create_dir(folder.path().join(LOCATION)).expect("location as a folder");
    assert!(matches!(
        chosen_root(folder.path()),
        Err(RootError::ReadPointer { .. })
    ));
}

#[test]
fn relative_environment_folders_are_refused() {
    assert!(matches!(
        absolute_var("XDG_DATA_HOME", Some("data".into())),
        Err(EnvPathError::Relative {
            name: "XDG_DATA_HOME",
            ..
        })
    ));
    assert_eq!(
        absolute_var("XDG_DATA_HOME", Some("".into())).expect("empty is unset"),
        None
    );
    let absolute = std::env::temp_dir();
    assert_eq!(
        absolute_var("HOME", Some(absolute.clone().into_os_string())).expect("absolute"),
        Some(absolute)
    );
}
