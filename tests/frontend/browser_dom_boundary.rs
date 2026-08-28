//! Dependency-free coverage for the browser-DOM failure boundary.

#[path = "../../modules/frontend/src/browser_dom_boundary.rs"]
mod browser_dom_boundary;

use std::error::Error;
use std::fmt;

use browser_dom_boundary::{
    BROWSER_DOM_OPERATION_FAILED, BrowserDomCause, BrowserDomFailure, run_browser_dom,
};

#[derive(Debug)]
struct ReaderFacingError {
    message: String,
    private_detail: &'static str,
}

impl ReaderFacingError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            private_detail: "private host detail",
        }
    }
}

impl fmt::Display for ReaderFacingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ReaderFacingError {}

#[derive(Debug)]
struct OpaqueCause {
    private_detail: &'static str,
}

#[test]
fn successful_values_are_returned_with_their_original_ownership() {
    let expected = Box::new(String::from("host value — owned once"));
    let address = std::ptr::from_ref(expected.as_ref());

    let result = run_browser_dom(|| Ok(expected)).expect("the supplied operation should succeed");

    assert_eq!(std::ptr::from_ref(result.as_ref()), address);
    assert_eq!(result.as_str(), "host value — owned once");
}

#[test]
fn supplied_empty_and_unicode_error_messages_are_preserved_exactly() {
    for message in ["", "DOM operation failed — 失敗 🚀\nwith spacing"] {
        let cause = ReaderFacingError::new(message);
        let failure = run_browser_dom(|| Err::<(), _>(BrowserDomCause::from_error(cause)))
            .expect_err("the supplied error should fail the operation");

        assert_eq!(failure.message(), message);
        assert_eq!(failure.to_string(), message);
    }
}

#[test]
fn opaque_causes_use_the_exact_fallback_without_retaining_private_data() {
    let cause = OpaqueCause {
        private_detail: "opaque secret payload",
    };
    let failure = run_browser_dom(|| {
        let _ = &cause;
        Err::<(), _>(BrowserDomCause::opaque())
    })
    .expect_err("the opaque operation should fail");

    assert_eq!(failure.message(), BROWSER_DOM_OPERATION_FAILED);
    assert_eq!(failure.to_string(), BROWSER_DOM_OPERATION_FAILED);
    assert!(!format!("{failure:?}").contains(cause.private_detail));
    assert!(!failure.to_string().contains(cause.private_detail));
}

#[test]
fn failure_display_debug_and_source_are_payload_safe() {
    let cause = ReaderFacingError::new("reader-facing message");
    let private_detail = cause.private_detail;
    let failure: BrowserDomFailure =
        run_browser_dom(|| Err::<(), _>(BrowserDomCause::from_error(cause)))
            .expect_err("the supplied error should fail the operation");

    assert_eq!(format!("{failure}"), "reader-facing message");
    assert_eq!(failure.message(), "reader-facing message");
    let debug = format!("{failure:?}");
    assert!(debug.contains("BrowserDomFailure"));
    assert!(debug.contains("reader-facing message"));
    assert!(!debug.contains(private_detail));
    assert!(failure.source().is_none());
}

#[test]
fn one_shot_operations_are_invoked_once_on_success_and_failure() {
    let mut success_calls = 0;
    let success = run_browser_dom(|| {
        success_calls += 1;
        Ok(17_u32)
    });
    assert_eq!(success, Ok(17));
    assert_eq!(success_calls, 1);

    let mut failure_calls = 0;
    let failure = run_browser_dom(|| {
        failure_calls += 1;
        Err::<(), _>(BrowserDomCause::opaque())
    });
    assert_eq!(
        failure.expect_err("the operation should fail").message(),
        BROWSER_DOM_OPERATION_FAILED
    );
    assert_eq!(failure_calls, 1);
}

#[test]
fn cause_projections_keep_error_and_opaque_paths_distinct() {
    assert_eq!(
        BrowserDomFailure::from_cause(BrowserDomCause::error("supplied")).message(),
        "supplied"
    );
    assert_eq!(
        BrowserDomFailure::from_cause(BrowserDomCause::opaque()).message(),
        BROWSER_DOM_OPERATION_FAILED
    );
}
