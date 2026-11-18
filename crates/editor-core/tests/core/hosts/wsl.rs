use editor_core::hosts::{WslDistro, parse_wsl_verbose};

fn distro(name: &str, default: bool, running: bool) -> WslDistro {
    WslDistro {
        name: name.to_owned(),
        default,
        running,
    }
}

#[test]
fn the_listing_reads_each_distro_its_default_mark_and_state() {
    let listing = "  NAME            STATE           VERSION\r\n\
                   * Ubuntu-24.04    Running         2\r\n\
                   \x20 Debian          Stopped         2\r\n\
                   \x20 docker-desktop  RUNNING         2\r\n\
                   \r\n";
    assert_eq!(
        parse_wsl_verbose(listing),
        [
            distro("Ubuntu-24.04", true, true),
            distro("Debian", false, false),
            distro("docker-desktop", false, true),
        ]
    );
}

#[test]
fn a_default_mark_without_a_space_and_lines_without_a_state_are_handled() {
    assert_eq!(
        parse_wsl_verbose("NAME STATE VERSION\n*Arch Running 2\nhalf\n\n"),
        [distro("Arch", true, true)]
    );
    assert!(parse_wsl_verbose("").is_empty());
    assert!(parse_wsl_verbose("  NAME  STATE  VERSION\n").is_empty());
}
