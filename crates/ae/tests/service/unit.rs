use super::*;

#[test]
fn only_a_unit_running_this_forge_is_ours() {
    let forge = Path::new("/home/a/.local/share/artisan-street/forge/forge");
    assert!(unit_runs(
        &unit_text(forge, StopMode::Gateway, "/usr/bin"),
        forge
    ));
    let quoted = Path::new("/tmp/quoted \"percent%h\" forge");
    assert!(unit_runs(
        &unit_text(quoted, StopMode::Gateway, "/usr/bin"),
        quoted
    ));
    assert!(!unit_runs(
        "[Service]\nExecStart=\"/home/a/.local/share/artisan/bin/ae\" forge serve\n",
        forge
    ));
}

#[test]
fn the_service_keeps_lasting_tools_and_drops_a_shells_temporary_ones() {
    let installer = "/nix/store/abc-git/bin:/home/a/.nix-profile/bin:/tmp/nix-shell.x/bin::/usr/bin:/home/a/.nix-profile/bin:/bin";

    let path = service_path(installer, Path::new("/tmp"));

    assert_eq!(path, "/home/a/.nix-profile/bin:/usr/bin:/bin");
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
