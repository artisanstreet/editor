use artisan_api::now;
use forge_store::{CertificateError, DeviceCertificate, PairCodeHash, Store};

#[tokio::test]
async fn a_pair_code_typed_in_lowercase_with_spaces_still_pairs() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store
        .add_pair_code(PairCodeHash::of("ABCD2345"), now() + 60_000)
        .await
        .unwrap();
    let certificate = DeviceCertificate::from_der(b"editor certificate");
    let paired = store
        .pair_device(
            PairCodeHash::of("abcd 2345"),
            "Editor".into(),
            certificate.clone(),
        )
        .await
        .unwrap();
    assert!(paired.is_some(), "the code pairs however it is typed");
    assert!(store.is_authorized(&certificate).unwrap().is_some());
}

#[test]
fn a_certificate_that_is_not_base64_der_is_refused() {
    assert!(matches!(
        DeviceCertificate::parse("not base64!"),
        Err(CertificateError::NotBase64(_))
    ));
    assert!(matches!(
        DeviceCertificate::parse(""),
        Err(CertificateError::Empty)
    ));
    assert_eq!(
        DeviceCertificate::parse(" ZWRpdG9y ").unwrap(),
        DeviceCertificate::from_der(b"editor")
    );
}
