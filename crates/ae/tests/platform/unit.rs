use super::*;

#[test]
fn ae_counts_as_on_the_windows_path_however_the_entry_is_cased_or_ended() {
    let dir = Path::new(r"C:\Users\A\AppData\Local\Artisan Street\ae");
    for path in [
        r"C:\Windows;C:\Users\A\AppData\Local\Artisan Street\ae",
        r"c:\users\a\appdata\local\artisan street\AE\;C:\Windows",
        r" C:\Users\A\AppData\Local\Artisan Street\ae ",
    ] {
        assert!(windows_path_lists(path, dir), "{path}");
    }
    for path in [
        r"C:\Windows",
        r"C:\Users\A\AppData\Local\Artisan Street\ae\bin",
        "",
    ] {
        assert!(!windows_path_lists(path, dir), "{path}");
    }
}
