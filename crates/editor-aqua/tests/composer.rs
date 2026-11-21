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
        editor_aqua::composer::spool::DraftSpool::new(
            spool.path(),
            editor_aqua::composer::spool::LocalDisk::shared(),
        ),
    );
    (spool, sessions)
}

fn recovered(session: &editor_aqua::composer::persistence::DraftHandle) {
    let mut changed = session.subscribe();
    editor_aqua::core::runtime::runtime().block_on(async {
        while session.read(editor_aqua::composer::draft_session::DraftSession::recovering) {
            changed.changed().await.unwrap();
        }
    });
}
