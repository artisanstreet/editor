//! Focused parity coverage for the pure machine-switch policy.
//!
//! The source is included directly so this harness stays dependency-free and
//! can be compiled with `rustc --test` before the VP-owned frontend module and
//! build-file registrations are added.

#[path = "../../modules/frontend/src/machine_switch.rs"]
mod machine_switch;

use machine_switch::{
    HomeHostMemory, build_machine_switch_url, encode_uri_component, validate_home_host_memory,
};

const HANDOFF_PARAMETER: &str = "artisan-handoff";

#[test]
fn home_host_requires_a_non_empty_label() {
    assert_eq!(validate_home_host_memory(None, None), None);
    assert_eq!(validate_home_host_memory(Some(""), None), None);

    assert_eq!(
        validate_home_host_memory(Some(" "), None),
        Some(HomeHostMemory {
            label: " ".to_owned(),
            detail: None,
        })
    );
}

#[test]
fn home_host_drops_only_missing_or_empty_detail() {
    assert_eq!(
        validate_home_host_memory(Some("Laptop"), None),
        Some(HomeHostMemory {
            label: "Laptop".to_owned(),
            detail: None,
        })
    );
    assert_eq!(
        validate_home_host_memory(Some("Laptop"), Some("")),
        Some(HomeHostMemory {
            label: "Laptop".to_owned(),
            detail: None,
        })
    );
    assert_eq!(
        validate_home_host_memory(Some("Laptop"), Some("host.example")),
        Some(HomeHostMemory {
            label: "Laptop".to_owned(),
            detail: Some("host.example".to_owned()),
        })
    );
}

#[test]
fn home_host_validation_retains_unicode_text() {
    assert_eq!(
        validate_home_host_memory(Some("Mý машина"), Some("東京 🚀")),
        Some(HomeHostMemory {
            label: "Mý машина".to_owned(),
            detail: Some("東京 🚀".to_owned()),
        })
    );
}

#[test]
fn encode_uri_component_matches_javascript_unescaped_set() {
    assert_eq!(encode_uri_component("AZaz09-_.!~*'()"), "AZaz09-_.!~*'()");
    assert_eq!(
        encode_uri_component("a b&c=d?/é/🚀%"),
        "a%20b%26c%3Dd%3F%2F%C3%A9%2F%F0%9F%9A%80%25"
    );
}

#[test]
fn http_endpoint_removes_exactly_one_trailing_slash() {
    assert_eq!(
        build_machine_switch_url(
            "http:",
            "https://current.example",
            "http://forge.example///",
            "pair code",
            "ignored nonce",
            HANDOFF_PARAMETER,
        ),
        "http://forge.example///#pair=pair%20code"
    );
    assert_eq!(
        build_machine_switch_url(
            "https:",
            "https://current.example",
            "http://forge.example",
            "pair",
            "ignored nonce",
            HANDOFF_PARAMETER,
        ),
        "http://forge.example/#pair=pair"
    );
}

#[test]
fn http_branch_uses_only_pair_fragment_and_handles_empty_values() {
    assert_eq!(
        build_machine_switch_url(
            "http:",
            "https://current.example",
            "",
            "",
            "nonce",
            HANDOFF_PARAMETER,
        ),
        "/#pair="
    );
}

#[test]
fn desktop_branch_interpolates_nonce_verbatim_before_encoded_fragment() {
    assert_eq!(
        build_machine_switch_url(
            "artisan:",
            "artisan://app",
            "http://127.0.0.1:45870/api/?x=1&y=2",
            "p/é?&",
            "n /é?#&",
            HANDOFF_PARAMETER,
        ),
        "artisan://app/?artisan-handoff=n /é?#&#pair=p%2F%C3%A9%3F%26&forge=http%3A%2F%2F127.0.0.1%3A45870%2Fapi%2F%3Fx%3D1%26y%3D2"
    );
}

#[test]
fn every_non_http_scheme_uses_desktop_navigation_shape() {
    for protocol in ["artisan:", "file:", "", "HTTP:"] {
        let url = build_machine_switch_url(
            protocol,
            "app://origin",
            "endpoint/",
            "pair",
            "nonce",
            HANDOFF_PARAMETER,
        );
        assert_eq!(
            url, "app://origin/?artisan-handoff=nonce#pair=pair&forge=endpoint%2F",
            "unexpected URL for protocol {protocol:?}"
        );
    }
}
