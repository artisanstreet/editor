//! Exhaustive coverage for the stable, safe Artisan frontend error contract.

#[path = "../../modules/frontend/src/artisan_error_code.rs"]
mod artisan_error_code;

use artisan_error_code::{ArtisanErrorCode, format_artisan_error};

const EXPECTED: [(ArtisanErrorCode, &str, &str); 11] = [
    (
        ArtisanErrorCode::ClientCapacityExceeded,
        "CLIENT_CAPACITY_EXCEEDED",
        "Artisan is temporarily at capacity. Please try again.",
    ),
    (
        ArtisanErrorCode::ClientConfiguration,
        "CLIENT_CONFIGURATION",
        "Artisan needs local configuration before it can continue.",
    ),
    (
        ArtisanErrorCode::ClientDisposed,
        "CLIENT_DISPOSED",
        "The Artisan connection was closed. Please reconnect.",
    ),
    (
        ArtisanErrorCode::ClientStateFailure,
        "CLIENT_STATE_FAILURE",
        "Artisan lost synchronization with the local service. Please reconnect.",
    ),
    (
        ArtisanErrorCode::ConnectionUnavailable,
        "CONNECTION_UNAVAILABLE",
        "Artisan could not reach the local Forge service.",
    ),
    (
        ArtisanErrorCode::HydrationProjectsFailed,
        "HYDRATION_PROJECTS_FAILED",
        "Artisan could not load your projects.",
    ),
    (
        ArtisanErrorCode::HydrationSessionDefaultsFailed,
        "HYDRATION_SESSION_DEFAULTS_FAILED",
        "Artisan could not load your session defaults.",
    ),
    (
        ArtisanErrorCode::HydrationThreadsFailed,
        "HYDRATION_THREADS_FAILED",
        "Artisan could not load your threads.",
    ),
    (
        ArtisanErrorCode::PairingRequired,
        "PAIRING_REQUIRED",
        "This Artisan window needs to be paired with Forge.",
    ),
    (
        ArtisanErrorCode::ProtocolFailure,
        "PROTOCOL_FAILURE",
        "Artisan and Forge could not agree on their local protocol.",
    ),
    (
        ArtisanErrorCode::TransportMalformed,
        "TRANSPORT_MALFORMED",
        "Artisan received an invalid local transport response.",
    ),
];

#[test]
fn every_code_has_exact_stable_text_and_message() {
    assert_eq!(ArtisanErrorCode::ALL.len(), EXPECTED.len());

    for (index, &(code, expected_code, expected_message)) in EXPECTED.iter().enumerate() {
        assert_eq!(ArtisanErrorCode::ALL[index], code);
        assert_eq!(code.as_str(), expected_code);
        assert_eq!(code.to_string(), expected_code);
        assert_eq!(code.message(), expected_message);
        assert_eq!(format_artisan_error(code), expected_message);
    }
}

#[test]
fn every_code_round_trips_through_exact_parsing() {
    for &(code, expected_code, _) in &EXPECTED {
        assert_eq!(ArtisanErrorCode::parse(expected_code), Some(code));
        assert_eq!(expected_code.parse::<ArtisanErrorCode>(), Ok(code));
        assert_eq!(code.to_string().parse::<ArtisanErrorCode>(), Ok(code));
    }
}

#[test]
fn unknown_codes_are_rejected_without_retaining_input() {
    let rejected = [
        "",
        "unknown",
        "CLIENT_CONFIGURATION ",
        " client_configuration",
        "client_configuration",
        "CLIENT_CONFIGURATION\ntransport detail",
        "TRANSPORT_MALFORMED_EXTRA",
    ];

    for input in rejected {
        assert_eq!(ArtisanErrorCode::parse(input), None, "input={input:?}");
        let error = input.parse::<ArtisanErrorCode>().unwrap_err();
        assert_eq!(error, artisan_error_code::UnknownArtisanErrorCode);
        assert_eq!(error.to_string(), "unknown Artisan error code");
        assert_eq!(format!("{error:?}"), "UnknownArtisanErrorCode");
    }
}

#[test]
fn stable_code_strings_are_unique_and_table_is_exhaustive() {
    for (index, &(_, expected_code, _)) in EXPECTED.iter().enumerate() {
        assert_eq!(ArtisanErrorCode::ALL[index].as_str(), expected_code);
        for &(_, other_code, _) in EXPECTED.iter().skip(index + 1) {
            assert_ne!(expected_code, other_code);
        }
    }
}

#[test]
fn formatting_is_fixed_and_never_includes_external_detail() {
    let injected_detail = "transport cause: peer supplied secret payload [injected]";

    for &(code, expected_code, expected_message) in &EXPECTED {
        let formatted = format_artisan_error(code);
        assert_eq!(formatted, expected_message);
        assert_eq!(code.as_str(), expected_code);
        assert!(!formatted.contains(injected_detail));
        assert!(!formatted.contains("transport cause"));
        assert!(!formatted.contains("secret payload"));
    }
}
