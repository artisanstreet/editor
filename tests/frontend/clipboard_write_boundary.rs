//! Dependency-free coverage for the pure clipboard-write boundary.

#[path = "../../modules/frontend/src/clipboard_write_boundary.rs"]
mod clipboard_write_boundary;

use std::error::Error;
use std::fmt;

use clipboard_write_boundary::{
    ClipboardWriteAdapter, ClipboardWriteError, ClipboardWriteIntent, write_clipboard_text,
};

#[derive(Clone, Debug, Eq, PartialEq)]
struct FakeCause {
    code: u8,
}

impl FakeCause {
    const fn new(code: u8) -> Self {
        Self { code }
    }
}

impl fmt::Display for FakeCause {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "fake clipboard failure {}", self.code)
    }
}

impl Error for FakeCause {}

#[derive(Debug, Default)]
struct FakeClipboardAdapter {
    invocations: usize,
    forwarded: Vec<Vec<u8>>,
    failure: Option<FakeCause>,
}

impl FakeClipboardAdapter {
    fn succeeding() -> Self {
        Self::default()
    }

    fn failing(cause: FakeCause) -> Self {
        Self {
            failure: Some(cause),
            ..Self::default()
        }
    }
}

impl ClipboardWriteAdapter for FakeClipboardAdapter {
    type Error = FakeCause;

    fn write_text(&mut self, text: &str) -> Result<(), Self::Error> {
        self.invocations += 1;
        self.forwarded.push(text.as_bytes().to_vec());
        self.failure.take().map_or(Ok(()), Err)
    }
}

#[test]
fn intent_owns_and_returns_the_exact_text() {
    let expected = String::from("empty stays empty: \n雪🦀");
    let intent = ClipboardWriteIntent::from(expected.clone());

    assert_eq!(intent.text(), expected);
    assert_eq!(intent.into_text(), expected);
}

#[test]
fn forwards_empty_and_unicode_text_byte_for_byte_once_and_succeeds() {
    for expected in ["", "line\n雪 and café 🦀\0"] {
        let mut adapter = FakeClipboardAdapter::succeeding();

        let intent = ClipboardWriteIntent::new(expected);
        let result = write_clipboard_text(&mut adapter, &intent);

        assert_eq!(result, Ok(()));
        assert_eq!(adapter.invocations, 1);
        assert_eq!(adapter.forwarded, vec![expected.as_bytes().to_vec()]);
    }
}

#[test]
fn preserves_the_typed_adapter_failure_without_retrying_or_replacing_it() {
    let expected = "送信内容";
    let cause = FakeCause::new(7);
    let mut adapter = FakeClipboardAdapter::failing(cause.clone());

    let intent = ClipboardWriteIntent::from(expected);
    let error: ClipboardWriteError<FakeCause> = write_clipboard_text(&mut adapter, &intent)
        .expect_err("the deterministic fake should fail");

    assert_eq!(adapter.invocations, 1);
    assert_eq!(adapter.forwarded, vec![expected.as_bytes().to_vec()]);
    assert_eq!(error.cause(), &cause);
    assert_eq!(
        error.source().and_then(|source| source.downcast_ref()),
        Some(&cause)
    );
    assert_eq!(error.into_cause(), cause);
}
