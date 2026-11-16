use aqua_text::{SharedFonts, SharedRuns};
use artisan_api::tls::{Identity, fingerprint};
use editor_aqua::composer::controller::ComposerTarget;
use editor_aqua::composer::persistence::DraftSessions;
use editor_aqua::composer::view::ComposerOwner;
use editor_aqua::core::{Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind};
use editor_aqua::ui::theme::ArtisanTheme;

const SENT: &str = "Work is his work skill: do the entire feature request, not one step and not half of it.\n\nAnd review spawns two reviewers that look for footguns and bad state handling.";

fn card(owner: &ComposerOwner, core: &Core, host: &HostId) -> aqua_ui::element::Element {
    aqua_ui::element::Box::new()
        .width(700.0)
        .height(500.0)
        .child(owner.card(
            ArtisanTheme::resolved(),
            core.host(host).unwrap(),
            false,
            "",
            None,
            None,
            None,
            None,
        ))
        .build()
}

#[test]
fn a_sent_message_leaves_the_prompt_on_screen_at_once() {
    let directory = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().join("editor"),
    })
    .unwrap();
    let identity = Identity::generate("composer-view").unwrap();
    let print = fingerprint(identity.certificate_der().unwrap().as_ref());
    let client = ForgeClient::new("127.0.0.1:1", &print, &identity).unwrap();
    let host = HostId("composer-view".into());
    core.add_test_host(
        &HostConfig {
            id: host.clone(),
            name: "Forge".into(),
            address: "127.0.0.1:1".into(),
            fingerprint: print,
            kind: HostKind::Local,
        },
        client,
    );
    let mut sessions = DraftSessions::default();
    let owner = ComposerOwner::new(
        &core,
        &mut sessions,
        ComposerTarget::NewThread {
            host: host.clone(),
            project: None,
        },
        std::rc::Rc::new(|_| {}),
    );
    owner.controller.borrow().editor.borrow_mut().set_text(SENT);
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.set_viewport(700.0, 500.0, 1.0);
    let typed = card(&owner, &core, &host);
    app.mount(move |_| typed).unwrap();
    app.build_frame().unwrap();
    let key = aqua_ui::tree::hash_key(owner.editor.text_key().unwrap());
    let prompt = app.tree().sole_node_with_key(key).unwrap();
    owner.editor.sync(app.tree_mut(), prompt);
    app.build_frame().unwrap();
    assert_eq!(app.tree().editor_snapshot(prompt).unwrap().text, SENT);
    owner.controller.borrow().editor.borrow_mut().set_text("");
    app.refresh(vec![card(&owner, &core, &host)]).unwrap();
    let prompt = app.tree().sole_node_with_key(key).unwrap();
    owner.editor.sync(app.tree_mut(), prompt);
    app.build_frame().unwrap();
    assert_eq!(app.tree().editor_snapshot(prompt).unwrap().text, "");
}
