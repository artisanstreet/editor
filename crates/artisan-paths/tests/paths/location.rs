use super::*;

#[test]
fn a_location_file_moves_the_root_until_it_names_something_unusable() {
    let default = std::env::temp_dir().join(format!("artisan-paths-{}", std::process::id()));
    std::fs::create_dir_all(&default).expect("default root");
    assert_eq!(chosen_root(&default), None, "no file keeps the default");

    let elsewhere = default.join("elsewhere");
    std::fs::write(default.join(LOCATION), format!("{}\n", elsewhere.display()))
        .expect("location file");
    assert_eq!(chosen_root(&default), Some(elsewhere));

    std::fs::write(default.join(LOCATION), "relative/folder\n").expect("location file");
    assert_eq!(chosen_root(&default), None, "a relative path is ignored");
    std::fs::remove_dir_all(&default).expect("clean up");
}
