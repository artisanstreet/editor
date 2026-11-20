use super::*;

#[test]
fn only_a_unit_running_this_forge_is_ours() {
    let forge = Path::new("/home/a/.local/share/artisan-street/forge/forge");
    assert!(unit_runs(
        &unit_text(forge, StopMode::Gateway, Some("/usr/bin")),
        forge
    ));
    let quoted = Path::new("/tmp/quoted \"percent%h\" forge");
    assert!(unit_runs(
        &unit_text(quoted, StopMode::Gateway, Some("/usr/bin")),
        quoted
    ));
    assert!(!unit_runs(
        "[Service]\nExecStart=\"/home/a/.local/share/artisan/bin/ae\" forge serve\n",
        forge
    ));
}

#[cfg(unix)]
#[test]
fn the_service_keeps_lasting_tools_and_drops_a_shells_temporary_ones() {
    let installer = "/nix/store/abc-git/bin:/home/a/.nix-profile/bin:/tmp/nix-shell.x/bin::/usr/bin:/home/a/.nix-profile/bin:/bin";

    let path = service_path(std::ffi::OsStr::new(installer), Path::new("/tmp"));

    assert_eq!(
        path.kept.as_deref(),
        Some("/home/a/.nix-profile/bin:/usr/bin:/bin")
    );
    assert!(path.unrepresentable.is_empty());
}

#[cfg(unix)]
#[test]
fn a_path_folder_that_is_not_unicode_is_left_out_and_named_instead_of_emptying_the_path() {
    use std::os::unix::ffi::OsStrExt as _;
    let installer = std::ffi::OsStr::from_bytes(b"/usr/local/bin:/home/a/caf\xe9/bin:/usr/bin");

    let path = service_path(installer, Path::new("/tmp"));

    assert_eq!(path.kept.as_deref(), Some("/usr/local/bin:/usr/bin"));
    assert_eq!(
        path.unrepresentable,
        [PathBuf::from(std::ffi::OsStr::from_bytes(
            b"/home/a/caf\xe9/bin"
        ))]
    );
}

#[test]
fn a_path_of_only_temporary_tools_leaves_the_service_on_systemds_own_path() {
    let installer = "/nix/store/abc-git/bin:/tmp/nix-shell.x/bin:";

    let path = service_path(std::ffi::OsStr::new(installer), Path::new("/tmp"));

    assert_eq!(path.kept, None);
    let unit = unit_text(
        Path::new("/home/a/.local/share/artisan-street/forge/forge"),
        StopMode::Gateway,
        path.kept.as_deref(),
    );
    assert!(!unit.contains("PATH="), "{unit}");
}

#[test]
fn a_start_that_keeps_logging_waits_and_a_silent_one_stalls() {
    let start = Instant::now();
    let mut watch = StartWatch::new(0, start);
    let later = |seconds| start + Duration::from_secs(seconds);

    assert_eq!(
        watch.observe(Startup::Starting, 100, later(50)),
        Verdict::Waiting
    );
    assert_eq!(
        watch.observe(Startup::Starting, 200, later(100)),
        Verdict::Waiting
    );
    assert_eq!(
        watch.observe(Startup::Starting, 200, later(159)),
        Verdict::Waiting
    );
    assert_eq!(
        watch.observe(Startup::Starting, 200, later(160)),
        Verdict::Stalled
    );
    assert_eq!(
        watch.observe(Startup::Ready, 200, later(500)),
        Verdict::Ready
    );
    assert_eq!(
        watch.observe(Startup::Exited, 200, later(1)),
        Verdict::Exited
    );
}

#[test]
fn an_open_port_counts_as_ready_only_when_restoring_a_forge_that_records_nothing() {
    let open_port = || true;

    assert_eq!(
        judge(Readiness::Recorded, false, true, open_port),
        Startup::Starting,
        "a gateway that bound its port before its backend answered is still starting"
    );
    assert_eq!(
        judge(Readiness::Listening, false, true, open_port),
        Startup::Serving
    );
    assert_eq!(
        judge(Readiness::Recorded, true, true, || unreachable!(
            "a recorded Forge needs no port check"
        )),
        Startup::Ready
    );
    assert_eq!(
        judge(Readiness::Listening, false, false, open_port),
        Startup::Exited
    );
}
