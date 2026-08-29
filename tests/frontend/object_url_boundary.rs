//! Dependency-free coverage for the native object-URL lifecycle boundary.
//!
//! The implementation is included directly so this packet can run with plain
//! Rust 1.98 without changing frontend module, Cargo, or Bazel registration.

#[path = "../../modules/frontend/src/object_url_boundary.rs"]
mod object_url_boundary;

use std::cell::RefCell;

use object_url_boundary::{
    ObjectUrlCapability, ObjectUrlCreateIntent, ObjectUrlFailure, ObjectUrlReleaseIntent,
    ObjectUrlSource,
};

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct AdapterFailure {
    operation: &'static str,
    code: u16,
}

#[derive(Default)]
struct Calls {
    creates: Vec<ObjectUrlCreateIntent>,
    releases: Vec<ObjectUrlReleaseIntent>,
}

struct FakeObjectUrlAdapter {
    calls: RefCell<Calls>,
    source: ObjectUrlSource,
    create_failure: Option<AdapterFailure>,
    release_failure: Option<AdapterFailure>,
}

impl FakeObjectUrlAdapter {
    fn successful(source: impl Into<ObjectUrlSource>) -> Self {
        Self {
            calls: RefCell::new(Calls::default()),
            source: source.into(),
            create_failure: None,
            release_failure: None,
        }
    }

    fn failing_create(cause: AdapterFailure) -> Self {
        let mut adapter = Self::successful(ObjectUrlSource::new("unused"));
        adapter.create_failure = Some(cause);
        adapter
    }

    fn failing_release(cause: AdapterFailure) -> Self {
        let mut adapter = Self::successful(ObjectUrlSource::new("unused"));
        adapter.release_failure = Some(cause);
        adapter
    }
}

impl ObjectUrlCapability for FakeObjectUrlAdapter {
    type Error = AdapterFailure;

    fn create(
        &self,
        intent: &ObjectUrlCreateIntent,
    ) -> Result<ObjectUrlSource, ObjectUrlFailure<Self::Error>> {
        self.calls.borrow_mut().creates.push(intent.clone());
        match &self.create_failure {
            Some(cause) => Err(ObjectUrlFailure::new(cause.clone())),
            None => Ok(self.source.clone()),
        }
    }

    fn release(
        &self,
        intent: &ObjectUrlReleaseIntent,
    ) -> Result<(), ObjectUrlFailure<Self::Error>> {
        self.calls.borrow_mut().releases.push(intent.clone());
        match &self.release_failure {
            Some(cause) => Err(ObjectUrlFailure::new(cause.clone())),
            None => Ok(()),
        }
    }
}

#[test]
fn create_intent_preserves_empty_and_arbitrary_inputs() {
    let empty = ObjectUrlCreateIntent::new([], "");
    assert!(empty.bytes.is_empty());
    assert_eq!(empty.media_type, "");

    let bytes = vec![0, 255, 1, 0, 128, 254];
    let media_type = "  not a MIME value\0/🌈\n";
    let intent = ObjectUrlCreateIntent::new(bytes.clone(), media_type);

    assert_eq!(intent.bytes, bytes);
    assert_eq!(intent.media_type, media_type);
}

#[test]
fn create_forwards_the_exact_intent_and_returns_the_opaque_source() {
    let intent = ObjectUrlCreateIntent::new(
        vec![0, 255, 3, 0, 254],
        "application/octet-stream; caller text 🌈",
    );
    let adapter = FakeObjectUrlAdapter::successful("opaque source /?%2F#exact");

    let source = adapter
        .create(&intent)
        .expect("the deterministic create adapter should succeed");

    assert_eq!(source.as_str(), "opaque source /?%2F#exact");
    let calls = adapter.calls.borrow();
    assert_eq!(calls.creates, vec![intent]);
    assert!(calls.releases.is_empty());
}

#[test]
fn release_forwards_the_exact_source_and_does_not_require_nonempty_text() {
    let intent = ObjectUrlReleaseIntent::new(ObjectUrlSource::new(""));
    let adapter = FakeObjectUrlAdapter::successful("unused");

    assert_eq!(adapter.release(&intent), Ok(()));

    let calls = adapter.calls.borrow();
    assert_eq!(calls.releases, vec![intent]);
    assert_eq!(calls.releases[0].source.as_str(), "");
    assert!(calls.creates.is_empty());
}

#[test]
fn create_failure_preserves_the_adapter_cause() {
    let intent = ObjectUrlCreateIntent::new(vec![7, 0, 255], "");
    let cause = AdapterFailure {
        operation: "create",
        code: 17,
    };
    let adapter = FakeObjectUrlAdapter::failing_create(cause.clone());

    let failure = adapter
        .create(&intent)
        .expect_err("the deterministic create adapter should fail");

    assert_eq!(failure.cause(), &cause);
    assert_eq!(failure.cause.operation, "create");
    assert_eq!(failure.into_cause(), cause);
    assert_eq!(adapter.calls.borrow().creates, vec![intent]);
}

#[test]
fn release_failure_uses_the_same_typed_cause_boundary() {
    let intent = ObjectUrlReleaseIntent::new("opaque source with spaces/%2F");
    let cause = AdapterFailure {
        operation: "release",
        code: 23,
    };
    let adapter = FakeObjectUrlAdapter::failing_release(cause.clone());

    let failure = adapter
        .release(&intent)
        .expect_err("the deterministic release adapter should fail");

    assert_eq!(failure.cause(), &cause);
    assert_eq!(failure.into_cause(), cause);
    assert_eq!(adapter.calls.borrow().releases, vec![intent]);
}
