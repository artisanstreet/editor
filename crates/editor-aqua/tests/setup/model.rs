use editor_aqua::setup::model::{
    Effect, Entry, Event, Existing, Kind, Machine, Phase, Setup, entry, existing,
};
use std::path::PathBuf;

fn words(arguments: &[&str]) -> Vec<String> {
    arguments.iter().map(|word| (*word).to_owned()).collect()
}

fn fresh() -> Setup {
    Setup::fresh("1.2.0", PathBuf::from("/default/Artisan Street"))
}

#[test]
fn only_a_downloaded_installer_outside_its_install_folder_sets_up() {
    assert_eq!(
        entry(false, false, &[]),
        Entry::Editor,
        "a dev build always opens"
    );
    assert_eq!(
        entry(true, true, &[]),
        Entry::Editor,
        "the installed Editor opens"
    );
    assert_eq!(entry(true, false, &[]), Entry::Setup);
    assert_eq!(
        entry(true, false, &words(&["artisan://thread/1"])),
        Entry::Setup,
        "a link doesn't skip setup"
    );
    assert_eq!(
        entry(true, false, &words(&["--install", "--forge", "keep"])),
        Entry::Install {
            arguments: words(&["--forge", "keep"])
        }
    );
    assert_eq!(
        entry(true, true, &words(&["--install"])),
        Entry::Editor,
        "the installed Editor never reinstalls itself"
    );
}

#[test]
fn an_older_download_never_replaces_a_newer_install() {
    let nightly = |day: u32| format!("0.0.0-nightly.202611{day:02}220000+abcdef123456");
    assert_eq!(existing(None, &nightly(11)), Existing::Absent);
    assert_eq!(
        existing(Some(&nightly(10)), &nightly(11)),
        Existing::Older(nightly(10))
    );
    assert_eq!(
        existing(Some(&nightly(11)), &nightly(11)),
        Existing::Current
    );
    assert_eq!(
        existing(Some(&nightly(12)), &nightly(11)),
        Existing::Current
    );
    assert_eq!(
        existing(Some("0.5.0-dev+abc.dirty"), "not a version"),
        Existing::Current,
        "an unreadable version leaves the install alone"
    );
}

#[test]
fn choosing_a_wsl_distro_and_a_folder_installs_forge_there() {
    let mut setup = fresh();
    assert_eq!(
        setup.apply(Event::DistrosFound(words(&[
            "Ubuntu-24.04",
            "docker-desktop",
            "docker-desktop-data",
            "Debian"
        ]))),
        None
    );
    assert_eq!(
        setup.machines,
        [
            Machine::ThisPc,
            Machine::Wsl("Ubuntu-24.04".to_owned()),
            Machine::Wsl("Debian".to_owned())
        ],
        "Docker's distributions aren't places to work"
    );
    assert_eq!(setup.selected, 0, "this PC stays the default");

    setup.apply(Event::Selected(2));
    setup.apply(Event::Selected(9));
    assert_eq!(setup.selected, 2, "a row that isn't there selects nothing");
    assert_eq!(setup.apply(Event::ChangeLocation), Some(Effect::PickFolder));
    setup.apply(Event::Located(PathBuf::from("/apps")));
    assert_eq!(setup.location, PathBuf::from("/apps/Artisan Street"));

    assert_eq!(
        setup.apply(Event::Started),
        Some(Effect::Install(words(&[
            "install",
            "--apply",
            "--editor",
            "here",
            "--forge",
            "wsl:Debian",
            "--location",
            "/apps/Artisan Street"
        ])))
    );
    assert_eq!(setup.apply(Event::Selected(0)), None);
    assert_eq!(setup.selected, 2, "nothing changes once installing");
}

#[test]
fn distros_found_late_keep_the_machine_already_chosen() {
    let mut setup = fresh();
    setup.apply(Event::DistrosFound(words(&["Debian", "Ubuntu"])));
    setup.apply(Event::Selected(2));
    setup.apply(Event::DistrosFound(words(&["Ubuntu", "Debian", "Arch"])));
    assert_eq!(
        setup.machines[setup.selected],
        Machine::Wsl("Ubuntu".to_owned())
    );
    setup.apply(Event::DistrosFound(Vec::new()));
    assert_eq!(setup.machines[setup.selected], Machine::ThisPc);
}

#[test]
fn the_default_install_keeps_the_default_folder_and_this_pc() {
    let mut setup = fresh();
    setup.apply(Event::Located(PathBuf::from("/default/Artisan Street")));
    assert_eq!(
        setup.apply(Event::Started),
        Some(Effect::Install(words(&[
            "install", "--apply", "--editor", "here", "--forge", "here"
        ])))
    );
}

#[test]
fn a_failed_install_can_be_tried_again_and_a_finished_one_closes_setup() {
    let mut setup = fresh();
    setup.apply(Event::Started);
    setup.apply(Event::Progressed("Installing Forge 1.2.0".to_owned()));
    assert_eq!(
        setup.phase,
        Phase::Installing {
            line: "Installing Forge 1.2.0".to_owned()
        }
    );
    assert_eq!(
        setup.apply(Event::Finished(Err("Forge didn't start".to_owned()))),
        None
    );
    assert_eq!(
        setup.phase,
        Phase::Failed {
            message: "Forge didn't start".to_owned()
        }
    );
    assert!(matches!(
        setup.apply(Event::Started),
        Some(Effect::Install(_))
    ));
    assert_eq!(setup.apply(Event::Finished(Ok(()))), Some(Effect::Quit));
    assert_eq!(setup.phase, Phase::Done);
    assert_eq!(
        setup.apply(Event::Started),
        None,
        "a finished setup doesn't run twice"
    );
}

#[test]
fn an_update_keeps_forge_where_it_already_is() {
    let mut setup = Setup::update(
        "1.2.0",
        "1.1.0".to_owned(),
        PathBuf::from("/elsewhere/Artisan Street"),
    );
    assert_eq!(
        setup.kind,
        Kind::Update {
            from: "1.1.0".to_owned()
        }
    );
    assert_eq!(
        setup.apply(Event::Started),
        Some(Effect::Install(words(&[
            "install", "--apply", "--editor", "here", "--forge", "keep"
        ])))
    );
}
