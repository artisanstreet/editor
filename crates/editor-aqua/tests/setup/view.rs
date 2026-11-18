use aqua::UiApp;
use aqua_core::{InputEvent, Point, PointerButton, PointerEvent, PointerKind};
use aqua_ui::element::Key;
use aqua_ui::tree::hash_key;
use editor_aqua::setup::InstallError;
use editor_aqua::setup::model::{Choice, Event, Setup};
use editor_aqua::setup::view::{self, Props};
use editor_aqua::shell::host::ShellResources;
use editor_aqua::ui::theme::ArtisanTheme;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

fn shown(setup: &Setup) -> (UiApp, Rc<RefCell<Vec<Choice>>>) {
    shown_with_motion(setup, false)
}

fn shown_with_motion(setup: &Setup, reduced_motion: bool) -> (UiApp, Rc<RefCell<Vec<Choice>>>) {
    let mut app = ShellResources::new(1.0).expect("setup window resources").ui;
    let heard = Rc::new(RefCell::new(Vec::new()));
    let hearing = Rc::clone(&heard);
    let element = view::screen(
        &ArtisanTheme::resolved(),
        &Props {
            setup,
            reduced_motion,
            emit: Rc::new(move |event| hearing.borrow_mut().push(event)),
        },
    );
    app.mount(|_| element).expect("mount setup");
    app.set_viewport(view::WINDOW.0 as f32, view::WINDOW.1 as f32, 1.0);
    app.build_frame().expect("lay out setup");
    (app, heard)
}

fn rect(app: &UiApp, key: &str) -> aqua_core::Rect {
    let node = app
        .tree()
        .sole_node_with_key(hash_key(Key::from(key)))
        .expect("the element is on screen");
    app.tree().rect(node).expect("a laid out node")
}

fn click(app: &mut UiApp, key: &str) {
    let bounds = rect(app, key);
    let position = Point::new(bounds.x + bounds.w * 0.5, bounds.y + bounds.h * 0.5);
    let pointer = |buttons| PointerEvent {
        contact: 0,
        kind: PointerKind::Mouse,
        position,
        buttons,
    };
    app.handle_input(&InputEvent::PointerDown(pointer(1), PointerButton::Primary));
    app.handle_input(&InputEvent::PointerUp(pointer(0), PointerButton::Primary));
}

fn with_distros(distros: &[&str]) -> Setup {
    with_distros_in(distros, true)
}

fn with_distros_in(distros: &[&str], relocatable: bool) -> Setup {
    let mut setup = Setup::fresh(
        "0.0.0-nightly.20261111220000+abcdef123456",
        PathBuf::from(r"C:\Users\someone with a long name\AppData\Local\Artisan Street"),
        relocatable,
    );
    setup.apply(Event::DistrosFound(
        distros.iter().map(|distro| (*distro).to_owned()).collect(),
    ));
    setup
}

fn with_three_distros() -> Setup {
    with_distros(&["Ubuntu-24.04", "Debian", "NixOS"])
}

fn card_fits(app: &UiApp) -> bool {
    let card = rect(app, "artisan-setup-card");
    card.y >= 0.0 && card.y + card.h <= view::WINDOW.1 as f32
}

#[test]
fn every_choice_fits_the_window_and_reports_what_was_clicked() {
    let (mut app, heard) = shown(&with_three_distros());
    assert!(
        card_fits(&app),
        "four machines and a long folder fit the window"
    );
    click(&mut app, "artisan-setup-machine-2");
    click(&mut app, "artisan-setup-change");
    click(&mut app, "artisan-setup-install");
    assert_eq!(
        *heard.borrow(),
        [Choice::Selected(2), Choice::ChangeLocation, Choice::Started]
    );
}

#[test]
fn a_failure_shows_its_reason_and_offers_another_try() {
    let mut setup = with_three_distros();
    setup.apply(Event::Chose(Choice::Started));
    setup.apply(Event::Finished(Err(InstallError::Refused {
        code: Some(1),
        reason: "Forge didn't start: the port is in use".to_owned(),
    })));
    let (mut app, heard) = shown(&setup);
    let said: Vec<String> = app
        .tree()
        .traverse()
        .into_iter()
        .filter_map(|node| match &app.tree().get(node)?.kind {
            aqua_ui::tree::NodeKind::Text(text) => Some(text.text().to_owned()),
            _ => None,
        })
        .collect();
    assert!(
        said.iter()
            .any(|line| line == "Forge didn't start: the port is in use"),
        "the reason is on screen: {said:?}"
    );
    click(&mut app, "artisan-setup-retry");
    assert_eq!(*heard.borrow(), [Choice::Started]);
}

#[test]
fn many_distros_still_leave_install_within_reach() {
    let (mut app, heard) = shown(&with_distros(&[
        "Ubuntu-24.04",
        "Ubuntu-22.04",
        "Debian",
        "NixOS",
        "Arch",
        "Fedora",
        "Alpine",
    ]));
    assert!(card_fits(&app), "eight machines fit the window");
    click(&mut app, "artisan-setup-machine-7");
    click(&mut app, "artisan-setup-install");
    assert_eq!(*heard.borrow(), [Choice::Selected(7), Choice::Started]);
}

#[test]
fn an_install_that_already_has_a_folder_does_not_offer_another() {
    let mut setup = with_distros_in(&["Ubuntu-24.04", "Debian", "NixOS"], false);
    let (app, _) = shown(&setup);
    assert!(
        app.tree()
            .sole_node_with_key(hash_key(Key::from("artisan-setup-change")))
            .is_none(),
        "the folder can't be changed once something is installed there"
    );
    assert_eq!(setup.apply(Event::Chose(Choice::ChangeLocation)), None);
}

#[test]
fn dozens_of_distros_scroll_inside_the_window_and_keep_install_reachable() {
    let names: Vec<String> = (0..30).map(|index| format!("Distro-{index}")).collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let (mut app, heard) = shown(&with_distros(&names));
    assert!(card_fits(&app), "thirty machines fit the window");
    let list = rect(&app, "artisan-setup-machines");
    let install = rect(&app, "artisan-setup-install");
    assert!(
        install.y >= list.y + list.h,
        "Install stays below the scrolling list"
    );
    click(&mut app, "artisan-setup-machine-1");
    click(&mut app, "artisan-setup-install");
    assert_eq!(*heard.borrow(), [Choice::Selected(1), Choice::Started]);
}

fn texts(app: &UiApp) -> Vec<String> {
    app.tree()
        .traverse()
        .into_iter()
        .filter_map(|node| match &app.tree().get(node)?.kind {
            aqua_ui::tree::NodeKind::Text(text) => Some(text.text().to_owned()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_failed_install_offers_a_way_back_to_the_choices() {
    let mut setup = with_three_distros();
    setup.apply(Event::Chose(Choice::Started));
    setup.apply(Event::Finished(Err(InstallError::Refused {
        code: Some(1),
        reason: "the folder is read-only".to_owned(),
    })));
    let (mut app, heard) = shown(&setup);
    click(&mut app, "artisan-setup-back");
    assert_eq!(*heard.borrow(), [Choice::Back]);
    setup.apply(Event::Chose(Choice::Back));
    let (app, _) = shown(&setup);
    assert!(
        texts(&app)
            .iter()
            .any(|text| text == "Where should agents work?")
    );
}

#[test]
fn wsl_discovery_shows_that_it_is_looking_and_why_it_failed() {
    let setup = Setup::fresh("1.2.0", PathBuf::from(r"C:\Artisan Street"), true);
    let (app, _) = shown(&setup);
    assert!(
        texts(&app)
            .iter()
            .any(|text| text == "Looking for WSL distributions…")
    );
    let mut setup = setup;
    setup.apply(Event::DistrosUnavailable("wsl.exe timed out".to_owned()));
    let (mut app, heard) = shown(&setup);
    assert!(
        texts(&app)
            .iter()
            .any(|text| text == "Couldn't list WSL distributions: wsl.exe timed out")
    );
    click(&mut app, "artisan-setup-find-distros");
    assert_eq!(*heard.borrow(), [Choice::FindDistros]);
}

#[test]
fn closing_mid_install_says_the_installer_is_stopping() {
    let mut setup = with_three_distros();
    setup.apply(Event::Chose(Choice::Started));
    setup.apply(Event::CloseRequested);
    let (app, _) = shown(&setup);
    assert!(
        texts(&app)
            .iter()
            .any(|text| text == "Stopping the installer…")
    );
}

fn pulses(app: &mut UiApp) -> bool {
    app.tree_mut()
        .emit_scene()
        .iter()
        .any(|(_, item)| matches!(item, aqua_core::DisplayItem::PushGpuiQuadPulse(_)))
}

#[test]
fn installing_breathes_on_the_gpu_and_holds_still_under_reduced_motion() {
    let mut setup = with_three_distros();
    setup.apply(Event::Chose(Choice::Started));
    assert!(setup.busy());
    let (mut moving, _) = shown_with_motion(&setup, false);
    assert!(pulses(&mut moving));
    let (mut still, _) = shown_with_motion(&setup, true);
    assert!(!pulses(&mut still));
}
