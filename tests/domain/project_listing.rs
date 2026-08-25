//! Attached-project rediscovery coverage: bounded catalog listing and its
//! parameterless query.

use artisan_domain::{
    DisplayName, ListAttachedProjects, ListProjectThreads, PROJECT_LISTING_MAX_PROJECTS, ProjectId,
    ProjectListing, ProjectListingError, ProjectSummary, Query, RootPath, UnixMillis,
};

fn project_summary(index: usize) -> ProjectSummary {
    let offset = i64::try_from(index).expect("fixture indices fit i64");
    ProjectSummary {
        project_id: ProjectId::parse(format!("proj-{index}")).expect("fixture ids are valid"),
        display_name: DisplayName::parse(format!("Project {index}")).expect("fixture names valid"),
        root_path: RootPath::parse(format!(r"C:\dev\project-{index}"))
            .expect("fixture paths are valid"),
        attached_at: UnixMillis::from_millis(1_758_816_000_000 + offset),
    }
}

#[test]
fn project_listing_enforces_its_deliberate_bound() {
    let projects = (0..PROJECT_LISTING_MAX_PROJECTS)
        .map(project_summary)
        .collect();

    let listing = ProjectListing::new(projects).expect("a bounds-sized listing fits");
    assert_eq!(listing.projects().len(), PROJECT_LISTING_MAX_PROJECTS);

    // One summary past the documented ceiling is refused with counts attached.
    let over_bound = (0..=PROJECT_LISTING_MAX_PROJECTS)
        .map(project_summary)
        .collect();
    assert_eq!(
        ProjectListing::new(over_bound),
        Err(ProjectListingError::TooManyProjects {
            count: PROJECT_LISTING_MAX_PROJECTS + 1,
            maximum: PROJECT_LISTING_MAX_PROJECTS,
        })
    );

    // A fresh Forge instance rediscovers an empty catalog without error.
    let empty = ProjectListing::new(Vec::new()).expect("an empty listing fits");
    assert_eq!(empty.projects().len(), 0);
}

#[test]
fn project_listing_rejects_a_repeated_project_identity() {
    // Each row projects one durable attached-project primary key, so a
    // catalog naming one project twice is corrupt input, not a longer list.
    let duplicated = vec![project_summary(5), project_summary(3), project_summary(5)];
    assert_eq!(
        ProjectListing::new(duplicated),
        Err(ProjectListingError::DuplicateProject {
            project_id: ProjectId::parse("proj-5").expect("the fixture is valid"),
        }),
        "the first repeated identity is named"
    );
}

#[test]
fn project_listing_preserves_forge_supplied_order() {
    let listing = ProjectListing::new(vec![project_summary(7), project_summary(3)])
        .expect("two summaries fit");

    let ids: Vec<&str> = listing
        .projects()
        .iter()
        .map(|summary| summary.project_id.as_str())
        .collect();
    assert_eq!(ids, ["proj-7", "proj-3"]);
}

#[test]
fn list_attached_projects_query_carries_no_identity() {
    // The rediscovery read carries no id at all, so nothing can be stale or
    // unknown: Forge mints and resolves every identity it answers with.
    let query = Query::ListAttachedProjects(ListAttachedProjects);
    assert!(matches!(query, Query::ListAttachedProjects(_)));

    // Pure reads clone freely and compare by value like every other query.
    assert_eq!(query.clone(), query);

    let list_threads = Query::ListProjectThreads(ListProjectThreads {
        project_id: ProjectId::parse("proj-1").expect("the fixture is valid"),
    });
    assert_ne!(query, list_threads);
}
