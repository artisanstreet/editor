//! Exhaustive dependency-free coverage for the editor workspace identity policy.
//!
//! The implementation is included directly so this focused harness does not
//! require frontend module, protocol, navigation, Cargo, or Bazel registration.

#[path = "../../modules/frontend/src/editor_workspace_identity.rs"]
mod editor_workspace_identity;

use editor_workspace_identity::{
    EditorProjectInput, EditorRouteTarget, EditorThreadInput, editor_route_path,
    editor_route_target_for_thread,
};

fn project(project_id: &str) -> EditorProjectInput {
    EditorProjectInput::new(project_id)
}

fn thread(thread_id: &str, primary_project: Option<EditorProjectInput>) -> EditorThreadInput {
    EditorThreadInput::new(thread_id, primary_project)
}

#[test]
fn encode_uri_component_reserved_ascii_bytes_in_each_route_position() {
    let reserved = " /:?#[]@,;+$=%&\"<>\\^\x60{|}~!";
    let expected_component =
        "%20%2F%3A%3F%23%5B%5D%40%2C%3B%2B%24%3D%25%26%22%3C%3E%5C%5E%60%7B%7C%7D~!";

    assert_eq!(
        editor_route_path(reserved, "thread_", Some(reserved)),
        format!("/e/{expected_component}/thread_?file={expected_component}")
    );

    // !, ~, and the other encodeURIComponent-safe punctuation remain
    // literal; the route must not use the RFC 3986-safe set instead.
    assert_eq!(
        editor_route_path("AZaz09-_.!~*'()", "plain", None),
        "/e/AZaz09-_.!~*'()/plain"
    );
}

#[test]
fn unicode_components_are_utf8_percent_encoded_without_normalization() {
    let workspace = "Grüße / 世界/👩🏽‍💻";
    let thread_id = "thread_会話/🚀";
    let file = "src/名-😀.rs";

    assert_eq!(
        editor_route_path(workspace, thread_id, Some(file)),
        "/e/Gr%C3%BC%C3%9Fe%20%2F%20%E4%B8%96%E7%95%8C%2F%F0%9F%91%A9%F0%9F%8F%BD%E2%80%8D%F0%9F%92%BB/%E4%BC%9A%E8%A9%B1%2F%F0%9F%9A%80?file=src%2F%E5%90%8D-%F0%9F%98%80.rs"
    );
}

#[test]
fn thread_route_id_removes_only_one_nonempty_legacy_prefix() {
    let cases = [
        ("plain", "/e/workspace/plain"),
        ("thread_plain", "/e/workspace/plain"),
        ("thread_thread_plain", "/e/workspace/thread_plain"),
        ("thread_", "/e/workspace/thread_"),
        ("", "/e/workspace/"),
    ];

    for (thread_id, expected) in cases {
        assert_eq!(editor_route_path("workspace", thread_id, None), expected);
    }
}

#[test]
fn absent_and_present_empty_files_remain_distinct() {
    assert_eq!(
        editor_route_path("workspace", "thread", None),
        "/e/workspace/thread"
    );
    assert_eq!(
        editor_route_path("workspace", "thread", Some("")),
        "/e/workspace/thread?file="
    );
}

#[test]
fn file_is_encoded_as_one_query_value_without_path_normalization() {
    assert_eq!(
        editor_route_path("workspace", "thread_id", Some(" dir\\file?.rs#fragment ")),
        "/e/workspace/id?file=%20dir%5Cfile%3F.rs%23fragment%20"
    );
}

#[test]
fn detached_thread_target_uses_thread_route_and_omits_editor_fields() {
    let input = thread("thread_historical/42", None);

    let target = editor_route_target_for_thread(&input, Some("ignored.rs"));

    assert_eq!(
        target,
        EditorRouteTarget::Thread {
            path: "/t/_/historical%2F42".to_owned(),
        }
    );
    assert_eq!(target.target_type(), "thread");
    assert_eq!(target.path(), "/t/_/historical%2F42");
    assert_eq!(target.workspace_id(), None);
    assert_eq!(input.thread_id, "thread_historical/42");
    assert!(input.primary_project.is_none());
}

#[test]
fn project_backed_target_preserves_all_editor_fields_and_file_state() {
    let project_id = "  project/主  ";
    let thread_id = "thread_ thread/one ";
    let input = thread(thread_id, Some(project(project_id)));

    let target = editor_route_target_for_thread(&input, Some(""));

    assert_eq!(
        target,
        EditorRouteTarget::Editor {
            path: "/e/%20%20project%2F%E4%B8%BB%20%20/%20thread%2Fone%20?file=".to_owned(),
            workspace_id: project_id.to_owned(),
        }
    );
    assert_eq!(target.target_type(), "editor");
    assert_eq!(
        target.path(),
        "/e/%20%20project%2F%E4%B8%BB%20%20/%20thread%2Fone%20?file="
    );
    assert_eq!(target.workspace_id(), Some(project_id));
    assert_eq!(input.thread_id, thread_id);
    assert_eq!(
        input
            .primary_project
            .as_ref()
            .map(|project| project.project_id.as_str()),
        Some(project_id)
    );
}

#[test]
fn project_presence_is_the_only_target_branch_and_detached_file_is_ignored() {
    let files = [None, Some(""), Some("main.rs")];

    for file in files {
        let detached = editor_route_target_for_thread(&thread("detached", None), file);
        assert_eq!(detached.target_type(), "thread");
        assert_eq!(detached.path(), "/t/_/detached");
        assert_eq!(detached.workspace_id(), None);

        let project_target =
            editor_route_target_for_thread(&thread("detached", Some(project("workspace"))), file);
        assert_eq!(project_target.target_type(), "editor");
        assert_eq!(project_target.workspace_id(), Some("workspace"));
        assert_eq!(
            project_target.path(),
            match file {
                None => "/e/workspace/detached",
                Some("") => "/e/workspace/detached?file=",
                Some("main.rs") => "/e/workspace/detached?file=main.rs",
                Some(_) => unreachable!(),
            }
        );
    }
}
