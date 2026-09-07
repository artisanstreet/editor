//! Bounded-text validation coverage: units, ceilings, and blank rejection.

use artisan_domain::{
    DISPLAY_NAME_MAX_BYTES, DisplayName, DisplayNameError, MESSAGE_BODY_MAX_BYTES, MessageBody,
    MessageBodyError, ROOT_PATH_MAX_BYTES, RootPath, RootPathError, THREAD_TITLE_MAX_BYTES,
    ThreadTitle, ThreadTitleError,
};

#[test]
fn accepts_valid_values_for_every_bounded_text_type() {
    let title = ThreadTitle::parse("New thread").expect("the fixture is valid");
    let display_name = DisplayName::parse("artisan-editor").expect("the fixture is valid");
    let root_path =
        RootPath::parse(r"C:\Users\sander\projects\artisan-editor").expect("the fixture is valid");
    let body = MessageBody::parse("Fix the flaky test.\n\nSteps to reproduce follow.")
        .expect("the fixture is valid");

    assert_eq!(title.as_str(), "New thread");
    assert_eq!(display_name.as_str(), "artisan-editor");
    assert_eq!(
        root_path.as_str(),
        r"C:\Users\sander\projects\artisan-editor"
    );
    assert_eq!(
        body.as_str(),
        "Fix the flaky test.\n\nSteps to reproduce follow."
    );
}

#[test]
fn preserves_original_text_without_trimming() {
    let body = MessageBody::parse("  padded submission  ").expect("the fixture is valid");

    // The domain rejects blank input but never rewrites accepted content.
    assert_eq!(body.as_str(), "  padded submission  ");
}

#[test]
fn rejects_blank_values_for_every_bounded_text_type() {
    assert_eq!(ThreadTitle::parse(""), Err(ThreadTitleError::Blank));
    assert_eq!(ThreadTitle::parse("   "), Err(ThreadTitleError::Blank));
    assert_eq!(DisplayName::parse("\t"), Err(DisplayNameError::Blank));
    assert_eq!(RootPath::parse(""), Err(RootPathError::Blank));
    assert_eq!(
        RootPath::parse(" \n "),
        Err(RootPathError::Blank),
        "whitespace-only paths are rejected like blank text"
    );
    assert_eq!(MessageBody::parse(""), Err(MessageBodyError::Blank));
    assert_eq!(
        MessageBody::parse(" \t\n "),
        Err(MessageBodyError::Blank),
        "legacy trims submissions and refuses empty text"
    );
}

#[test]
fn enforces_the_title_bound_in_utf8_bytes() {
    let ascii_at_bound = "x".repeat(THREAD_TITLE_MAX_BYTES);
    let multibyte_at_bound = "é".repeat(THREAD_TITLE_MAX_BYTES / 2);

    ThreadTitle::parse(ascii_at_bound).expect("256 ASCII bytes fit the title bound");
    ThreadTitle::parse(multibyte_at_bound).expect("128 two-byte characters fit the title bound");

    let ascii_over_bound = "x".repeat(THREAD_TITLE_MAX_BYTES + 1);
    assert_eq!(
        ThreadTitle::parse(ascii_over_bound),
        Err(ThreadTitleError::TooLong {
            length: THREAD_TITLE_MAX_BYTES + 1,
            maximum: THREAD_TITLE_MAX_BYTES,
        })
    );

    // Byte units, not character counts: 129 characters exceed the bound.
    let multibyte_over_bound = "é".repeat((THREAD_TITLE_MAX_BYTES / 2) + 1);
    assert_eq!(
        ThreadTitle::parse(multibyte_over_bound),
        Err(ThreadTitleError::TooLong {
            length: THREAD_TITLE_MAX_BYTES + 2,
            maximum: THREAD_TITLE_MAX_BYTES,
        })
    );
}

#[test]
fn enforces_the_display_name_bound_in_utf8_bytes() {
    DisplayName::parse("a".repeat(DISPLAY_NAME_MAX_BYTES))
        .expect("256 ASCII bytes fit the display-name bound");
    assert_eq!(
        DisplayName::parse("a".repeat(DISPLAY_NAME_MAX_BYTES + 1)),
        Err(DisplayNameError::TooLong {
            length: DISPLAY_NAME_MAX_BYTES + 1,
            maximum: DISPLAY_NAME_MAX_BYTES,
        })
    );
}

#[test]
fn enforces_the_root_path_bound_without_canonicalization() {
    RootPath::parse(String::from(r"C:\") + &r"nested\".repeat(100))
        .expect("a long but realistic path fits the root bound");
    assert_eq!(
        RootPath::parse("a".repeat(ROOT_PATH_MAX_BYTES + 1)),
        Err(RootPathError::TooLong {
            length: ROOT_PATH_MAX_BYTES + 1,
            maximum: ROOT_PATH_MAX_BYTES,
        })
    );

    // The domain stores paths verbatim; separators and casing are untouched.
    let verbatim =
        RootPath::parse(r"C:\Users\SANDER/../projects\project").expect("the fixture is valid");
    assert_eq!(verbatim.as_str(), r"C:\Users\SANDER/../projects\project");
}

#[test]
fn preserves_the_legacy_message_body_ceiling() {
    let at_ceiling = "x".repeat(MESSAGE_BODY_MAX_BYTES);
    MessageBody::parse(at_ceiling).expect("65,536 UTF-8 bytes remain the documented ceiling");

    let over_ceiling = "x".repeat(MESSAGE_BODY_MAX_BYTES + 1);
    assert_eq!(
        MessageBody::parse(over_ceiling),
        Err(MessageBodyError::TooLong {
            length: MESSAGE_BODY_MAX_BYTES + 1,
            maximum: MESSAGE_BODY_MAX_BYTES,
        })
    );
}

#[test]
fn exposes_max_bytes_constants_matching_the_documented_bounds() {
    assert_eq!(ThreadTitle::MAX_BYTES, THREAD_TITLE_MAX_BYTES);
    assert_eq!(DisplayName::MAX_BYTES, DISPLAY_NAME_MAX_BYTES);
    assert_eq!(RootPath::MAX_BYTES, ROOT_PATH_MAX_BYTES);
    assert_eq!(MessageBody::MAX_BYTES, MESSAGE_BODY_MAX_BYTES);
}
