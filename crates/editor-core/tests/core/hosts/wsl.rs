use editor_core::hosts::{WslDistro, parse_wsl_listing};

fn distro(name: &str, default: bool, running: bool) -> WslDistro {
    WslDistro {
        name: name.to_owned(),
        default,
        running,
    }
}

#[test]
fn the_listing_reads_each_distro_its_default_mark_and_whether_it_runs() {
    let listing = "  NAME            STATE           VERSION\r\n\
                   * Ubuntu-24.04    Running         2\r\n\
                   \x20 Debian          Stopped         2\r\n\
                   \x20 docker-desktop  Running         2\r\n\
                   \r\n";
    assert_eq!(
        parse_wsl_listing(listing, "Ubuntu-24.04\r\ndocker-desktop\r\n"),
        [
            distro("Ubuntu-24.04", true, true),
            distro("Debian", false, false),
            distro("docker-desktop", false, true),
        ]
    );
}

#[test]
fn a_localized_listing_reads_running_distros_from_the_running_list_not_the_state_words() {
    let listing = "  NAME            STATUS             VERSION\r\n\
                   * Ubuntu          Wird ausgeführt    2\r\n\
                   \x20 Debian          Beendet            2\r\n";
    assert_eq!(
        parse_wsl_listing(listing, "Ubuntu\r\n"),
        [distro("Ubuntu", true, true), distro("Debian", false, false)]
    );
}

#[test]
fn a_running_list_message_never_marks_a_distro_running() {
    let listing = "NAME STATE VERSION\n* Ubuntu Stopped 2\n";
    assert_eq!(
        parse_wsl_listing(listing, "There are no running distributions.\r\n"),
        [distro("Ubuntu", true, false)]
    );
}

#[test]
fn a_default_mark_without_a_space_and_lines_without_a_state_are_handled() {
    assert_eq!(
        parse_wsl_listing("NAME STATE VERSION\n*Arch Running 2\nhalf\n\n", "Arch\n"),
        [distro("Arch", true, true)]
    );
    assert!(parse_wsl_listing("", "").is_empty());
    assert!(parse_wsl_listing("  NAME  STATE  VERSION\n", "").is_empty());
}
