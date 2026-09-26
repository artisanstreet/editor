//! Grok resolution and TypeScript version-regex parity tests. The executable
//! is only ever the Forge-managed generation or the absolute developer
//! override; `PATH` is never searched.

use artisan_native_engine::grok::discovery::{
    GROK_AUTH_PROBE_ARGS, GROK_BINARY_NAME, GROK_EXECUTABLE_ENV, GROK_VERSION_ARGS,
    parse_grok_version, resolve_live,
};

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
fn without_a_managed_install_grok_is_not_resolved_from_path() {
    if std::env::var_os(GROK_EXECUTABLE_ENV).is_none() {
        assert!(resolve_live().is_none());
    }
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
