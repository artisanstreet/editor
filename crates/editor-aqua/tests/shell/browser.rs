use editor_aqua::shell::browser::{Browser, pick, program_of};
use std::path::PathBuf;

fn browser(name: &str, program: &str, handler: Option<&str>) -> Browser {
    Browser {
        name: name.to_owned(),
        program: PathBuf::from(program),
        https_handler: handler.map(str::to_owned),
    }
}

#[test]
fn the_program_is_read_from_quoted_and_bare_open_commands() {
    assert_eq!(
        program_of(r#""C:\Users\s\AppData\Local\imput\Helium\Application\chrome.exe" --single-argument %1"#),
        Some(PathBuf::from(
            r"C:\Users\s\AppData\Local\imput\Helium\Application\chrome.exe"
        ))
    );
    assert_eq!(
        program_of(r"C:\Program Files\Mozilla Firefox\firefox.exe -osint -url %1"),
        Some(PathBuf::from(r"C:\Program Files\Mozilla Firefox\firefox.exe"))
    );
    assert_eq!(program_of("  "), None);
}

#[test]
fn the_named_browser_wins_then_the_system_default_then_the_first() {
    let installed = || {
        vec![
            browser("Google Chrome", "chrome.exe", Some("ChromeHTML")),
            browser("Helium", "helium/chrome.exe", Some("HeliumHTM.X")),
            browser("Microsoft Edge", "msedge.exe", Some("MSEdgeHTM")),
        ]
    };
    let name = |preferred, default| pick(installed(), preferred, default).map(|found| found.name);
    assert_eq!(name(Some("helium"), Some("MSEdgeHTM")).as_deref(), Some("Helium"));
    assert_eq!(name(None, Some("msedgehtm")).as_deref(), Some("Microsoft Edge"));
    assert_eq!(
        name(Some("Not installed"), Some("MSEdgeHTM")).as_deref(),
        Some("Microsoft Edge")
    );
    assert_eq!(name(None, None).as_deref(), Some("Google Chrome"));
    assert_eq!(pick(Vec::new(), Some("Helium"), None), None);
}

#[test]
fn a_search_goes_to_the_browsers_own_search_engine() {
    assert_eq!(
        browser("Helium", r"C:\Helium\chrome.exe", None).search_arguments("  fuzzy\n  finder "),
        ["? fuzzy finder"]
    );
    assert_eq!(
        browser("Firefox", r"C:\Mozilla Firefox\firefox.exe", None).search_arguments("fuzzy finder"),
        ["-search", "fuzzy finder"]
    );
}
