//! Exhaustive direct coverage for the compact project-path policy.
//!
//! The production module is included directly so this focused harness can be
//! compiled with Rust 1.98 using `rustc --test`, without Cargo, Bazel, or
//! frontend registration changes.

#![forbid(unsafe_code)]

#[path = "../../modules/frontend/src/project_path_policy.rs"]
mod project_path_policy;

use project_path_policy::{
    PathSeparator, PathSeparatorPreference, SeparatorPreference, short_project_path,
    short_project_path_with_preference,
};

type PathCase = (
    &'static str,
    Option<&'static str>,
    Option<PathSeparator>,
    Option<&'static str>,
);
type OutputCase = (&'static str, Option<&'static str>, Option<&'static str>);
type PreferenceCase = (
    &'static str,
    Option<&'static str>,
    SeparatorPreference,
    Option<&'static str>,
);

fn assert_path(
    root_path: &str,
    display_name: Option<&str>,
    separator: Option<PathSeparator>,
    expected: Option<&str>,
) {
    assert_eq!(
        short_project_path(root_path, display_name, separator).as_deref(),
        expected,
        "path={root_path:?} display_name={display_name:?} separator={separator:?}",
    );
}

#[test]
fn empty_and_ecmascript_whitespace_inputs_have_no_result() {
    for input in [
        "",
        " \t\n\u{000b}\u{000c}\r ",
        "\u{00a0}\u{1680}\u{2000}\u{200a}\u{2028}\u{2029}",
        "\u{202f}\u{205f}\u{3000}\u{feff}",
    ] {
        assert_path(input, None, None, None);
    }
}

#[test]
fn documented_example_and_posix_home_rules_match() {
    let cases: &[PathCase] = &[
        (
            r"C:\Users\sander\Desktop\artisan-editor",
            Some("artisan-editor"),
            Some(PathSeparator::Backslash),
            Some(r"~\Desktop"),
        ),
        (
            "/home/sander/Desktop/artisan-editor",
            Some("artisan-editor"),
            None,
            Some("~/Desktop"),
        ),
        (
            "/USERS/sander/Projects/app",
            Some("app"),
            None,
            Some("~/Projects"),
        ),
        ("/home/sander", None, None, Some("~/")),
        ("/home", None, None, Some("/home")),
        ("/users", None, None, Some("/users")),
        (
            "/work/home/sander/project",
            None,
            None,
            Some("/work/…/project"),
        ),
        ("/work/home/sander", None, None, Some("/work/home/sander")),
    ];
    for (path, name, separator, expected) in cases {
        assert_path(path, *name, *separator, *expected);
    }
}

#[test]
fn drive_anchors_home_collapse_and_root_rendering_are_exact() {
    let cases: &[PathCase] = &[
        (
            r"C:/Users/sander/Desktop/artisan-editor",
            Some("artisan-editor"),
            None,
            Some(r"~\Desktop"),
        ),
        (
            r"d:\USERS\sander\Projects\app",
            Some("app"),
            None,
            Some(r"~\Projects"),
        ),
        (r"C:/users/sander", None, None, Some("~\\")),
        (r"C:/", None, None, Some("C:\\")),
        ("C:", None, None, Some("C:/")),
        (r"C:/Users", None, None, Some(r"C:\Users")),
        (
            r"C:/Home/sander/app",
            Some("app"),
            None,
            Some(r"C:\Home\sander"),
        ),
        (
            r"C:/Users/sander/one/two/three/four",
            None,
            None,
            Some(r"~\one\…\four"),
        ),
    ];
    for (path, name, separator, expected) in cases {
        assert_path(path, *name, *separator, *expected);
    }
}

#[test]
fn mixed_and_repeated_separators_normalize_without_changing_dialect() {
    let cases: &[PathCase] = &[
        ("/var//lib///app//", Some("app"), None, Some("/var/lib")),
        (
            r"projects\mixed/app\\nested///leaf",
            None,
            None,
            Some(r"projects\…\leaf"),
        ),
        (
            "projects/mixed\\nested",
            None,
            None,
            Some(r"projects\mixed\nested"),
        ),
        ("relative////", None, None, Some("relative")),
        ("////", None, None, None),
        ("/", None, None, None),
    ];
    for (path, name, separator, expected) in cases {
        assert_path(path, *name, *separator, *expected);
    }
}

#[test]
fn unc_server_and_share_anchors_survive_compaction_and_overrides() {
    let cases: &[PathCase] = &[
        (
            r"//server/share/team/artisan-editor",
            Some("artisan-editor"),
            None,
            Some(r"\\server\share\team"),
        ),
        (
            r"\\SERVER\Share\one\two\three\four",
            None,
            Some(PathSeparator::ForwardSlash),
            Some("//SERVER/Share/one/…/four"),
        ),
        (r"//server/share/", None, None, Some("\\\\server\\share\\")),
        (r"//server/share", None, None, Some("\\\\server\\share\\")),
        (
            r"//server/share//team///project",
            Some("project"),
            None,
            Some(r"\\server\share\team"),
        ),
        ("//server", None, None, Some("/server")),
    ];
    for (path, name, separator, expected) in cases {
        assert_path(path, *name, *separator, *expected);
    }
}

#[test]
fn wsl_prefixes_keep_distro_suffixes_and_use_windows_native_rendering() {
    let cases: &[PathCase] = &[
        (
            r"//wsl$/Ubuntu/home/sander/Desktop/artisan-editor",
            Some("artisan-editor"),
            None,
            Some(r"~\Desktop · Ubuntu (WSL)"),
        ),
        (
            "//WSL.LOCALHOST/Debian/var/lib/project",
            Some("project"),
            Some(PathSeparator::Backslash),
            Some(r"\var\lib · Debian (WSL)"),
        ),
        (
            r"\\wsl$\Fedora\home\sander\src\project",
            Some("project"),
            None,
            Some(r"~\src · Fedora (WSL)"),
        ),
        (
            "//wsl.localhost/Ubuntu/",
            None,
            None,
            Some("/ · Ubuntu (WSL)"),
        ),
        (
            "//wsl$/Ubuntu/home",
            None,
            None,
            Some("/home · Ubuntu (WSL)"),
        ),
        (
            "//wsl$/Ubuntu/home/sander/project",
            None,
            Some(PathSeparator::ForwardSlash),
            Some("~/project · Ubuntu (WSL)"),
        ),
    ];
    for (path, name, separator, expected) in cases {
        assert_path(path, *name, *separator, *expected);
    }
}

#[test]
fn repeated_names_are_posix_exact_but_windows_case_insensitive_and_accent_sensitive() {
    let cases: &[OutputCase] = &[
        ("/tmp/Project", Some("Project"), Some("/tmp")),
        ("/tmp/Project", Some("project"), Some("/tmp/Project")),
        (r"C:\tmp\Project", Some("project"), Some(r"C:\tmp")),
        (r"C:\tmp\café", Some("CAFÉ"), Some(r"C:\tmp")),
        (r"C:\tmp\café", Some("cafe"), Some(r"C:\tmp\café")),
        (r"C:\tmp\Δelta", Some("δELTA"), Some(r"C:\tmp")),
        ("/tmp/café", Some("CAFÉ"), Some("/tmp/café")),
        ("/tmp/Δelta", Some("δELTA"), Some("/tmp/Δelta")),
    ];
    for (path, name, expected) in cases {
        assert_path(path, *name, None, *expected);
    }
}

#[test]
fn shallow_and_deep_contexts_apply_the_middle_ellipsis_after_name_removal() {
    let cases: &[OutputCase] = &[
        ("/one", None, Some("/one")),
        ("/one/two", None, Some("/one/two")),
        ("/one/two/three", None, Some("/one/two/three")),
        ("/one/two/three/four/five", None, Some("/one/…/five")),
        (
            "/one/two/three/project",
            Some("project"),
            Some("/one/two/three"),
        ),
        (r"C:\one\two\three\four", None, Some(r"C:\one\…\four")),
        ("/one/two/three/four", None, Some(r"\one\…\four")),
    ];
    for (path, name, expected) in cases {
        let separator = if *path == "/one/two/three/four" {
            Some(PathSeparator::Backslash)
        } else {
            None
        };
        assert_path(path, *name, separator, *expected);
    }
}

#[test]
fn explicit_separator_preferences_override_both_dialects() {
    let cases: &[PreferenceCase] = &[
        (
            r"C:\Users\sander\Desktop\app",
            Some("app"),
            SeparatorPreference::Native,
            Some(r"~\Desktop"),
        ),
        (
            r"C:\Users\sander\Desktop\app",
            Some("app"),
            SeparatorPreference::ForwardSlash,
            Some("~/Desktop"),
        ),
        (
            "/var/lib/app",
            Some("app"),
            SeparatorPreference::Native,
            Some("/var/lib"),
        ),
        (
            "/var/lib/app",
            Some("app"),
            SeparatorPreference::Backslash,
            Some(r"\var\lib"),
        ),
        (
            r"\\server\share\team\app",
            Some("app"),
            SeparatorPreference::ForwardSlash,
            Some("//server/share/team"),
        ),
    ];
    for (path, name, preference, expected) in cases {
        assert_eq!(
            short_project_path_with_preference(path, *name, *preference).as_deref(),
            *expected,
            "path={path:?} preference={preference:?}",
        );
    }

    let alias: PathSeparatorPreference = SeparatorPreference::ForwardSlash;
    assert_eq!(
        short_project_path_with_preference(r"C:\var\lib\app", Some("app"), alias),
        Some(String::from("C:/var/lib")),
    );
}

#[test]
fn unicode_segments_and_display_names_are_preserved() {
    let cases: &[OutputCase] = &[
        ("/home/Åsa/桌面/项目", None, Some("~/桌面/项目")),
        ("/home/Åsa/桌面/项目/应用", None, Some("~/桌面/项目/应用")),
        (r"C:\tmp\Δelta", Some("δELTA"), Some(r"C:\tmp")),
        ("/tmp/Δelta", Some("δELTA"), Some("/tmp/Δelta")),
        ("/tmp/📁/résumé", None, Some("/tmp/📁/résumé")),
    ];
    for (path, name, expected) in cases {
        assert_path(path, *name, None, *expected);
    }
}
