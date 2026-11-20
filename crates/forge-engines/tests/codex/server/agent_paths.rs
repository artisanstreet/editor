use super::*;

fn owns(paths: &AgentPaths, sender: &str, path: &str) -> bool {
    paths
        .parent_path(sender, false)
        .is_some_and(|parent| path.starts_with(&format!("{parent}/")))
}

#[test]
fn closing_a_root_thread_forgets_the_paths_of_every_agent_under_it() {
    let mut paths = AgentPaths::default();
    paths.adopt("first", "reviewer", "/root/reviewer");
    paths.adopt("reviewer", "tester", "/root/reviewer/tester");
    paths.adopt("second", "planner", "/root/planner");
    assert!(owns(&paths, "reviewer", "/root/reviewer/linter"));
    assert!(owns(&paths, "tester", "/root/reviewer/tester/fixtures"));

    paths.forget("first");

    assert!(!owns(&paths, "reviewer", "/root/reviewer/linter"));
    assert!(!owns(&paths, "tester", "/root/reviewer/tester/fixtures"));
    assert!(
        owns(&paths, "planner", "/root/planner/notes"),
        "another root's agents keep their paths"
    );
}

#[test]
fn closing_an_agent_thread_forgets_only_that_agent() {
    let mut paths = AgentPaths::default();
    paths.adopt("first", "reviewer", "/root/reviewer");
    paths.adopt("first", "planner", "/root/planner");

    paths.forget("reviewer");

    assert!(!owns(&paths, "reviewer", "/root/reviewer/linter"));
    assert!(owns(&paths, "planner", "/root/planner/notes"));
    assert_eq!(paths.parent_path("first", true), Some("/root"));
}
