use super::*;

const DAY: Duration = Duration::from_hours(24);

fn kept(path: &Path, written: SystemTime) {
    std::fs::write(path, "kept").expect("write a kept database");
    std::fs::File::options()
        .write(true)
        .open(path)
        .expect("open the kept database")
        .set_modified(written)
        .expect("date the kept database");
}

fn names(database: &Path) -> Vec<String> {
    list(database)
        .expect("list the kept databases")
        .into_iter()
        .map(|retained| {
            retained
                .path
                .file_name()
                .expect("a kept database has a name")
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

#[test]
fn the_two_newest_backups_stay_until_they_are_a_month_old() {
    let dir = tempfile::tempdir().expect("data dir");
    let database = dir.path().join("forge.sqlite3");
    let now = SystemTime::now();
    kept(&backup(&database, 24), now - DAY);
    kept(&backup(&database, 26), now - 2 * DAY);
    kept(&backup(&database, 29), now - DAY);

    prune(&database, now).expect("prune");
    assert_eq!(
        names(&database),
        ["forge.sqlite3.schema-26", "forge.sqlite3.schema-29"]
    );

    prune(&database, now + 29 * DAY).expect("prune a month later");
    assert_eq!(names(&database), ["forge.sqlite3.schema-29"]);

    prune(&database, now + 31 * DAY).expect("prune after a month");
    assert_eq!(names(&database), Vec::<String>::new());
}

#[test]
fn only_the_two_latest_set_aside_databases_are_kept() {
    let dir = tempfile::tempdir().expect("data dir");
    let database = dir.path().join("forge.sqlite3");
    let now = SystemTime::now();
    kept(&aside(&database, 31, 0), now - 90 * DAY);
    kept(&aside(&database, 30, 0), now - 3 * DAY);
    kept(&aside(&database, 31, 1), now - DAY);

    prune(&database, now).expect("prune");
    assert_eq!(
        names(&database),
        [
            "forge.sqlite3.rolled-back-from-30-0",
            "forge.sqlite3.rolled-back-from-31-1"
        ]
    );
}

#[test]
fn images_stay_pinned_only_while_the_database_that_uses_them_is_kept() {
    let dir = tempfile::tempdir().expect("data dir");
    let database = dir.path().join("forge.sqlite3");
    let hash = "ab".repeat(32);
    let kept_backup = backup(&database, 26);
    std::fs::write(&kept_backup, "kept").expect("write a backup");
    std::fs::write(manifest(&kept_backup), format!("{hash}\n")).expect("write its images");
    std::fs::write(
        manifest(&backup(&database, 20)),
        format!("{}\n", "cd".repeat(32)),
    )
    .expect("leave the images of a removed backup");

    assert_eq!(
        kept_images(&database).expect("read the kept images"),
        HashSet::from([hash])
    );
    discard(&kept_backup).expect("discard the backup");
    assert!(
        kept_images(&database)
            .expect("read the kept images")
            .is_empty()
    );
}
