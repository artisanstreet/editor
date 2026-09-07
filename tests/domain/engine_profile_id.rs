use std::str::FromStr;

use artisan_domain::{ENGINE_PROFILE_ID_MAX_BYTES, EngineProfileId, EngineProfileIdError};

#[test]
fn accepts_the_exact_ascii_profile_id_grammar() {
    for value in ["a", "A0", "work.profile", "work_profile-2", "default"] {
        let id = EngineProfileId::parse(value).expect("profile id should be valid");
        assert_eq!(id.as_str(), value);
        assert_eq!(id.to_string(), value);
        assert_eq!(EngineProfileId::from_str(value), Ok(id));
    }
}

#[test]
fn enforces_the_byte_bound_and_rejects_non_ascii_or_path_syntax() {
    let at_limit = "a".repeat(ENGINE_PROFILE_ID_MAX_BYTES);
    assert_eq!(
        EngineProfileId::parse(&at_limit).unwrap().as_str(),
        at_limit
    );
    assert_eq!(
        EngineProfileId::parse("a".repeat(ENGINE_PROFILE_ID_MAX_BYTES + 1)),
        Err(EngineProfileIdError::TooLong {
            length: ENGINE_PROFILE_ID_MAX_BYTES + 1,
            maximum: ENGINE_PROFILE_ID_MAX_BYTES,
        })
    );

    for value in [
        "", ".", "..", "-leading", "_leading", "a/b", "a\\b", "a:b", "a b", "a\t b", "a\0b", "é",
        "𝄞",
    ] {
        assert!(
            matches!(
                EngineProfileId::parse(value),
                Err(EngineProfileIdError::Empty | EngineProfileIdError::Invalid)
            ),
            "unexpectedly accepted profile id {value:?}"
        );
    }
}

#[test]
fn ordering_equality_hashing_and_debug_are_safe() {
    let work = EngineProfileId::parse("work").unwrap();
    let other = EngineProfileId::parse("other").unwrap();
    assert_eq!(work, EngineProfileId::parse("work").unwrap());
    assert!(other < work);

    let debug = format!("{work:?}");
    assert!(!debug.contains("work"));
    assert!(debug.contains("REDACTED"));
}
