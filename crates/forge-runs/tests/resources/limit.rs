use crate::resources::{ResourceError, read_regular};
use artisan_api::{Failure, Limited, ResourceFailure, TUNNEL_BODY_MAX_BYTES};

#[test]
fn a_file_larger_than_an_editor_can_carry_is_refused_as_too_large() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("huge.png");
    let file = std::fs::File::create(&path).unwrap();
    let bytes = TUNNEL_BODY_MAX_BYTES as u64 + 1;
    file.set_len(bytes).unwrap();
    drop(file);

    let error = read_regular(std::fs::File::open(&path).unwrap(), &path).unwrap_err();

    assert!(
        matches!(&error, ResourceError::TooLarge { bytes: actual, .. } if *actual == bytes),
        "{error:?}"
    );
    assert_eq!(
        error.wire("huge.png"),
        Failure::Resource(ResourceFailure::TooLarge {
            subject: Limited::File,
            actual: Some(bytes),
            limit: TUNNEL_BODY_MAX_BYTES as u64,
        }),
        "Forge refuses what no Editor transport can carry instead of sending it"
    );
}
