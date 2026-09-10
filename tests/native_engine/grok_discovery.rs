//! Grok discovery tests: precedence, `PATH` lookup (incl. spaces), and the
//! TypeScript version-regex parity matrix. Fixture inputs only; no host
//! environment reads, no spawns.

use artisan_native_engine::grok::discovery::{
    GROK_AUTH_PROBE_ARGS, GROK_BINARY_NAME, GROK_EXECUTABLE_ENV, GROK_VERSION_ARGS,
    GrokResolveSource, explicit_override, find_grok_on_path, parse_grok_version,
    resolve_grok_binary,
};
use std::path::Path;

fn join_path_var(directories: &[&str]) -> String {
    directories.join(if cfg!(windows) { ";" } else { ":" })
}

#[test]
fn executable_env_name_matches_native_override_convention() {
    assert_eq!(GROK_EXECUTABLE_ENV, "ARTISAN_GROK_EXECUTABLE");
    assert_eq!(GROK_BINARY_NAME, "grok");
}

#[test]
fn probe_argument_constants_match_typescript_definition() {
    assert_eq!(GROK_VERSION_ARGS, &["--version"]);
    assert_eq!(GROK_AUTH_PROBE_ARGS, &["--no-auto-update", "models"]);
}

#[test]
fn explicit_override_wins_and_preserves_spaces_verbatim() {
    let resolved = resolve_grok_binary(
        Some("D:\\Custom Tools\\Grok\\grok.exe"),
        Some(&join_path_var(&["C:\\Tools"])),
        |_| true,
    );
    let resolved = resolved.expect("override must resolve");
    assert_eq!(resolved.source(), GrokResolveSource::ExplicitOverride);
    assert_eq!(
        resolved.path(),
        Path::new("D:\\Custom Tools\\Grok\\grok.exe")
    );
}

#[test]
fn blank_override_falls_through_to_path_lookup() {
    let resolved = resolve_grok_binary(Some("   "), Some(&join_path_var(&["dir"])), |_| true);
    let resolved = resolved.expect("lookup must resolve");
    assert_eq!(resolved.source(), GrokResolveSource::PathLookup);
}

#[test]
fn no_override_and_no_path_hit_resolves_to_none() {
    assert_eq!(resolve_grok_binary(None, None, |_| true), None);
    assert_eq!(
        resolve_grok_binary(None, Some(&join_path_var(&["C:\\Tools"])), |_| false),
        None
    );
    assert_eq!(explicit_override(None), None);
    assert_eq!(explicit_override(Some("  ")), None);
}

#[test]
fn path_lookup_finds_executable_under_directory_with_spaces() {
    #[cfg(windows)]
    let directories = ["C:\\Nothing Here", "D:\\Program Files\\Grok"];
    #[cfg(not(windows))]
    let directories = ["/nothing here", "/opt/program files/grok"];
    let binary = if cfg!(windows) { "grok.exe" } else { "grok" };
    let expected = Path::new(directories[1]).join(binary);
    let path_var = join_path_var(&directories);
    let found = find_grok_on_path(Some(&path_var), |path| path == expected);
    assert_eq!(found.as_deref(), Some(expected.as_path()));
}

#[test]
fn path_lookup_prefers_first_directory_with_a_hit() {
    #[cfg(windows)]
    let directories = ["C:\\First", "D:\\Second"];
    #[cfg(not(windows))]
    let directories = ["/first", "/second"];
    let binary = if cfg!(windows) { "grok.exe" } else { "grok" };
    let expected = Path::new(directories[0]).join(binary);
    let path_var = join_path_var(&directories);
    let found = find_grok_on_path(Some(&path_var), |_| true);
    assert_eq!(found.as_deref(), Some(expected.as_path()));
}

#[test]
fn version_regex_accepts_documented_shapes() {
    let accepted = [
        ("grok 1.2.3", "1.2.3"),
        ("Grok 0.142.5", "0.142.5"),
        ("Grok Build grok 2.1.220", "2.1.220"),
        ("grok 1.0.0-beta.1", "1.0.0-beta.1"),
        ("grok 1.0.0-rc_1", "1.0.0-rc_1"),
        ("  grok\t3.4.5\n", "3.4.5"),
        ("some banner\ngrok 10.20.30 (channel stable)", "10.20.30"),
    ];
    for (output, expected) in accepted {
        assert_eq!(
            parse_grok_version(output).as_deref(),
            Some(expected),
            "output: {output:?}"
        );
    }
}

#[test]
fn version_regex_rejects_malformed_and_unprefixed_shapes() {
    let rejected = [
        "",
        "no version here",
        "1.2.3",
        "grok",
        "grok ",
        "grok 1.2",
        "grok version unknown",
        "grokk 1.2.3",
        "agrok 1.2.3",
        "grok v1.2",
    ];
    for output in rejected {
        assert_eq!(parse_grok_version(output), None, "output: {output:?}");
    }
}

#[test]
fn version_parity_notes_for_edge_shapes() {
    // Extra dotted segments: the regex consumes the leading valid triple;
    // the parser does the same.
    assert_eq!(
        parse_grok_version("grok 1.2.3.4.5").as_deref(),
        Some("1.2.3")
    );
    // Trailing `-` with an empty suffix: the optional regex group cannot
    // match, so the base triple stands — same as TypeScript.
    assert_eq!(parse_grok_version("grok 1.2.3-").as_deref(), Some("1.2.3"));
    // `+build` is outside the TypeScript prerelease class; parsing stops
    // before it, same as the regex.
    assert_eq!(
        parse_grok_version("GROK 2.0.0-rc.1+build").as_deref(),
        Some("2.0.0-rc.1")
    );
}
