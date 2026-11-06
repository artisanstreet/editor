#![cfg(windows)]

use aqua_core::{InputEvent, Point, PointerButton, PointerEvent, PointerKind};
use aqua_ime_tsf::double::TsfDouble;
use aqua_protocol::{Instant, Machine, PixelSize, RuntimeInput, ShellEvent, WindowId};
use aqua_text::{SharedFonts, SharedRuns};
use artisan_api::tls::{fingerprint, Identity};
use editor_aqua::composer::controller::ComposerTarget;
use editor_aqua::composer::persistence::DraftSessions;
use editor_aqua::composer::view::ComposerOwner;
use editor_aqua::core::{Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind};
use editor_aqua::ui::text::editor::Mode;
use editor_aqua::ui::theme::ArtisanTheme;
use std::rc::Rc;

fn window_id() -> WindowId {
    WindowId::new(1).unwrap()
}

struct Rig {
    app: aqua::UiApp,
    tsf: TsfDouble,
    now: f64,
    owner: ComposerOwner,
    _directory: tempfile::TempDir,
}

impl Rig {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let mut core = Core::for_tests(EditorFiles {
            dir: directory.path().join("editor"),
        })
        .unwrap();
        let identity = Identity::generate("composer-tsf").unwrap();
        let print = fingerprint(identity.certificate_der().unwrap().as_ref());
        let client = ForgeClient::new("127.0.0.1:1", &print, &identity).unwrap();
        let host = HostId("tsf-host".into());
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
                host,
                project: None,
            },
            Rc::new(|_| {}),
        );
        assert!(!owner.controller.borrow().editor.borrow().is_disabled());
        let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
        let editor = owner.editor.clone();
        app.mount(move |_| editor.element(Mode::Prompt, &ArtisanTheme::resolved(), ""))
            .unwrap();
        let mut rig = Self {
            app,
            tsf: TsfDouble::new().unwrap(),
            now: 0.0,
            owner,
            _directory: directory,
        };
        rig.step(RuntimeInput::Shell(ShellEvent::Resized {
            window: window_id(),
            size: PixelSize {
                width: 400,
                height: 300,
            },
            scale: 1.0,
        }));
        rig.frame();
        rig.step(RuntimeInput::Shell(ShellEvent::Input {
            window: window_id(),
            event: InputEvent::PointerDown(
                PointerEvent {
                    contact: 0,
                    kind: PointerKind::Mouse,
                    position: Point::new(20.0, 20.0),
                    buttons: 1,
                },
                PointerButton::Primary,
            ),
        }));
        rig.frame();
        rig
    }

    fn step(&mut self, input: RuntimeInput) {
        let output = self.app.step(input);
        for command in output.ime {
            self.tsf.command(command).expect("tsf accepts command");
        }
    }

    fn frame(&mut self) {
        self.now += 0.016;
        self.step(RuntimeInput::Shell(ShellEvent::Frame {
            window: window_id(),
            presentation: Instant::from_secs_f64(self.now),
        }));
    }

    fn settle(&mut self) {
        for _ in 0..4 {
            let mut events = Vec::new();
            self.tsf.drain(&mut events);
            events
                .into_iter()
                .for_each(|event| self.step(RuntimeInput::Ime(event)));
            self.frame();
        }
    }

    fn text(&self) -> String {
        self.owner
            .controller
            .borrow()
            .editor
            .borrow()
            .text()
            .to_owned()
    }

    fn marked(&self) -> Option<std::ops::Range<usize>> {
        self.owner
            .controller
            .borrow()
            .editor
            .borrow()
            .marked_range()
    }
}

#[test]
fn a_tsf_composition_commits_into_the_composer_draft_exactly_once() {
    let mut rig = Rig::new();
    assert_eq!(rig.text(), "");
    rig.tsf.compose(&["ka", "\u{304B}"], true).unwrap();
    rig.settle();
    assert_eq!(rig.text(), "\u{304B}");
    assert_eq!(rig.marked(), None);
    rig.tsf.compose(&["\u{306A}"], true).unwrap();
    rig.settle();
    assert_eq!(rig.text(), "\u{304B}\u{306A}");
}

#[test]
fn an_open_tsf_composition_is_marked_in_the_composer() {
    let mut rig = Rig::new();
    rig.tsf.compose(&["\u{5019}"], false).unwrap();
    rig.settle();
    assert!(rig.marked().is_some());
    assert_eq!(rig.text(), "\u{5019}");
}
