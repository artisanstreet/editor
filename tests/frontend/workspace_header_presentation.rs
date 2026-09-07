//! Dependency-free parity coverage for the workspace-header presentation.
//!
//! The production module is included directly so these matrices run with
//! `rustc --test`, without Cargo metadata, UI dependencies, or registration
//! changes owned by the integrating VP.

#[path = "../../modules/frontend/src/workspace_header_presentation.rs"]
mod workspace_header_presentation;

use workspace_header_presentation::{
    WORKSPACE_HEADER_DETACHED_HEAD, WORKSPACE_HEADER_IN, WORKSPACE_HEADER_ON,
    WORKSPACE_HEADER_THREAD_SEPARATOR, WorkspaceHeaderBranch, WorkspaceHeaderInput,
    WorkspaceHeaderPresentation, WorkspaceHeaderRemote, WorkspaceHeaderRemoteLink,
    WorkspaceHeaderRepository, WorkspaceHeaderRepositoryInspection, WorkspaceHeaderSegment,
    WorkspaceHeaderText, present_workspace_header, workspace_header_presentation,
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct HostMark {
    name: String,
}

fn link<'a>(
    web_url: &'a str,
    link_label: &'a str,
    qualified_label: &'a str,
    host: &'a str,
) -> WorkspaceHeaderRemoteLink<'a, HostMark> {
    WorkspaceHeaderRemoteLink::new(
        web_url,
        link_label,
        qualified_label,
        HostMark {
            name: host.to_owned(),
        },
    )
}

fn remote<'a>(
    name: &'a str,
    link: Option<WorkspaceHeaderRemoteLink<'a, HostMark>>,
) -> WorkspaceHeaderRemote<'a, HostMark> {
    WorkspaceHeaderRemote::new(name, link)
}

fn repository<'a>(
    branch: WorkspaceHeaderBranch<'a>,
    default_remote: Option<&'a str>,
    remotes: &'a [WorkspaceHeaderRemote<'a, HostMark>],
) -> WorkspaceHeaderRepository<'a, HostMark> {
    WorkspaceHeaderRepository::new(branch, default_remote, remotes)
}

fn input<'a>(
    project_display_name: Option<&'a str>,
    repository: Option<WorkspaceHeaderRepositoryInspection<'a, HostMark>>,
    thread_title: Option<&'a str>,
) -> WorkspaceHeaderInput<'a, HostMark> {
    WorkspaceHeaderInput::new(project_display_name, repository, thread_title)
}

fn repository_inspection(
    repository: WorkspaceHeaderRepository<'_, HostMark>,
) -> WorkspaceHeaderRepositoryInspection<'_, HostMark> {
    WorkspaceHeaderRepositoryInspection::repository(repository)
}

fn text<'a>(segment: &WorkspaceHeaderSegment<'a, HostMark>) -> &'a str {
    segment.visible_text()
}

#[test]
fn no_project_hides_the_whole_header_for_every_inspection_and_title_state() {
    let remotes = [WorkspaceHeaderRemote::without_link("origin")];
    let repository = repository(
        WorkspaceHeaderBranch::Named("main"),
        Some("origin"),
        &remotes,
    );
    let inspections = [
        None,
        Some(WorkspaceHeaderRepositoryInspection::not_repository()),
        Some(repository_inspection(repository)),
    ];

    for inspection in inspections {
        for thread_title in [None, Some(""), Some("thread")] {
            assert_eq!(
                present_workspace_header(input(None, inspection.clone(), thread_title)),
                None,
                "a missing project must hide inspection={inspection:?} title={thread_title:?}"
            );
        }
    }
}

#[test]
fn absent_and_non_repository_inspection_use_the_folder_matrix() {
    let cases = [
        (None, None),
        (
            Some(WorkspaceHeaderRepositoryInspection::not_repository()),
            None,
        ),
        (None, Some("")),
        (
            Some(WorkspaceHeaderRepositoryInspection::not_repository()),
            Some("thread 🦀"),
        ),
    ];

    for (inspection, thread_title) in cases {
        let presentation =
            present_workspace_header(input(Some("Project 名"), inspection, thread_title))
                .expect("a present project must produce a header");
        let expected = match thread_title {
            None => vec!["Project 名"],
            Some(title) => vec!["Project 名", WORKSPACE_HEADER_THREAD_SEPARATOR, title],
        };
        assert_eq!(
            presentation.segments().iter().map(text).collect::<Vec<_>>(),
            expected,
        );
        assert!(presentation.is_visible());
    }
}

#[test]
fn repository_without_a_matching_web_remote_still_shows_on_and_branch() {
    let remotes = [
        remote("origin", None),
        remote(
            "backup",
            Some(link("file:///backup", "backup", "backup", "git")),
        ),
    ];
    let cases = [
        (None, WorkspaceHeaderBranch::Named("main")),
        (Some("missing"), WorkspaceHeaderBranch::Named("release/名")),
        (Some("origin"), WorkspaceHeaderBranch::DetachedHead),
    ];

    for (default_remote, branch) in cases {
        let snapshot = repository(branch, default_remote, &remotes);
        let presentation = present_workspace_header(input(
            Some("checkout"),
            Some(repository_inspection(snapshot)),
            None,
        ))
        .expect("repository project must produce a header");

        assert_eq!(presentation.segments.len(), 3);
        assert!(matches!(
            presentation.segments[0],
            WorkspaceHeaderSegment::Folder { label: "checkout" }
        ));
        assert!(matches!(
            presentation.segments[1],
            WorkspaceHeaderSegment::Text(WorkspaceHeaderText::On)
        ));
        assert_eq!(text(&presentation.segments[2]), branch.label());
        assert_eq!(
            text(&presentation.segments[1]),
            WORKSPACE_HEADER_ON,
            "the fixed repository connector remains exact"
        );
    }
}

#[test]
fn repository_selects_only_the_first_exact_default_remote_and_preserves_its_link() {
    let remotes = [
        remote(
            "mirror",
            Some(link(
                "https://first.invalid/a",
                "first",
                "owner/first",
                "first-host",
            )),
        ),
        remote(
            "origin",
            Some(link(
                "https://one.invalid/a",
                "one",
                "owner/one",
                "one-host",
            )),
        ),
        remote(
            "origin",
            Some(link(
                "https://two.invalid/a",
                "two",
                "owner/two",
                "two-host",
            )),
        ),
        remote(
            "Origin",
            Some(link(
                "https://case.invalid/a",
                "case",
                "owner/case",
                "case-host",
            )),
        ),
    ];
    let snapshot = repository(
        WorkspaceHeaderBranch::Named("main"),
        Some("origin"),
        &remotes,
    );
    let presentation = present_workspace_header(input(
        Some("one"),
        Some(repository_inspection(snapshot)),
        None,
    ))
    .expect("project must produce a header");

    let WorkspaceHeaderSegment::RemoteLink { link } = &presentation.segments[0] else {
        panic!("first segment should be the selected remote link");
    };
    assert_eq!(link.href(), "https://one.invalid/a");
    assert_eq!(link.repository_label(), "one");
    assert_eq!(link.qualified_label(), "owner/one");
    assert_eq!(link.host_mark().name, "one-host");

    let no_match = repository(
        WorkspaceHeaderBranch::Named("main"),
        Some("ORIGIN"),
        &remotes,
    );
    let no_match_presentation = present_workspace_header(input(
        Some("checkout"),
        Some(repository_inspection(no_match)),
        None,
    ))
    .expect("project must produce a header");
    assert!(matches!(
        no_match_presentation.segments[0],
        WorkspaceHeaderSegment::Folder { .. }
    ));
}

#[test]
fn linked_remote_emits_the_complete_source_order_and_checkout_only_when_distinct() {
    let remotes = [remote(
        "origin",
        Some(link(
            "https://example.invalid/team/repo",
            "repo",
            "team/repo",
            "host",
        )),
    )];
    let repository = repository(
        WorkspaceHeaderBranch::Named("feature/名"),
        Some("origin"),
        &remotes,
    );
    let presentation = present_workspace_header(input(
        Some("worktree"),
        Some(repository_inspection(repository)),
        Some("Thread title"),
    ))
    .expect("project must produce a header");

    assert_eq!(
        presentation.segments.iter().map(text).collect::<Vec<_>>(),
        vec![
            "team/repo",
            WORKSPACE_HEADER_ON,
            "feature/名",
            WORKSPACE_HEADER_IN,
            "worktree",
            WORKSPACE_HEADER_THREAD_SEPARATOR,
            "Thread title",
        ]
    );
    assert!(matches!(
        presentation.segments[3],
        WorkspaceHeaderSegment::Text(WorkspaceHeaderText::In)
    ));
    assert!(matches!(
        presentation.segments[4],
        WorkspaceHeaderSegment::Checkout { label: "worktree" }
    ));
}

#[test]
fn checkout_comparison_is_unicode_case_insensitive_and_default_is_suppressed() {
    let remotes = [remote(
        "origin",
        Some(link(
            "https://example.invalid/repo",
            "Repo",
            "owner/Repo",
            "host",
        )),
    )];
    let branch = WorkspaceHeaderBranch::Named("main");

    for project_name in ["repo", "REPO", "RePo", "default", "DEFAULT", "DeFaUlT"] {
        let repository = repository(branch, Some("origin"), &remotes);
        let presentation = present_workspace_header(input(
            Some(project_name),
            Some(repository_inspection(repository)),
            None,
        ))
        .expect("project must produce a header");
        assert_eq!(
            presentation.segments.len(),
            3,
            "checkout should be suppressed for {project_name:?}"
        );
    }

    let unicode_remotes = [remote(
        "origin",
        Some(link(
            "https://example.invalid/école",
            "ÉCOLE",
            "owner/ÉCOLE",
            "host",
        )),
    )];
    let repository = repository(branch, Some("origin"), &unicode_remotes);
    let presentation = present_workspace_header(input(
        Some("école"),
        Some(repository_inspection(repository)),
        None,
    ))
    .expect("project must produce a header");
    assert_eq!(presentation.segments.len(), 3);
}

#[test]
fn detached_and_named_branches_preserve_exact_labels() {
    let remotes = [remote("origin", None)];
    let cases = [
        (
            WorkspaceHeaderBranch::DetachedHead,
            WORKSPACE_HEADER_DETACHED_HEAD,
        ),
        (WorkspaceHeaderBranch::Named("main"), "main"),
        (
            WorkspaceHeaderBranch::Named("feature/🚀/名"),
            "feature/🚀/名",
        ),
        (WorkspaceHeaderBranch::Named(""), ""),
    ];

    for (branch, expected) in cases {
        assert_eq!(branch.label(), expected);
        assert_eq!(
            branch.is_detached(),
            matches!(branch, WorkspaceHeaderBranch::DetachedHead)
        );
        let repository = repository(branch, Some("origin"), &remotes);
        let presentation = present_workspace_header(input(
            Some("project"),
            Some(repository_inspection(repository)),
            None,
        ))
        .expect("project must produce a header");
        assert_eq!(text(&presentation.segments[2]), expected);
    }
}

#[test]
fn empty_and_unicode_labels_are_not_normalized_and_empty_title_is_present() {
    let remotes = [remote(
        "origin",
        Some(link("https://例.invalid/", "", "", "标记")),
    )];
    let repository = repository(
        WorkspaceHeaderBranch::Named("分支"),
        Some("origin"),
        &remotes,
    );
    let presentation = present_workspace_header(input(
        Some("工作区"),
        Some(repository_inspection(repository)),
        Some(""),
    ))
    .expect("project must produce a header");

    assert_eq!(presentation.segments.len(), 7);
    assert_eq!(text(&presentation.segments[0]), "");
    assert_eq!(text(&presentation.segments[3]), WORKSPACE_HEADER_IN);
    assert_eq!(text(&presentation.segments[4]), "工作区");
    assert_eq!(
        text(&presentation.segments[5]),
        WORKSPACE_HEADER_THREAD_SEPARATOR
    );
    assert_eq!(text(&presentation.segments[6]), "");
}

#[test]
fn borrowed_labels_and_owned_marks_remain_borrowed_by_the_projection() {
    let project = String::from("project");
    let title = String::from("thread");
    let web_url = String::from("https://example.invalid/repo");
    let link_label = String::from("repo");
    let qualified_label = String::from("owner/repo");
    let remotes = [remote(
        "origin",
        Some(WorkspaceHeaderRemoteLink::new(
            web_url.as_str(),
            link_label.as_str(),
            qualified_label.as_str(),
            HostMark {
                name: String::from("github"),
            },
        )),
    )];
    let repository = repository(
        WorkspaceHeaderBranch::Named("main"),
        Some("origin"),
        &remotes,
    );
    let presentation = present_workspace_header(input(
        Some(project.as_str()),
        Some(repository_inspection(repository)),
        Some(title.as_str()),
    ))
    .expect("project must produce a header");

    let WorkspaceHeaderSegment::RemoteLink { link } = &presentation.segments[0] else {
        panic!("expected remote link segment");
    };
    assert!(std::ptr::eq(link.web_url.as_ptr(), web_url.as_ptr()));
    assert!(std::ptr::eq(
        link.qualified_label.as_ptr(),
        qualified_label.as_ptr()
    ));
    assert_eq!(link.host_mark.name, "github");
    assert!(std::ptr::eq(
        text(&presentation.segments[4]).as_ptr(),
        project.as_ptr()
    ));
    assert!(std::ptr::eq(
        text(&presentation.segments[6]).as_ptr(),
        title.as_ptr()
    ));
}

#[test]
fn presentation_is_deterministic_and_alias_has_identical_segments() {
    let remotes = [remote(
        "origin",
        Some(link(
            "https://example.invalid/repo",
            "repo",
            "owner/repo",
            "host",
        )),
    )];
    let repository = repository(
        WorkspaceHeaderBranch::Named("main"),
        Some("origin"),
        &remotes,
    );
    let first_input = input(
        Some("checkout"),
        Some(repository_inspection(repository.clone())),
        Some("thread"),
    );
    let second_input = first_input.clone();
    let first: WorkspaceHeaderPresentation<'_, HostMark> =
        present_workspace_header(first_input).expect("project must produce a header");
    let second = workspace_header_presentation(second_input).expect("alias must produce a header");

    assert_eq!(first, second);
    assert_eq!(first.into_segments().len(), 7);
}
