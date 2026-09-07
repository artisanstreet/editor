//! Focused, dependency-free coverage for the Forge endpoint policy.

#[path = "../../modules/frontend/src/forge_endpoint_policy.rs"]
mod forge_endpoint_policy;

use forge_endpoint_policy::{
    FORGE_ENDPOINT_STORAGE_KEY, ForgeEndpointAdoptionResult, ForgeEndpointStorageIntent,
    ForgeEndpointStorageReadResult, ForgeEndpointStorageWriteOutcome,
    ForgeEndpointStorageWriteResult, MAX_FORGE_ENDPOINT_CODE_UNITS, adopt_forge_endpoint,
    complete_forge_endpoint_adoption, decode_loopback_forge_endpoint, endpoint_bearing_page,
    forge_http_url, is_endpoint_bearing_page, resolve_forge_endpoint,
};

#[test]
fn storage_key_and_candidate_bound_are_exact() {
    assert_eq!(FORGE_ENDPOINT_STORAGE_KEY, "artisan.forge-endpoint");
    assert_eq!(MAX_FORGE_ENDPOINT_CODE_UNITS, 256);
}

#[test]
fn only_non_http_s_pages_are_endpoint_bearing() {
    let cases = [
        ("http:", false),
        ("https:", false),
        ("artisan:", true),
        ("file:", true),
        ("", true),
        ("HTTP:", true),
        ("HTTPS:", true),
    ];

    for (protocol, expected) in cases {
        assert_eq!(
            is_endpoint_bearing_page(protocol),
            expected,
            "protocol {protocol:?} endpoint-bearing decision"
        );
        assert_eq!(endpoint_bearing_page(protocol), expected);
    }
}

#[test]
fn loopback_http_origins_normalize_and_discard_url_suffixes() {
    let cases = [
        (
            "http://127.0.0.1:45870/forge/path?query=value#fragment",
            "http://127.0.0.1:45870",
        ),
        ("HTTP://127.0.0.1:0045870/", "http://127.0.0.1:45870"),
        ("http://[0:0:0:0:0:0:0:1]:4312/api", "http://[::1]:4312"),
        ("http://[::1]:0/", "http://[::1]:0"),
    ];

    for (candidate, expected) in cases {
        assert_eq!(
            decode_loopback_forge_endpoint(candidate).as_deref(),
            Some(expected),
            "candidate {candidate:?} normalization"
        );
    }
}

#[test]
fn whatwg_special_http_and_numeric_ipv4_forms_normalize_for_admission() {
    let accepted = [
        ("http:127.0.0.1:4312", "http://127.0.0.1:4312"),
        ("http:/127.0.0.1:4312", "http://127.0.0.1:4312"),
        ("http:\\127.0.0.1:4312", "http://127.0.0.1:4312"),
        ("http:\\\\127.0.0.1:4312", "http://127.0.0.1:4312"),
        ("http://///127.0.0.1:4312", "http://127.0.0.1:4312"),
        ("http://127.0.0.1:4312\\path", "http://127.0.0.1:4312"),
        ("http://127.1:4312", "http://127.0.0.1:4312"),
        ("http://127.0.0.1.:4312", "http://127.0.0.1:4312"),
        ("http://127.0.1:4312", "http://127.0.0.1:4312"),
        ("http://0177.0.0.1:4312", "http://127.0.0.1:4312"),
        ("http://017700000001:4312", "http://127.0.0.1:4312"),
        ("http://0x7f.0.0.1:4312", "http://127.0.0.1:4312"),
        ("http://0x7f000001:4312", "http://127.0.0.1:4312"),
        ("http://2130706433:4312", "http://127.0.0.1:4312"),
        ("http://%31%32%37.0.0.1:4312", "http://127.0.0.1:4312"),
        ("http://127.0.0.1%2e:4312", "http://127.0.0.1:4312"),
    ];

    for (candidate, expected) in accepted {
        assert_eq!(
            decode_loopback_forge_endpoint(candidate).as_deref(),
            Some(expected),
            "candidate {candidate:?} normalization"
        );
    }

    let rejected = [
        "http://127.2:4312",
        "http://0177.0.0.2:4312",
        "http://0x7f000002:4312",
        "http://2130706434:4312",
        "http://08.0.0.1:4312",
        "http://127.0.0.1..:4312",
        "http://4294967296:4312",
    ];

    for candidate in rejected {
        assert_eq!(
            decode_loopback_forge_endpoint(candidate),
            None,
            "candidate {candidate:?} must be rejected"
        );
    }
}

#[test]
fn credentials_https_and_non_loopback_hosts_are_rejected() {
    let candidates = [
        "http://user:password@127.0.0.1:45870",
        "http://user@127.0.0.1:45870",
        "http://:password@127.0.0.1:45870",
        "https://127.0.0.1:45870",
        "http://localhost:45870",
        "http://127.0.0.2:45870",
        "http://[::2]:45870",
        "http://::1:45870",
    ];

    for candidate in candidates {
        assert_eq!(
            decode_loopback_forge_endpoint(candidate),
            None,
            "candidate {candidate:?} must be rejected"
        );
    }
}

#[test]
fn missing_and_default_ports_are_rejected_but_explicit_non_default_ports_work() {
    let cases = [
        ("http://127.0.0.1", None),
        ("http://127.0.0.1:", None),
        ("http://127.0.0.1:80", None),
        ("http://127.0.0.1:00080/path", None),
        ("http://[::1]", None),
        ("http://[::1]:80", None),
        ("http://127.0.0.1:1", Some("http://127.0.0.1:1")),
        ("http://127.0.0.1:65535", Some("http://127.0.0.1:65535")),
        ("http://[::1]:65535", Some("http://[::1]:65535")),
    ];

    for (candidate, expected) in cases {
        assert_eq!(
            decode_loopback_forge_endpoint(candidate).as_deref(),
            expected,
            "candidate {candidate:?} port decision"
        );
    }
}

#[test]
fn candidate_length_uses_utf16_code_units_at_both_bounds() {
    let prefix = "http://127.0.0.1:123/";
    let remaining = MAX_FORGE_ENDPOINT_CODE_UNITS - prefix.encode_utf16().count();
    let at_limit = format!("{prefix}{}", "a".repeat(remaining));
    let over_limit = format!("{at_limit}a");

    assert_eq!(
        at_limit.encode_utf16().count(),
        MAX_FORGE_ENDPOINT_CODE_UNITS
    );
    assert!(decode_loopback_forge_endpoint(&at_limit).is_some());
    assert_eq!(
        over_limit.encode_utf16().count(),
        MAX_FORGE_ENDPOINT_CODE_UNITS + 1
    );
    assert_eq!(decode_loopback_forge_endpoint(&over_limit), None);
    assert_eq!(decode_loopback_forge_endpoint(""), None);

    let emoji_candidate = format!("{prefix}{}", "🚀".repeat(remaining / 2));
    assert!(emoji_candidate.encode_utf16().count() <= MAX_FORGE_ENDPOINT_CODE_UNITS);
    assert!(decode_loopback_forge_endpoint(&emoji_candidate).is_some());
}

#[test]
fn adoption_emits_only_a_normalized_write_intent_on_endpoint_bearing_pages() {
    let intent = adopt_forge_endpoint("http://127.0.0.1:0045870/path?ignored#ignored", "artisan:");

    assert_eq!(
        intent,
        Some(ForgeEndpointStorageIntent {
            key: FORGE_ENDPOINT_STORAGE_KEY,
            value: String::from("http://127.0.0.1:45870"),
        })
    );
    assert_eq!(
        adopt_forge_endpoint("http://127.0.0.1:45870", "http:"),
        None
    );
    assert_eq!(
        adopt_forge_endpoint("http://127.0.0.1:45870", "https:"),
        None
    );
    assert_eq!(
        adopt_forge_endpoint("https://127.0.0.1:45870", "artisan:"),
        None
    );
}

#[test]
fn both_storage_write_results_keep_a_valid_adoption() {
    let intent = adopt_forge_endpoint("http://[::1]:4312/", "artisan:")
        .expect("valid app-scheme adoption should emit an intent");

    let persisted = complete_forge_endpoint_adoption(
        intent.clone(),
        ForgeEndpointStorageWriteResult::Succeeded,
    );
    assert_eq!(
        persisted,
        ForgeEndpointAdoptionResult {
            endpoint: String::from("http://[::1]:4312"),
            storage: ForgeEndpointStorageWriteOutcome::Persisted,
        }
    );
    assert!(!persisted.storage.is_failure_absorbed());

    let failed = complete_forge_endpoint_adoption(intent, ForgeEndpointStorageWriteResult::Failed);
    assert_eq!(failed.endpoint, "http://[::1]:4312");
    assert_eq!(
        failed.storage,
        ForgeEndpointStorageWriteOutcome::FailureAbsorbed
    );
    assert!(failed.storage.is_failure_absorbed());
}

#[test]
fn invalid_stored_values_and_read_failures_resolve_absent() {
    let cases = [
        (ForgeEndpointStorageReadResult::Missing, None),
        (ForgeEndpointStorageReadResult::ReadFailure, None),
        (
            ForgeEndpointStorageReadResult::Value(String::from("https://127.0.0.1:4312")),
            None,
        ),
        (
            ForgeEndpointStorageReadResult::Value(String::from("http://127.0.0.1")),
            None,
        ),
        (
            ForgeEndpointStorageReadResult::Value(String::from("stale endpoint")),
            None,
        ),
        (
            ForgeEndpointStorageReadResult::Value(String::from("http://[::1]:4312/path")),
            Some(String::from("http://[::1]:4312")),
        ),
    ];

    for (read, expected) in cases {
        assert_eq!(resolve_forge_endpoint(read), expected);
    }
}

#[test]
fn http_url_assembly_preserves_raw_paths_and_concatenates_exactly() {
    let paths = [
        "",
        "/health",
        "api/pair?x=1#fragment",
        "//already-prefixed",
        " https://not-reparsed.example/with spaces ",
    ];

    for path in paths {
        assert_eq!(forge_http_url(path, None), path);
        assert_eq!(
            forge_http_url(path, Some("http://127.0.0.1:4312")),
            format!("http://127.0.0.1:4312{path}")
        );
    }
}
