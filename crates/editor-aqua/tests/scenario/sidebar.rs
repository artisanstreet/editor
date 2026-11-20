use artisan_api::EngineId;
use test_support::scenario::{Opening, Role, Scenario};

const TITLES: [&str; 3] = [
    "Run the conformance tests",
    "Review the shadow primitives",
    "Wire the settings pages",
];

#[test]
fn a_search_that_matches_nothing_leaves_no_thread_rows_behind() {
    let mut editor = TITLES
        .iter()
        .enumerate()
        .fold(
            Scenario::replaying(EngineId::Claude, "reply"),
            |scenario, (index, title)| scenario.thread(&format!("thread-{index}"), title),
        )
        .open(Opening::NewThread);
    for title in TITLES {
        editor.shows(title);
    }
    editor.click(Role::TextInput, "Search threads");
    editor.type_text("zzqxv");
    let settled = editor.shows("No threads match “zzqxv”.");
    for title in TITLES {
        assert!(
            !settled.contains(title),
            "{title:?} is still listed after the search matched nothing:\n{settled}"
        );
    }
    editor.click(Role::Button, "Clear search");
    for title in TITLES {
        editor.shows(title);
    }
}
