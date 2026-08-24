//! Identifier validation coverage for the shared wire-facing rule.

use artisan_domain::{
    DirectoryId, IDENTIFIER_MAX_BYTES, IdentifierError, MessageId, ProjectId, RequestId, ThreadId,
};

#[test]
fn accepts_representative_ids_for_every_identifier_type() {
    let request = RequestId::parse("01J9Q5V2N6M7Z3A4B5C6").expect("request id is valid");
    let directory = DirectoryId::parse("dir_9f8e7d6c").expect("directory id is valid");
    let project =
        ProjectId::parse("0195f3a2-7bd1-6f10-a5c4-9d2b8e7f1a30").expect("project id is valid");
    let thread = ThreadId::parse("th-000042").expect("thread id is valid");
    let message = MessageId::parse("msg-000007").expect("message id is valid");

    // Each type stands alone: identical text parses into independent types.
    assert_eq!(request.as_str(), "01J9Q5V2N6M7Z3A4B5C6");
    assert_eq!(directory.as_str(), "dir_9f8e7d6c");
    assert_eq!(project.as_str(), "0195f3a2-7bd1-6f10-a5c4-9d2b8e7f1a30");
    assert_eq!(thread.as_str(), "th-000042");
    assert_eq!(message.as_str(), "msg-000007");
}

#[test]
fn round_trips_display_and_from_str() {
    let parsed: RequestId = "request-17".parse().expect("the fixture is valid");

    assert_eq!(parsed.to_string(), "request-17");
    assert_eq!(
        "request-17".parse::<RequestId>(),
        Ok(RequestId::parse("request-17").expect("the fixture is valid"))
    );
}

#[test]
fn rejects_empty_identifiers() {
    assert_eq!(RequestId::parse(""), Err(IdentifierError::Empty));
}

#[test]
fn rejects_whitespace_anywhere_in_identifiers() {
    // A whitespace-only value trips the forbidden-character rule first.
    assert_eq!(
        ThreadId::parse("   "),
        Err(IdentifierError::ForbiddenCharacter { character: ' ' })
    );
    assert_eq!(
        ThreadId::parse("th\t42"),
        Err(IdentifierError::ForbiddenCharacter { character: '\t' })
    );
    assert_eq!(
        MessageId::parse("msg\n42"),
        Err(IdentifierError::ForbiddenCharacter { character: '\n' })
    );
    // Unicode whitespace counts: a non-breaking space is still whitespace.
    assert_eq!(
        ProjectId::parse("proj\u{00a0}42"),
        Err(IdentifierError::ForbiddenCharacter {
            character: '\u{00a0}'
        })
    );
}

#[test]
fn rejects_control_characters_in_identifiers() {
    assert_eq!(
        DirectoryId::parse("dir\u{0000}"),
        Err(IdentifierError::ForbiddenCharacter {
            character: '\u{0000}'
        })
    );
    assert_eq!(
        RequestId::parse("req\u{007f}"),
        Err(IdentifierError::ForbiddenCharacter {
            character: '\u{007f}'
        })
    );
}

#[test]
fn accepts_ids_exactly_at_the_shared_byte_bound() {
    let ascii_at_bound = "a".repeat(IDENTIFIER_MAX_BYTES);
    let multibyte_at_bound = "é".repeat(IDENTIFIER_MAX_BYTES / 2);

    let parsed = DirectoryId::parse(ascii_at_bound).expect("128 ASCII bytes fit the bound");
    assert_eq!(parsed.as_str().len(), IDENTIFIER_MAX_BYTES);

    // 64 two-byte characters equal exactly 128 UTF-8 bytes.
    let parsed_multibyte = ThreadId::parse(multibyte_at_bound).expect("128 UTF-8 bytes fit");
    assert_eq!(
        parsed_multibyte.as_str().chars().count(),
        IDENTIFIER_MAX_BYTES / 2
    );
}

#[test]
fn rejects_ids_beyond_the_shared_byte_bound_with_lengths() {
    let ascii_over_bound = "a".repeat(IDENTIFIER_MAX_BYTES + 1);
    assert_eq!(
        RequestId::parse(ascii_over_bound),
        Err(IdentifierError::TooLong {
            length: IDENTIFIER_MAX_BYTES + 1,
            maximum: IDENTIFIER_MAX_BYTES,
        })
    );

    // 65 two-byte characters equal 130 UTF-8 bytes.
    let multibyte_over_bound = "é".repeat((IDENTIFIER_MAX_BYTES / 2) + 1);
    assert_eq!(
        ProjectId::parse(multibyte_over_bound),
        Err(IdentifierError::TooLong {
            length: IDENTIFIER_MAX_BYTES + 2,
            maximum: IDENTIFIER_MAX_BYTES,
        })
    );
}
