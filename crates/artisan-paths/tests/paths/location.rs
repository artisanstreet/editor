use super::*;

#[test]
fn a_chosen_folder_is_the_root_until_it_is_forgotten() {
    let folder = tempfile::tempdir().expect("temp folder");
    let default = folder.path().join("Artisan Street");
    assert_eq!(chosen_root(&default), None, "no file keeps the default");

    let elsewhere = folder.path().join("somewhere else ");
    point(&default, &elsewhere).expect("choose a folder");
    assert_eq!(
        chosen_root(&default),
        Some(elsewhere),
        "a folder name ending in a space survives"
    );

    assert!(
        point(&default, Path::new("relative/folder")).is_err(),
        "a relative folder is refused"
    );
    point(&default, &default).expect("choose the default again");
    assert_eq!(chosen_root(&default), None);
    assert!(!default.join(LOCATION).exists(), "the pointer is gone");
}

#[test]
fn an_unusable_location_file_is_ignored() {
    let folder = tempfile::tempdir().expect("temp folder");
    std::fs::write(folder.path().join(LOCATION), "relative/folder\n").expect("location file");
    assert_eq!(chosen_root(folder.path()), None);
}
