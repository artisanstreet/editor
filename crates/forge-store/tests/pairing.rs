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
    assert!(store.is_authorized(&certificate).await.unwrap().is_some());
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

#[test]
fn other_requests_run_while_a_device_check_reloads_every_paired_device() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let store = Store::open(dir.path()).unwrap();
        let certificate = DeviceCertificate::from_der(b"editor certificate");
        let device = store
            .add_device("Editor".into(), certificate.clone())
            .await
            .unwrap();
        assert_eq!(
            store.is_authorized(&certificate).await.unwrap(),
            Some(device.clone())
        );
        rusqlite::Connection::open(dir.path().join("forge.sqlite3"))
            .unwrap()
            .execute_batch(include_str!("pairing/many_devices.sql"))
            .unwrap();

        let check = tokio::spawn({
            let store = store.clone();
            async move { store.is_authorized(&certificate).await }
        });
        tokio::spawn(async {}).await.unwrap();
        assert!(
            !check.is_finished(),
            "the device check must reload the paired devices off the thread that serves other requests"
        );
        assert_eq!(check.await.unwrap().unwrap(), Some(device));
    });
}

#[tokio::test]
async fn a_device_rekeyed_by_another_process_is_refused_next() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let certificate = DeviceCertificate::from_der(b"editor certificate");
    let device = store
        .add_device("Editor".into(), certificate.clone())
        .await
        .unwrap();
    assert_eq!(
        store.is_authorized(&certificate).await.unwrap(),
        Some(device.clone())
    );
    rusqlite::Connection::open(dir.path().join("forge.sqlite3"))
        .unwrap()
        .execute(include_str!("pairing/rekey.sql"), [device.as_str()])
        .unwrap();
    assert_eq!(store.is_authorized(&certificate).await.unwrap(), None);
}
