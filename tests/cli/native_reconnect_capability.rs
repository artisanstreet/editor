use artisan_editor_cli::credentials::{
    ForgeCredentialError, RECONNECT_LOCK_TIMEOUT, ReconnectBinding, ReconnectCapabilityStore,
    load_existing_client_identity, provision_or_load,
};
use artisan_protocol::{RECONNECT_CAPABILITY_BYTES, ReconnectCapability};
use std::{
    fs,
    num::NonZeroU32,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

const RECORD_BYTES: usize = 120;
const MAGIC: &[u8; 8] = b"ARTNRC01";
const STATE_OFFSET: usize = 9;
const GENERATION_OFFSET: usize = 10;
const INSTANCE_OFFSET: usize = 18;
const PORT_OFFSET: usize = 34;
const CERTIFICATE_OFFSET: usize = 36;
const PID_OFFSET: usize = 68;
const OWNER_NONCE_OFFSET: usize = 72;
const CAPABILITY_OFFSET: usize = 88;

const _: fn() = || {
    struct DebugMarker;
    trait AmbiguousIfDebug<A> {
        fn marker() {}
    }
    impl<T: ?Sized> AmbiguousIfDebug<()> for T {}
    impl<T: ?Sized + std::fmt::Debug> AmbiguousIfDebug<DebugMarker> for T {}
    let _ = <ReconnectCapability as AmbiguousIfDebug<_>>::marker;
};

const _: fn() = || {
    struct DisplayMarker;
    trait AmbiguousIfDisplay<A> {
        fn marker() {}
    }
    impl<T: ?Sized> AmbiguousIfDisplay<()> for T {}
    impl<T: ?Sized + std::fmt::Display> AmbiguousIfDisplay<DisplayMarker> for T {}
    let _ = <ReconnectCapability as AmbiguousIfDisplay<_>>::marker;
};

const _: fn() = || {
    struct CloneMarker;
    trait AmbiguousIfClone<A> {
        fn marker() {}
    }
    impl<T: ?Sized> AmbiguousIfClone<()> for T {}
    impl<T: Clone> AmbiguousIfClone<CloneMarker> for T {}
    let _ = <ReconnectCapability as AmbiguousIfClone<_>>::marker;
};

fn temp_home(label: &str) -> PathBuf {
    let home = std::env::temp_dir().join(format!(
        "artisan-reconnect-int-{label}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&home);
    fs::create_dir_all(&home).expect("create test home");
    home
}

fn capability(byte: u8) -> ReconnectCapability {
    ReconnectCapability::from_bytes([byte; RECONNECT_CAPABILITY_BYTES])
}

fn binding(byte: u8) -> ReconnectBinding {
    ReconnectBinding::new(
        [byte; 16],
        47_321,
        [byte.wrapping_add(1); 32],
        NonZeroU32::new(41).expect("nonzero test pid"),
    )
    .expect("valid reconnect binding")
}

fn store(home: &Path) -> ReconnectCapabilityStore {
    ReconnectCapabilityStore::new(home).expect("create store facade")
}

fn initialize(home: &Path, _label: &str) -> (ReconnectCapabilityStore, ReconnectBinding) {
    provision_or_load(home).expect("provision identity fixture");
    let store = store(home);
    let owner = binding(0x21);
    store
        .initialize_owner_only(owner, capability(0xa1), Duration::from_millis(100))
        .expect("initialize reconnect record");
    (store, owner)
}

fn record_path(store: &ReconnectCapabilityStore) -> PathBuf {
    store.paths().reconnect_capability_path()
}

fn read_record(store: &ReconnectCapabilityStore) -> Vec<u8> {
    fs::read(record_path(store)).expect("read reconnect record fixture")
}

fn write_record(store: &ReconnectCapabilityStore, bytes: &[u8]) {
    fs::write(record_path(store), bytes).expect("write reconnect record fixture");
}

fn checkout_error(
    store: &ReconnectCapabilityStore,
    owner: ReconnectBinding,
) -> ForgeCredentialError {
    match store.checkout(owner, Duration::from_millis(100)) {
        Ok(attempt) => {
            drop(attempt);
            panic!("malformed or unavailable record unexpectedly checked out")
        }
        Err(error) => error,
    }
}

fn assert_record_header_and_binding(
    bytes: &[u8],
    state: u8,
    generation: u64,
    owner: ReconnectBinding,
) {
    assert_eq!(bytes.len(), RECORD_BYTES);
    assert_eq!(&bytes[..8], MAGIC);
    assert_eq!(bytes[8], 1);
    assert_eq!(bytes[STATE_OFFSET], state);
    assert_eq!(
        u64::from_le_bytes(
            bytes[GENERATION_OFFSET..GENERATION_OFFSET + 8]
                .try_into()
                .expect("generation bytes"),
        ),
        generation
    );
    assert_eq!(
        &bytes[INSTANCE_OFFSET..INSTANCE_OFFSET + 16],
        &owner.instance_id
    );
    assert_eq!(
        u16::from_le_bytes(
            bytes[PORT_OFFSET..PORT_OFFSET + 2]
                .try_into()
                .expect("port bytes"),
        ),
        owner.endpoint_port
    );
    assert_eq!(
        &bytes[CERTIFICATE_OFFSET..CERTIFICATE_OFFSET + 32],
        &owner.certificate_sha256
    );
    assert_eq!(
        u32::from_le_bytes(
            bytes[PID_OFFSET..PID_OFFSET + 4]
                .try_into()
                .expect("pid bytes"),
        ),
        owner.pid.get()
    );
}

fn assert_no_temporary_records(store: &ReconnectCapabilityStore) {
    let directory = store.paths().credentials_dir();
    let temporary = fs::read_dir(directory)
        .expect("read credential directory")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| {
            name.starts_with(".reconnect-capability.bin.")
                && Path::new(name)
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("tmp"))
        })
        .collect::<Vec<_>>();
    assert!(temporary.is_empty(), "temporary reconnect files remain");
}

#[test]
fn zero_instance_identity_is_rejected_at_binding_boundary() {
    assert!(matches!(
        ReconnectBinding::new(
            [0; 16],
            47_321,
            [0x44; 32],
            NonZeroU32::new(41).expect("nonzero test pid"),
        ),
        Err(ForgeCredentialError::ReconnectInvalidBinding)
    ));
}

#[test]
fn consuming_transfer_preserves_secret_fencing_traits() {
    let bytes = capability(0x5a).into_zeroizing_bytes();
    assert_eq!(bytes.as_ref(), &[0x5a; RECONNECT_CAPABILITY_BYTES]);
    drop(bytes);
}

fn assert_invalid_reconnect_record_states(
    store: &ReconnectCapabilityStore,
    owner: ReconnectBinding,
    original: &[u8],
) {
    let mut zero_generation = original.to_vec();
    zero_generation[GENERATION_OFFSET..GENERATION_OFFSET + 8].fill(0);
    write_record(store, &zero_generation);
    assert!(matches!(
        checkout_error(store, owner),
        ForgeCredentialError::ReconnectRecordMalformed
    ));

    let mut zero_instance = original.to_vec();
    zero_instance[INSTANCE_OFFSET..INSTANCE_OFFSET + 16].fill(0);
    write_record(store, &zero_instance);
    assert!(matches!(
        checkout_error(store, owner),
        ForgeCredentialError::ReconnectRecordMalformed
    ));

    let mut zero_port = original.to_vec();
    zero_port[PORT_OFFSET..PORT_OFFSET + 2].fill(0);
    write_record(store, &zero_port);
    assert!(matches!(
        checkout_error(store, owner),
        ForgeCredentialError::ReconnectRecordMalformed
    ));

    let mut zero_pid = original.to_vec();
    zero_pid[PID_OFFSET..PID_OFFSET + 4].fill(0);
    write_record(store, &zero_pid);
    assert!(matches!(
        checkout_error(store, owner),
        ForgeCredentialError::ReconnectRecordMalformed
    ));

    let mut ready_nonce = original.to_vec();
    ready_nonce[OWNER_NONCE_OFFSET] = 0x77;
    write_record(store, &ready_nonce);
    assert!(matches!(
        checkout_error(store, owner),
        ForgeCredentialError::ReconnectRecordMalformed
    ));

    let mut ready_capability = original.to_vec();
    ready_capability[CAPABILITY_OFFSET..].fill(0);
    write_record(store, &ready_capability);
    assert!(matches!(
        checkout_error(store, owner),
        ForgeCredentialError::ReconnectRecordMalformed
    ));

    let mut in_flight_nonce = original.to_vec();
    in_flight_nonce[STATE_OFFSET] = 1;
    in_flight_nonce[OWNER_NONCE_OFFSET..CAPABILITY_OFFSET].fill(0);
    in_flight_nonce[CAPABILITY_OFFSET..].fill(0);
    write_record(store, &in_flight_nonce);
    assert!(matches!(
        checkout_error(store, owner),
        ForgeCredentialError::ReconnectRecordMalformed
    ));

    let mut in_flight_capability = in_flight_nonce.clone();
    in_flight_capability[OWNER_NONCE_OFFSET] = 0x88;
    in_flight_capability[CAPABILITY_OFFSET] = 0x01;
    write_record(store, &in_flight_capability);
    assert!(matches!(
        checkout_error(store, owner),
        ForgeCredentialError::ReconnectRecordMalformed
    ));

    let mut lost_nonce = original.to_vec();
    lost_nonce[STATE_OFFSET] = 2;
    lost_nonce[OWNER_NONCE_OFFSET] = 0x44;
    lost_nonce[CAPABILITY_OFFSET..].fill(0);
    write_record(store, &lost_nonce);
    assert!(matches!(
        checkout_error(store, owner),
        ForgeCredentialError::ReconnectRecordMalformed
    ));

    let mut lost_capability = original.to_vec();
    lost_capability[STATE_OFFSET] = 2;
    lost_capability[OWNER_NONCE_OFFSET..CAPABILITY_OFFSET].fill(0);
    lost_capability[CAPABILITY_OFFSET] = 0x02;
    write_record(store, &lost_capability);
    assert!(matches!(
        checkout_error(store, owner),
        ForgeCredentialError::ReconnectRecordMalformed
    ));
}

#[test]
fn strict_record_round_trip_and_malformed_inputs_are_rejected() {
    let home = temp_home("record");
    let (store, owner) = initialize(&home, "record");
    let original = read_record(&store);
    assert_record_header_and_binding(&original, 0, 1, owner);
    assert_eq!(
        &original[OWNER_NONCE_OFFSET..CAPABILITY_OFFSET],
        &[0_u8; 16]
    );
    assert_eq!(&original[CAPABILITY_OFFSET..], &[0xa1; 32]);

    let mut malformed = vec![0_u8; RECORD_BYTES - 1];
    malformed.copy_from_slice(&original[..RECORD_BYTES - 1]);
    write_record(&store, &malformed);
    assert!(matches!(
        checkout_error(&store, owner),
        ForgeCredentialError::ReconnectRecordMalformed
    ));

    let mut trailing = original.clone();
    trailing.push(0x99);
    write_record(&store, &trailing);
    assert!(matches!(
        checkout_error(&store, owner),
        ForgeCredentialError::ReconnectRecordMalformed
    ));

    let mut wrong_magic = original.clone();
    wrong_magic[0] ^= 0xff;
    let mut wrong_version = original.clone();
    wrong_version[8] = 2;
    let mut invalid_state = original.clone();
    invalid_state[STATE_OFFSET] = 3;
    for candidate in [wrong_magic, wrong_version, invalid_state] {
        write_record(&store, &candidate);
        assert!(matches!(
            checkout_error(&store, owner),
            ForgeCredentialError::ReconnectRecordMalformed
        ));
    }

    assert_invalid_reconnect_record_states(&store, owner, &original);

    write_record(&store, &original);
    assert_no_temporary_records(&store);
    fs::remove_dir_all(home).expect("remove record fixture");
}

#[test]
fn checkout_is_ready_to_in_flight_and_excludes_second_checkout() {
    let home = temp_home("busy");
    let (store, owner) = initialize(&home, "busy");
    let attempt = store
        .checkout(owner, Duration::from_millis(100))
        .expect("checkout ready capability");
    let in_flight = read_record(&store);
    assert_eq!(in_flight[STATE_OFFSET], 1);
    assert_ne!(
        &in_flight[OWNER_NONCE_OFFSET..CAPABILITY_OFFSET],
        &[0_u8; 16]
    );
    assert_eq!(&in_flight[CAPABILITY_OFFSET..], &[0_u8; 32]);

    let second_store = store.clone();
    let second = thread::spawn(move || second_store.checkout(owner, Duration::from_millis(100)));
    let result = second.join().expect("second checkout thread");
    assert!(matches!(result, Err(ForgeCredentialError::CapabilityBusy)));
    drop(attempt);
    assert_eq!(read_record(&store)[STATE_OFFSET], 2);
    fs::remove_dir_all(home).expect("remove busy fixture");
}

#[test]
fn owner_lease_initializes_missing_and_retains_exclusive_lock() {
    let home = temp_home("owner-lease");
    provision_or_load(&home).expect("provision identity fixture");
    let store = store(&home);
    let owner = binding(0x31);
    let lease = store
        .initialize_owner_lease(owner, capability(0xe1), RECONNECT_LOCK_TIMEOUT)
        .expect("initialize owner lease");
    let ready = read_record(&store);
    assert_record_header_and_binding(&ready, 0, 1, owner);
    assert_eq!(&ready[CAPABILITY_OFFSET..], &[0xe1; 32]);

    let second_store = store.clone();
    let second = thread::spawn(move || {
        second_store.initialize_owner_lease(binding(0x32), capability(0xe2), RECONNECT_LOCK_TIMEOUT)
    });
    assert!(matches!(
        second.join().expect("owner lease contention thread"),
        Err(ForgeCredentialError::CapabilityBusy)
    ));

    drop(lease);
    fs::remove_dir_all(home).expect("remove owner-lease fixture");
}

#[test]
fn owner_lease_same_binding_fails_and_different_binding_rebinds_generation() {
    let home = temp_home("owner-rebind");
    provision_or_load(&home).expect("provision identity fixture");
    let store = store(&home);
    let first = binding(0x41);
    let second = binding(0x42);
    let third = binding(0x43);

    let lease = store
        .initialize_owner_lease(first, capability(0xf1), RECONNECT_LOCK_TIMEOUT)
        .expect("initialize first owner lease");
    drop(lease);
    assert!(matches!(
        store.initialize_owner_lease(first, capability(0xf2), RECONNECT_LOCK_TIMEOUT),
        Err(ForgeCredentialError::ReconnectRecordExists)
    ));
    assert_eq!(&read_record(&store)[CAPABILITY_OFFSET..], &[0xf1; 32]);

    let lease = store
        .initialize_owner_lease(second, capability(0xf3), RECONNECT_LOCK_TIMEOUT)
        .expect("rebind second owner lease");
    let rebound = read_record(&store);
    assert_record_header_and_binding(&rebound, 0, 2, second);
    assert_eq!(&rebound[CAPABILITY_OFFSET..], &[0xf3; 32]);
    drop(lease);

    let mut released_in_flight = read_record(&store);
    released_in_flight[STATE_OFFSET] = 1;
    released_in_flight[OWNER_NONCE_OFFSET..CAPABILITY_OFFSET].fill(0x99);
    released_in_flight[CAPABILITY_OFFSET..].fill(0);
    write_record(&store, &released_in_flight);
    let lease = store
        .initialize_owner_lease(third, capability(0xf4), RECONNECT_LOCK_TIMEOUT)
        .expect("rebind released in-flight record");
    let rebound = read_record(&store);
    assert_record_header_and_binding(&rebound, 0, 3, third);
    assert_eq!(&rebound[CAPABILITY_OFFSET..], &[0xf4; 32]);
    drop(lease);

    let attempt = store
        .checkout(third, RECONNECT_LOCK_TIMEOUT)
        .expect("checkout rebound capability");
    drop(attempt);
    assert_eq!(read_record(&store)[STATE_OFFSET], 2);

    let fourth = binding(0x44);
    let lease = store
        .initialize_owner_lease(fourth, capability(0xf5), RECONNECT_LOCK_TIMEOUT)
        .expect("rebind released lost record");
    let rebound = read_record(&store);
    assert_record_header_and_binding(&rebound, 0, 4, fourth);
    assert_eq!(&rebound[CAPABILITY_OFFSET..], &[0xf5; 32]);
    lease.quarantine().expect("quarantine owner lease");
    assert_eq!(read_record(&store)[STATE_OFFSET], 2);

    fs::remove_dir_all(home).expect("remove owner-rebind fixture");
}

#[test]
fn owner_lease_rebind_generation_overflow_fails_closed() {
    let home = temp_home("owner-overflow");
    provision_or_load(&home).expect("provision identity fixture");
    let store = store(&home);
    let first = binding(0x51);
    let second = binding(0x52);
    let lease = store
        .initialize_owner_lease(first, capability(0xf5), RECONNECT_LOCK_TIMEOUT)
        .expect("initialize overflow owner lease");
    drop(lease);
    let mut maximum = read_record(&store);
    maximum[GENERATION_OFFSET..GENERATION_OFFSET + 8].copy_from_slice(&u64::MAX.to_le_bytes());
    write_record(&store, &maximum);

    assert!(matches!(
        store.initialize_owner_lease(second, capability(0xf6), RECONNECT_LOCK_TIMEOUT),
        Err(ForgeCredentialError::ReconnectGenerationOverflow)
    ));
    assert_eq!(read_record(&store), maximum);
    fs::remove_dir_all(home).expect("remove owner-overflow fixture");
}

#[test]
fn owner_lease_quarantine_keeps_lock_until_release() {
    let home = temp_home("owner-shutdown");
    provision_or_load(&home).expect("provision identity fixture");
    let store = store(&home);
    let owner = binding(0x61);
    let lease = store
        .initialize_owner_lease(owner, capability(0xf7), RECONNECT_LOCK_TIMEOUT)
        .expect("initialize shutdown owner lease");
    let lease = lease
        .quarantine_for_shutdown()
        .expect("quarantine before Forge shutdown");
    assert_eq!(read_record(&store)[STATE_OFFSET], 2);

    let second_store = store.clone();
    let second = thread::spawn(move || second_store.checkout(owner, RECONNECT_LOCK_TIMEOUT));
    assert!(matches!(
        second.join().expect("shutdown contention thread"),
        Err(ForgeCredentialError::CapabilityBusy)
    ));
    drop(lease);
    assert!(matches!(
        checkout_error(&store, owner),
        ForgeCredentialError::ReconnectCapabilityUnavailable
    ));
    fs::remove_dir_all(home).expect("remove owner-shutdown fixture");
}

#[test]
fn restore_publish_quarantine_and_generation_are_fenced() {
    let home = temp_home("transitions");
    let (store, owner) = initialize(&home, "transitions");

    let mut attempt = store
        .checkout(owner, Duration::from_millis(100))
        .expect("checkout for restore");
    let credential = attempt.take_credential().expect("take restore credential");
    let restored_lease = attempt
        .restore_before_handshake(credential)
        .expect("restore before handshake");
    drop(restored_lease);
    let restored = read_record(&store);
    assert_record_header_and_binding(&restored, 0, 1, owner);
    assert_eq!(&restored[CAPABILITY_OFFSET..], &[0xa1; 32]);

    let mut attempt = store
        .checkout(owner, Duration::from_millis(100))
        .expect("checkout for publish");
    let credential = attempt.take_credential().expect("take publish credential");
    drop(credential);
    let session_lease = attempt
        .publish_next(owner, capability(0xb2))
        .expect("publish rotated capability");
    let published = read_record(&store);
    assert_record_header_and_binding(&published, 0, 2, owner);
    assert_eq!(&published[CAPABILITY_OFFSET..], &[0xb2; 32]);
    assert_no_temporary_records(&store);

    let second_store = store.clone();
    let second = thread::spawn(move || second_store.checkout(owner, Duration::from_millis(100)));
    let result = second.join().expect("checkout while session lease is held");
    assert!(matches!(result, Err(ForgeCredentialError::CapabilityBusy)));

    let mut attempt = session_lease
        .begin_reconnect()
        .expect("begin quarantine attempt");
    let _credential = attempt
        .take_credential()
        .expect("take quarantine credential");
    attempt.quarantine().expect("quarantine capability");
    let lost = read_record(&store);
    assert_record_header_and_binding(&lost, 2, 2, owner);
    assert_eq!(&lost[OWNER_NONCE_OFFSET..], &[0_u8; 48]);
    assert!(matches!(
        checkout_error(&store, owner),
        ForgeCredentialError::ReconnectCapabilityUnavailable
    ));

    fs::remove_dir_all(home).expect("remove transition fixture");
}

#[test]
fn uncompleted_attempt_fails_closed_and_overflow_does_not_wrap() {
    let home = temp_home("drop");
    let (store, owner) = initialize(&home, "drop");
    {
        let mut attempt = store
            .checkout(owner, Duration::from_millis(100))
            .expect("checkout for dropped attempt");
        let _credential = attempt.take_credential().expect("take dropped credential");
    }
    let lost = read_record(&store);
    assert_record_header_and_binding(&lost, 2, 1, owner);
    fs::remove_dir_all(&home).expect("remove dropped-attempt fixture");

    let home = temp_home("overflow");
    let (store, owner) = initialize(&home, "overflow");
    let mut max_generation = read_record(&store);
    max_generation[GENERATION_OFFSET..GENERATION_OFFSET + 8]
        .copy_from_slice(&u64::MAX.to_le_bytes());
    write_record(&store, &max_generation);
    let mut attempt = store
        .checkout(owner, Duration::from_millis(100))
        .expect("checkout maximum generation");
    let credential = attempt.take_credential().expect("take maximum credential");
    drop(credential);
    assert!(matches!(
        attempt.publish_next(owner, capability(0xc3)),
        Err(ForgeCredentialError::ReconnectGenerationOverflow)
    ));
    let lost = read_record(&store);
    assert_record_header_and_binding(&lost, 2, u64::MAX, owner);

    fs::remove_dir_all(home).expect("remove overflow fixture");
}

#[test]
fn stale_binding_generation_file_id_and_owner_nonce_writers_fail_closed() {
    let cases = [
        ("owner-nonce", 0_u8),
        ("generation", 1_u8),
        ("instance", 2_u8),
        ("endpoint", 3_u8),
        ("certificate", 4_u8),
        ("pid", 5_u8),
        ("file-id", 6_u8),
    ];
    for (label, kind) in cases {
        let home = temp_home(label);
        let (store, owner) = initialize(&home, label);
        let mut attempt = store
            .checkout(owner, Duration::from_millis(100))
            .expect("checkout stale-writer fixture");
        let credential = attempt
            .take_credential()
            .expect("take stale-writer credential");
        let mut current = read_record(&store);
        let error = if kind == 6 {
            let path = record_path(&store);
            let backup = path.with_extension("stale");
            fs::rename(&path, &backup).expect("move original record");
            fs::write(&path, &current).expect("replace record identity");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
                    .expect("restrict replacement record");
            }
            let error = attempt.quarantine().expect_err("file-id drift must fail");
            let _ = fs::remove_file(&backup);
            error
        } else {
            match kind {
                0 => current[OWNER_NONCE_OFFSET] ^= 0x55,
                1 => current[GENERATION_OFFSET..GENERATION_OFFSET + 8]
                    .copy_from_slice(&2_u64.to_le_bytes()),
                2 => current[INSTANCE_OFFSET] ^= 0x01,
                3 => current[PORT_OFFSET] ^= 0x01,
                4 => current[CERTIFICATE_OFFSET] ^= 0x01,
                5 => current[PID_OFFSET] ^= 0x01,
                _ => unreachable!("bounded stale-writer case"),
            }
            write_record(&store, &current);
            if (2..=5).contains(&kind) {
                match attempt.publish_next(owner, capability(0xd4)) {
                    Ok(lease) => {
                        drop(lease);
                        panic!("binding drift unexpectedly published")
                    }
                    Err(error) => error,
                }
            } else {
                drop(credential);
                attempt.quarantine().expect_err("stale writer must fail")
            }
        };
        assert!(matches!(
            error,
            ForgeCredentialError::ReconnectStaleWriter
                | ForgeCredentialError::ReconnectBindingMismatch
        ));
        fs::remove_dir_all(home).expect("remove stale-writer fixture");
    }
}

#[test]
fn identity_loader_is_non_provisioning_and_never_needs_bootstrap_bytes() {
    let home = temp_home("identity-present");
    let paths = provision_or_load(&home).expect("provision identity fixture");
    let expected_certificate =
        fs::read(paths.certificate_paths()[0].as_path()).expect("read certificate fixture");
    let identity = load_existing_client_identity(&home).expect("load existing identity");
    assert_eq!(
        identity.paths().certificate_paths()[0],
        paths.certificate_paths()[0]
    );
    assert_eq!(
        identity.certificate().as_ref(),
        expected_certificate.as_slice()
    );

    fs::write(
        paths.capability_path(),
        b"bootstrap sentinel that is never read",
    )
    .expect("replace bootstrap fixture");
    let identity = load_existing_client_identity(&home).expect("load identity without bootstrap");
    assert_eq!(
        identity.certificate().as_ref(),
        expected_certificate.as_slice()
    );
    fs::remove_dir_all(&home).expect("remove present identity fixture");

    let absent_home = temp_home("identity-absent");
    let absent_paths = provision_or_load(&absent_home).expect("create absent identity fixture");
    for path in [
        absent_paths.manifest_path(),
        absent_paths.certificate_paths()[0].as_path(),
        absent_paths.private_key_path(),
    ] {
        fs::remove_file(path).expect("remove identity fixture");
    }
    let credentials = absent_paths.credentials_dir();
    let bootstrap = credentials.join("bootstrap-capability.bin");
    let sentinel = b"bootstrap must remain untouched";
    fs::write(&bootstrap, sentinel).expect("write bootstrap sentinel");
    let error = match load_existing_client_identity(&absent_home) {
        Ok(identity) => {
            drop(identity);
            panic!("identity unexpectedly exists")
        }
        Err(error) => error,
    };
    let rendered = format!("{error}\n{error:?}");
    assert!(!rendered.contains("bootstrap must remain untouched"));
    assert_eq!(fs::read(&bootstrap).expect("read sentinel"), sentinel);
    assert!(!credentials.join("manifest.json").exists());
    assert!(!credentials.join("localhost-leaf.der").exists());
    assert!(!credentials.join("localhost-key.pkcs8.der").exists());
    fs::remove_dir_all(absent_home).expect("remove absent identity fixture");
}

#[test]
fn private_directory_modes_and_record_symlinks_fail_closed() {
    let home = temp_home("unsafe-path");
    let (store, owner) = initialize(&home, "unsafe-path");
    #[cfg(unix)]
    let directory = store.paths().credentials_dir();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o755))
            .expect("make directory public");
        assert!(checkout_error(&store, owner).to_string().contains("ACL"));
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .expect("restore directory mode");

        fs::set_permissions(record_path(&store), fs::Permissions::from_mode(0o644))
            .expect("make record public");
        assert!(checkout_error(&store, owner).to_string().contains("ACL"));
        fs::set_permissions(record_path(&store), fs::Permissions::from_mode(0o600))
            .expect("restore record mode");
    }

    let path = record_path(&store);
    let backup = path.with_extension("link-target");
    fs::rename(&path, &backup).expect("move record to link target");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&backup, &path).expect("create record symlink");
        drop(checkout_error(&store, owner));
        fs::remove_file(&path).expect("remove record symlink");
    }
    #[cfg(windows)]
    {
        match std::os::windows::fs::symlink_file(&backup, &path) {
            Ok(()) => {
                drop(checkout_error(&store, owner));
                fs::remove_file(&path).expect("remove record reparse point");
            }
            Err(_) => eprintln!("SKIP: record reparse test not supported on this host"),
        }
    }
    fs::rename(&backup, &path).expect("restore record after link test");
    fs::remove_dir_all(home).expect("remove unsafe-path fixture");
}

#[test]
fn reconnect_errors_are_redacted() {
    let home = temp_home("redacted");
    let (store, owner) = initialize(&home, "redacted");
    let mut raw = read_record(&store);
    raw[OWNER_NONCE_OFFSET..CAPABILITY_OFFSET].fill(0xcd);
    write_record(&store, &raw);
    let error = checkout_error(&store, owner);
    let rendered = format!("{error}\n{error:?}");
    assert!(!rendered.contains("cdcd"));
    assert!(!rendered.contains("205"));
    assert!(!rendered.contains("a1a1"));
    let binding = ReconnectBinding::new(
        [0xab; 16],
        47_321,
        [0xcd; 32],
        NonZeroU32::new(41).expect("nonzero test pid"),
    )
    .expect("redaction binding");
    let binding_debug = format!("{binding:?}");
    assert!(!binding_debug.contains("171"));
    assert!(!binding_debug.contains("cd"));
    fs::remove_dir_all(home).expect("remove redaction fixture");
}
