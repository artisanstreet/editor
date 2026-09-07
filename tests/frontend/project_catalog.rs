//! Focused parity coverage for the pure project catalog policy.

#[path = "../../modules/frontend/src/project_catalog.rs"]
mod project_catalog;

use project_catalog::{
    EMPTY_MONOGRAM, Project, ProjectRef, RecentProject, ThreadRow, preferred_project,
    preferred_project_owned, project_monogram, recent_projects,
};

fn project(project_id: &str, updated_at: &str) -> Project {
    Project::new(project_id, updated_at)
}

fn assigned_thread(project_id: &str, created_at: &str, last_message_at: Option<&str>) -> ThreadRow {
    ThreadRow::new(
        created_at,
        last_message_at.map(str::to_owned),
        Some(ProjectRef::new(project_id)),
    )
}

fn unassigned_thread(created_at: &str, last_message_at: Option<&str>) -> ThreadRow {
    ThreadRow::new(created_at, last_message_at.map(str::to_owned), None)
}

#[test]
fn assignments_count_and_keep_the_newest_activity_per_project() {
    let projects = [
        project("alpha", "2026-08-01T00:00:00Z"),
        project("beta", "2026-08-02T00:00:00Z"),
        project("quiet", "2026-08-01T00:00:00Z"),
    ];
    let threads = [
        assigned_thread("alpha", "2026-08-03T00:00:00Z", None),
        assigned_thread(
            "alpha",
            "2026-08-04T00:00:00Z",
            Some("2026-08-05T00:00:00Z"),
        ),
        assigned_thread(
            "alpha",
            "2026-08-06T00:00:00Z",
            Some("2026-08-04T12:00:00Z"),
        ),
        assigned_thread("beta", "2026-08-07T00:00:00Z", None),
        assigned_thread("unknown", "2026-08-08T00:00:00Z", None),
        unassigned_thread("2026-08-10T00:00:00Z", Some("2026-08-11T00:00:00Z")),
    ];

    let recents = recent_projects(&projects, &threads);

    assert_eq!(recents[0].project.project_id, "beta");
    assert_eq!(recents[0].last_message_at, "2026-08-07T00:00:00Z");
    assert_eq!(recents[0].thread_count, 1);
    assert_eq!(recents[1].project.project_id, "alpha");
    assert_eq!(recents[1].last_message_at, "2026-08-05T00:00:00Z");
    assert_eq!(recents[1].thread_count, 3);
    assert_eq!(recents[2].project.project_id, "quiet");
    assert_eq!(recents[2].last_message_at, "2026-08-01T00:00:00Z");
    assert_eq!(recents[2].thread_count, 0);
}

#[test]
fn newest_timestamp_is_lexicographic_and_fallback_uses_updated_at() {
    let projects = [
        project("alpha", "2026-01-01T00:00:00Z"),
        project("beta", "2026-02-01T00:00:00Z"),
    ];
    let threads = [
        assigned_thread("alpha", "2026-12-01T00:00:00Z", Some("2026-9-01T00:00:00Z")),
        assigned_thread(
            "alpha",
            "2026-11-01T00:00:00Z",
            Some("2026-10-01T00:00:00Z"),
        ),
    ];

    let recents = recent_projects(&projects, &threads);

    assert_eq!(recents[0].project.project_id, "alpha");
    assert_eq!(recents[0].last_message_at, "2026-9-01T00:00:00Z");
    assert_eq!(recents[0].thread_count, 2);
    assert_eq!(recents[1].project.project_id, "beta");
    assert_eq!(recents[1].last_message_at, "2026-02-01T00:00:00Z");
    assert_eq!(recents[1].thread_count, 0);
}

#[test]
fn equal_timestamps_retain_the_input_project_order() {
    let projects = [
        project("first", "2026-08-01T00:00:00Z"),
        project("second", "2026-08-01T00:00:00Z"),
        project("third", "2026-07-01T00:00:00Z"),
    ];

    let recents = recent_projects(&projects, &[]);

    assert_eq!(
        recents
            .iter()
            .map(|recent| recent.project.project_id.as_str())
            .collect::<Vec<_>>(),
        ["first", "second", "third"]
    );
}

#[test]
fn preferred_project_uses_known_request_then_first_then_none() {
    let recents = vec![
        RecentProject {
            last_message_at: "2026-08-02T00:00:00Z".to_owned(),
            project: project("first", "2026-08-01T00:00:00Z"),
            thread_count: 0,
        },
        RecentProject {
            last_message_at: "2026-08-01T00:00:00Z".to_owned(),
            project: project("second", "2026-07-01T00:00:00Z"),
            thread_count: 1,
        },
    ];

    assert_eq!(
        preferred_project(&recents, Some("second")).map(|project| project.project_id.as_str()),
        Some("second")
    );
    assert_eq!(
        preferred_project(&recents, Some("missing")).map(|project| project.project_id.as_str()),
        Some("first")
    );
    assert_eq!(
        preferred_project(&recents, None).map(|project| project.project_id.as_str()),
        Some("first")
    );
    assert_eq!(preferred_project(&[], Some("missing")), None);
    assert_eq!(
        preferred_project_owned(&recents, Some("second")),
        Some(recents[1].project.clone())
    );
}

#[test]
fn monograms_split_punctuation_paths_and_repeated_delimiters() {
    assert_eq!(project_monogram("alpha-beta"), "AB");
    assert_eq!(project_monogram("alpha_beta.project"), "AB");
    assert_eq!(project_monogram("--alpha__beta..gamma"), "AB");
    assert_eq!(project_monogram(r"C:\\Users\\sander\\artisan"), "CU");
    assert_eq!(project_monogram("alpha/beta\\gamma"), "AB");
}

#[test]
fn monograms_use_unicode_scalars_and_uppercase_without_byte_slicing() {
    assert_eq!(project_monogram("éclair"), "ÉC");
    assert_eq!(project_monogram("東京-世界"), "東世");
    assert_eq!(project_monogram("Å"), "Å");
    assert_eq!(project_monogram("🦀-rust"), "🦀R");
}

#[test]
fn empty_names_and_names_without_ascii_space_parts_use_the_empty_marker() {
    assert_eq!(project_monogram(""), EMPTY_MONOGRAM);
    assert_eq!(project_monogram("-_. /\\"), EMPTY_MONOGRAM);
    assert_eq!(project_monogram("   "), EMPTY_MONOGRAM);
    assert_eq!(project_monogram("alpha\tbeta"), "AL");
}
