#![cfg(test)]

#[path = "composer/draft_session.rs"]
mod draft_session;
#[path = "composer/encode.rs"]
mod encode;
#[path = "composer/model_line.rs"]
mod model_line;
#[path = "composer/model_list.rs"]
mod model_list;
#[path = "composer/persistence.rs"]
mod persistence;
#[path = "composer/picker.rs"]
mod picker;
#[path = "composer/questionnaire.rs"]
mod questionnaire;
#[path = "composer/sliding_tab.rs"]
mod sliding_tab;
#[cfg(windows)]
#[path = "composer/tsf.rs"]
mod tsf;
#[path = "composer/view.rs"]
mod view;

fn draft_sessions() -> (
    tempfile::TempDir,
    editor_aqua::composer::persistence::DraftSessions,
) {
    let spool = tempfile::tempdir().unwrap();
    let sessions = editor_aqua::composer::persistence::DraftSessions::with_spool(
        editor_aqua::composer::spool::DraftSpool::new(spool.path()),
    );
    (spool, sessions)
}
