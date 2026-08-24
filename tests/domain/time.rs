//! Coverage for the signed Unix epoch millisecond timestamp contract.

use artisan_domain::{
    DisplayName, ProjectId, ProjectSummary, RootPath, ThreadId, ThreadSummary, ThreadTitle,
    UnixMillis,
};

#[test]
fn accepts_the_complete_signed_wire_range() {
    // Negative values are valid instants before 1970: the schema
    // deliberately says signed epoch milliseconds.
    let before_epoch = UnixMillis::from_millis(-1);
    assert_eq!(before_epoch.as_millis(), -1);

    let deep_past = UnixMillis::from_millis(-62_167_219_200_000);
    assert_eq!(deep_past.as_millis(), -62_167_219_200_000);

    assert_eq!(UnixMillis::from_millis(0), UnixMillis::EPOCH);
    assert_eq!(UnixMillis::EPOCH.as_millis(), 0);

    let after_epoch = UnixMillis::from_millis(1_758_816_000_000);
    assert_eq!(after_epoch.as_millis(), 1_758_816_000_000);
}

#[test]
fn exposes_extreme_instants_without_narrowing() {
    assert_eq!(UnixMillis::MAX.as_millis(), i64::MAX);
    assert_eq!(UnixMillis::MIN.as_millis(), i64::MIN);

    // Const construction and raw construction agree at both extremes.
    assert_eq!(UnixMillis::from_millis(i64::MAX), UnixMillis::MAX);
    assert_eq!(UnixMillis::from_millis(i64::MIN), UnixMillis::MIN);
}

#[test]
fn orders_instants_by_raw_milliseconds() {
    assert!(UnixMillis::MIN < UnixMillis::EPOCH);
    assert!(UnixMillis::EPOCH < UnixMillis::MAX);
    assert!(
        UnixMillis::from_millis(-5) < UnixMillis::from_millis(5),
        "negative instants sort before positive ones"
    );
}

#[test]
fn projects_back_to_the_raw_wire_value() {
    for raw in [i64::MIN, -1, 0, 1, 1_758_816_000_000, i64::MAX] {
        assert_eq!(
            UnixMillis::from_millis(raw).as_millis(),
            raw,
            "the projection must stay total for {raw}"
        );
    }
}

#[test]
fn summaries_carry_schema_timestamps() {
    let project = ProjectSummary {
        project_id: ProjectId::parse("proj-time").expect("the fixture is valid"),
        display_name: DisplayName::parse("artisan-editor").expect("the fixture is valid"),
        root_path: RootPath::parse(r"C:\dev\artisan-editor").expect("the fixture is valid"),
        attached_at: UnixMillis::from_millis(-3_600_000),
    };
    assert_eq!(project.attached_at.as_millis(), -3_600_000);

    let thread = ThreadSummary {
        thread_id: ThreadId::parse("th-time").expect("the fixture is valid"),
        project_id: project.project_id.clone(),
        title: ThreadTitle::parse("New thread").expect("the fixture is valid"),
        created_at: UnixMillis::from_millis(0),
        updated_at: UnixMillis::MAX,
    };
    assert_eq!(thread.created_at.as_millis(), 0);
    assert_eq!(thread.updated_at.as_millis(), i64::MAX);

    // No cross-field ordering is enforced between created and updated.
    let inverted = ThreadSummary {
        thread_id: ThreadId::parse("th-inverted").expect("the fixture is valid"),
        project_id: project.project_id,
        title: thread.title.clone(),
        created_at: UnixMillis::MAX,
        updated_at: UnixMillis::MIN,
    };
    assert!(inverted.created_at > inverted.updated_at);
}
