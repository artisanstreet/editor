use editor_aqua::setup::InstallError;
use editor_aqua::setup::model::{
    Choice, Choices, Discovery, Effect, Entry, Event, Existing, Job, Machine, Setup, State, entry,
    existing,
};
use std::path::PathBuf;

fn words(arguments: &[&str]) -> Vec<String> {
    arguments.iter().map(|word| (*word).to_owned()).collect()
}

fn choices(setup: &Setup) -> &Choices {
    match setup.state() {
        State::Choosing(choices) => choices,
        state @ State::Installing { .. }
        | state @ State::Cancelling { .. }
        | state @ State::Failed { .. }
        | state @ State::Done { .. } => panic!("setup is not choosing: {state:?}"),
    }
}

fn refused(reason: &str) -> InstallError {
    InstallError::Refused {
        code: Some(1),
        reason: reason.to_owned(),
    }
}

fn fresh() -> Setup {
    Setup::fresh("1.2.0", PathBuf::from("/default/Artisan Street"), true)
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
        choices(&setup).machines(),
        [
            Machine::ThisPc,
            Machine::Wsl("Ubuntu-24.04".to_owned()),
            Machine::Wsl("Debian".to_owned())
        ],
        "Docker's distributions aren't places to work"
    );
    assert_eq!(
        choices(&setup).selected(),
        &Machine::ThisPc,
        "this PC stays the default"
    );

    setup.apply(Event::Chose(Choice::Selected(2)));
    setup.apply(Event::Chose(Choice::Selected(9)));
    assert_eq!(
        choices(&setup).selected(),
        &Machine::Wsl("Debian".to_owned()),
        "a row that isn't there selects nothing"
    );
    assert_eq!(
        setup.apply(Event::Chose(Choice::ChangeLocation)),
        Some(Effect::PickFolder)
    );
    setup.apply(Event::Chose(Choice::Located(PathBuf::from("/apps"))));
    let chosen = PathBuf::from("/apps").join("Artisan Street");
    assert_eq!(choices(&setup).location().path(), chosen);

    let arguments = words(&[
        "install",
        "--apply",
        "--editor",
        "here",
        "--forge",
        "wsl:Debian",
        "--location",
        &chosen.display().to_string(),
    ]);
    assert_eq!(
        setup.apply(Event::Chose(Choice::Started)),
        Some(Effect::Install(arguments.clone()))
    );
    assert_eq!(setup.apply(Event::Chose(Choice::Selected(0))), None);
    assert_eq!(
        setup.apply(Event::DistrosFound(Vec::new())),
        None,
        "nothing changes once installing"
    );
    setup.apply(Event::Finished(Err(refused("Forge didn't start"))));
    assert_eq!(
        setup.apply(Event::Chose(Choice::Started)),
        Some(Effect::Install(arguments)),
        "trying again installs what was chosen"
    );
}

#[test]
fn distros_found_late_keep_the_machine_already_chosen() {
    let mut setup = fresh();
    setup.apply(Event::DistrosFound(words(&["Debian", "Ubuntu"])));
    setup.apply(Event::Chose(Choice::Selected(2)));
    setup.apply(Event::DistrosFound(words(&["Ubuntu", "Debian", "Arch"])));
    assert_eq!(
        choices(&setup).selected(),
        &Machine::Wsl("Ubuntu".to_owned())
    );
    setup.apply(Event::DistrosFound(Vec::new()));
    assert_eq!(choices(&setup).selected(), &Machine::ThisPc);
}

#[test]
fn the_default_install_keeps_the_default_folder_and_this_pc() {
    let mut setup = fresh();
    setup.apply(Event::Chose(Choice::Located(PathBuf::from(
        "/default/Artisan Street",
    ))));
    assert_eq!(
        setup.apply(Event::Chose(Choice::Started)),
        Some(Effect::Install(words(&[
            "install", "--apply", "--editor", "here", "--forge", "here"
        ])))
    );
}

#[test]
fn a_failed_install_can_be_tried_again_and_a_finished_one_closes_setup() {
    let mut setup = fresh();
    setup.apply(Event::Chose(Choice::Started));
    setup.apply(Event::Progressed("Installing Forge 1.2.0".to_owned()));
    assert!(matches!(
        setup.state(),
        State::Installing { line, .. } if line == "Installing Forge 1.2.0"
    ));
    assert_eq!(
        setup.apply(Event::Finished(Err(refused("Forge didn't start")))),
        None
    );
    assert!(matches!(
        setup.state(),
        State::Failed { error, .. } if error.to_string() == "Forge didn't start"
    ));
    assert!(matches!(
        setup.apply(Event::Chose(Choice::Started)),
        Some(Effect::Install(_))
    ));
    assert_eq!(setup.apply(Event::Finished(Ok(()))), Some(Effect::Quit));
    assert!(matches!(setup.state(), State::Done { .. }));
    assert_eq!(
        setup.apply(Event::Chose(Choice::Started)),
        None,
        "a finished setup doesn't run twice"
    );
}

#[test]
fn an_update_keeps_forge_where_it_already_is() {
    let mut setup = Setup::update("1.2.0", "1.1.0".to_owned());
    assert!(matches!(
        setup.state(),
        State::Installing { job: Job::Update { from }, .. } if from == "1.1.0"
    ));
    let keep = words(&["install", "--apply", "--editor", "here", "--forge", "keep"]);
    assert_eq!(setup.started(), Some(Effect::Install(keep.clone())));
    setup.apply(Event::Finished(Err(refused("Forge didn't stop"))));
    assert_eq!(
        setup.apply(Event::Chose(Choice::Started)),
        Some(Effect::Install(keep))
    );
}

#[test]
fn a_failed_install_can_go_back_and_change_what_was_chosen() {
    let mut setup = fresh();
    setup.apply(Event::DistrosFound(words(&["Debian"])));
    setup.apply(Event::Chose(Choice::Selected(1)));
    setup.apply(Event::Chose(Choice::Started));
    setup.apply(Event::Finished(Err(refused("Debian has no systemd"))));
    assert_eq!(
        setup.apply(Event::Chose(Choice::Back)),
        Some(Effect::FindDistros),
        "going back looks for distributions again"
    );
    assert_eq!(
        choices(&setup).selected(),
        &Machine::Wsl("Debian".to_owned())
    );
    assert_eq!(choices(&setup).discovery(), &Discovery::Searching);
    setup.apply(Event::DistrosFound(words(&["Debian"])));
    setup.apply(Event::Chose(Choice::Selected(0)));
    assert_eq!(
        setup.apply(Event::Chose(Choice::Started)),
        Some(Effect::Install(words(&[
            "install", "--apply", "--editor", "here", "--forge", "here"
        ])))
    );
}

#[test]
fn an_update_that_failed_has_nothing_to_go_back_to() {
    let mut setup = Setup::update("1.2.0", "1.1.0".to_owned());
    setup.apply(Event::Finished(Err(refused("Forge didn't stop"))));
    assert_eq!(setup.apply(Event::Chose(Choice::Back)), None);
    assert!(matches!(setup.state(), State::Failed { .. }));
}

#[test]
fn closing_during_an_install_stops_the_installer_before_setup_closes() {
    let mut setup = fresh();
    setup.apply(Event::Chose(Choice::Started));
    assert!(!setup.closable());
    assert_eq!(setup.apply(Event::CloseRequested), Some(Effect::Cancel));
    assert!(matches!(setup.state(), State::Cancelling { .. }));
    assert!(setup.busy());
    assert!(
        setup.closable(),
        "a second close doesn't wait for an installer that won't stop"
    );
    assert_eq!(setup.apply(Event::CloseRequested), None);
    assert_eq!(
        setup.apply(Event::Finished(Err(InstallError::Cancelled))),
        Some(Effect::Quit)
    );
}

#[test]
fn wsl_discovery_reports_its_progress_and_failure_and_can_look_again() {
    let mut setup = fresh();
    assert_eq!(setup.started(), Some(Effect::FindDistros));
    assert_eq!(choices(&setup).discovery(), &Discovery::Searching);
    setup.apply(Event::DistrosUnavailable("wsl.exe timed out".to_owned()));
    assert_eq!(
        choices(&setup).discovery(),
        &Discovery::Failed("wsl.exe timed out".to_owned())
    );
    assert_eq!(
        setup.apply(Event::Chose(Choice::FindDistros)),
        Some(Effect::FindDistros)
    );
    assert_eq!(choices(&setup).discovery(), &Discovery::Searching);
    setup.apply(Event::DistrosFound(words(&["Ubuntu"])));
    assert_eq!(choices(&setup).discovery(), &Discovery::Found);
    assert_eq!(choices(&setup).machines().len(), 2);
}
