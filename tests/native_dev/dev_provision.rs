//! The runner configures the dev Forge only through the installed `ae
//! setup`; its arguments must be exactly what that grammar accepts.

use std::path::PathBuf;

use artisan_editor_cli::commands::{Cli, Commands};
use clap::Parser as _;
use native_dev::{
    DEV_ADMISSION_CAPACITY, DEV_REQUESTS_PER_CONNECTION, DevPaths, HostAccess, setup_arguments,
};

fn root() -> PathBuf {
    std::env::temp_dir()
        .join("artisan-dev-provision")
        .join("Artisan Street Dev")
}

#[test]
fn setup_arguments_parse_as_ae_setup_with_service_and_host_access() {
    let paths = DevPaths::new(&root()).expect("absolute root");
    let access = HostAccess {
        listen: "auto:4433".to_owned(),
        name: "Ubuntu".to_owned(),
    };
    let mut argv = vec![std::ffi::OsString::from("ae")];
    argv.extend(setup_arguments(&paths, &access));
    let cli = Cli::try_parse_from(argv).expect("ae accepts the runner's setup");
    let Some(Commands::Setup {
        database_path,
        custody_path,
        readiness_path,
        admission_capacity,
        requests_per_connection,
        autostart,
        listen,
        host_name,
        ..
    }) = cli.command
    else {
        panic!("setup command");
    };
    assert_eq!(database_path, paths.database_path());
    assert_eq!(custody_path, paths.custody_path());
    assert_eq!(readiness_path, paths.readiness_path());
    assert_eq!(admission_capacity.get(), DEV_ADMISSION_CAPACITY);
    assert_eq!(requests_per_connection.get(), DEV_REQUESTS_PER_CONNECTION);
    assert!(autostart, "the dev Forge runs as the service");
    assert_eq!(
        listen.map(|address| address.to_string()).as_deref(),
        Some("auto:4433")
    );
    assert_eq!(host_name.as_deref(), Some("Ubuntu"));
}

#[test]
fn the_forge_state_lives_in_the_installation() {
    let paths = DevPaths::new(&root()).expect("absolute root");
    for path in [
        paths.database_path(),
        paths.custody_path(),
        paths.readiness_path(),
    ] {
        assert!(path.starts_with(&paths.home), "{}", path.display());
    }
    assert_eq!(paths.database_path(), paths.home.join("data/forge.sqlite3"));
}
