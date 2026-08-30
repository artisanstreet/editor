//! Focused real-crate tests for explicit Forge process assembly.
//!
//! These tests exercise the signal-free public facade with real migrated
//! storage, real loopback binding, and in-process cancellation. The binary's
//! signal registration remains owned by the synchronous library boundary.

use std::{
    ffi::OsString,
    fs,
    net::SocketAddr,
    num::NonZeroU32,
    path::{Path, PathBuf},
    process,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use artisan_backend::{
    ForgeApp, ForgeConfig, ForgeProcessCustody, ListenerLimits,
    forge_runtime::{self, ForgeConfigError, ForgeLaunchConfig, ForgeRuntimeError},
};
use artisan_database::SqliteConfig;
use artisan_domain::UnixMillis;
use artisan_protocol::{
    APPLICATION_PROTOCOL_VERSION, FrameId, Hello, HelloCredential, LOCAL_CAPABILITY_BYTES,
    LocalCapability, ProtocolVersion, VersionOffer, WireEnvelope, WireEnvelopeBody,
};
use artisan_transport::{
    CancelHandle, LOOPBACK_SERVER_NAME, PinnedIdentity, client_config, client_handshake,
};
use quinn::{Connection, Endpoint, VarInt};
use rustls_pki_types::CertificateDer;

const ADMISSION_TIMEOUT: Duration = Duration::from_secs(30);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const DRAIN_TIMEOUT: Duration = Duration::from_secs(1);
const STARTUP_WAIT: Duration = Duration::from_secs(5);
const SHUTDOWN_WAIT: Duration = Duration::from_secs(5);
const FUTURE_WAIT: Duration = Duration::from_secs(10);

const TEST_CAPABILITY: [u8; LOCAL_CAPABILITY_BYTES] = [0x5a; LOCAL_CAPABILITY_BYTES];

static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

const _: fn() = || {
    struct DefaultMarker;
    trait AmbiguousIfDefault<A> {
        fn marker() {}
    }
    impl<T: ?Sized> AmbiguousIfDefault<()> for T {}
    impl<T: Default> AmbiguousIfDefault<DefaultMarker> for T {}
    let _ = <ForgeLaunchConfig as AmbiguousIfDefault<_>>::marker;
};

struct TemporaryDirectory {
    path: PathBuf,
}

impl TemporaryDirectory {
    fn new(label: &str) -> Self {
        let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "artisan-forge-runtime-{label}-{}-{sequence}",
            process::id()
        ));
        fs::create_dir(&path).expect("isolated Forge directory should be created");
        Self { path }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _cleanup_result = fs::remove_dir_all(&self.path);
    }
}

struct Credentials {
    certificate: CertificateDer<'static>,
    certificate_path: PathBuf,
    private_key_path: PathBuf,
    capability_path: PathBuf,
}

fn credentials(directory: &TemporaryDirectory) -> Credentials {
    let certified = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()])
        .expect("test certificate should be generated");
    let certificate = certified.cert.der().clone();
    let certificate_path = directory.path("leaf.der");
    let private_key_path = directory.path("private-key.der");
    let capability_path = directory.path("bootstrap.cap");
    fs::write(&certificate_path, certificate.as_ref()).expect("certificate should be written");
    fs::write(&private_key_path, certified.signing_key.serialize_der())
        .expect("private key should be written");
    fs::write(&capability_path, [0x5a; LOCAL_CAPABILITY_BYTES])
        .expect("bootstrap capability should be written");
    Credentials {
        certificate,
        certificate_path,
        private_key_path,
        capability_path,
    }
}

fn limits() -> ListenerLimits {
    ListenerLimits {
        admission: ADMISSION_TIMEOUT,
        handshake: HANDSHAKE_TIMEOUT,
        next_request: REQUEST_TIMEOUT,
        drain: DRAIN_TIMEOUT,
    }
}

fn config(
    directory: &TemporaryDirectory,
    credentials: &Credentials,
    cancel: Arc<CancelHandle>,
) -> ForgeLaunchConfig {
    ForgeLaunchConfig::new(
        directory.path("forge.sqlite3"),
        directory.path("forge.custody"),
        vec![credentials.certificate_path.clone()],
        credentials.private_key_path.clone(),
        credentials.capability_path.clone(),
        directory.path("forge.ready"),
        limits(),
        NonZeroU32::new(1).expect("one admission is nonzero"),
        NonZeroU32::new(2).expect("two requests are nonzero"),
        cancel,
    )
    .expect("test launch configuration should be explicit and valid")
}

fn client_endpoint(credentials: &Credentials) -> Endpoint {
    let identity = PinnedIdentity::from_certificate(&credentials.certificate);
    let client = client_config(credentials.certificate.clone(), identity)
        .expect("test client configuration should be valid");
    artisan_transport::bind_loopback_client(client).expect("test client should bind")
}

fn hello_envelope() -> WireEnvelope {
    WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id: FrameId::parse("forge-test-hello").expect("test frame id should be valid"),
        sent_at: UnixMillis::from_millis(1),
        body: WireEnvelopeBody::Hello(Hello {
            supported_versions: VersionOffer::new(vec![APPLICATION_PROTOCOL_VERSION])
                .expect("test version offer should be valid"),
            credential: HelloCredential::Initial(LocalCapability::from_bytes(TEST_CAPABILITY)),
        }),
    }
}

async fn authenticate_client(client: &Endpoint, address: SocketAddr) -> Connection {
    let connecting = client
        .connect(address, LOOPBACK_SERVER_NAME)
        .expect("test client should begin connecting");
    let connection = tokio::time::timeout(FUTURE_WAIT, connecting)
        .await
        .expect("test client connection should settle")
        .expect("test client connection should establish");
    let (mut send, mut receive) = connection
        .open_bi()
        .await
        .expect("test control stream should open");
    tokio::time::timeout(
        FUTURE_WAIT,
        client_handshake(&mut send, &mut receive, hello_envelope()),
    )
    .await
    .expect("test handshake should settle")
    .expect("test handshake should succeed");
    drop(send);
    drop(receive);
    connection
}

async fn connect_without_auth(client: &Endpoint, address: SocketAddr) -> Connection {
    let connecting = client
        .connect(address, LOOPBACK_SERVER_NAME)
        .expect("test client should begin connecting");
    tokio::time::timeout(FUTURE_WAIT, connecting)
        .await
        .expect("test transport connection should settle")
        .expect("test transport connection should establish")
}

fn append_path(arguments: &mut Vec<OsString>, option: &str, path: PathBuf) {
    arguments.push(OsString::from(option));
    arguments.push(path.into_os_string());
}

fn parser_arguments(directory: &TemporaryDirectory) -> Vec<OsString> {
    let mut arguments = Vec::new();
    append_path(
        &mut arguments,
        "--database",
        directory.path("database.sqlite3"),
    );
    append_path(
        &mut arguments,
        "--custody",
        directory.path("process.custody"),
    );
    append_path(
        &mut arguments,
        "--certificate-der",
        directory.path("leaf.der"),
    );
    append_path(
        &mut arguments,
        "--certificate-der",
        directory.path("intermediate.der"),
    );
    append_path(
        &mut arguments,
        "--private-key-der",
        directory.path("private-key.der"),
    );
    append_path(
        &mut arguments,
        "--bootstrap-capability",
        directory.path("bootstrap.cap"),
    );
    append_path(&mut arguments, "--ready-file", directory.path("ready.json"));
    arguments.extend([
        OsString::from("--admission-timeout-ms"),
        OsString::from("11"),
        OsString::from("--handshake-timeout-ms"),
        OsString::from("12"),
        OsString::from("--request-timeout-ms"),
        OsString::from("13"),
        OsString::from("--drain-timeout-ms"),
        OsString::from("14"),
        OsString::from("--admission-capacity"),
        OsString::from("3"),
        OsString::from("--requests-per-connection"),
        OsString::from("4"),
    ]);
    arguments
}

fn replace_option_value(arguments: &mut [OsString], option: &str, value: OsString) {
    let position = arguments
        .iter()
        .position(|argument| argument.as_os_str().to_str() == Some(option))
        .expect("test option should exist");
    arguments[position + 1] = value;
}

fn parse(arguments: Vec<OsString>) -> Result<ForgeLaunchConfig, ForgeConfigError> {
    forge_runtime::parse_args(arguments, Arc::new(CancelHandle::new()))
}

#[test]
fn exact_parser_requires_every_field_and_preserves_certificate_order() {
    let directory = TemporaryDirectory::new("parser");
    let cancel = Arc::new(CancelHandle::new());
    let parsed = ForgeLaunchConfig::from_args(parser_arguments(&directory), Arc::clone(&cancel))
        .expect("the exact long-form contract should parse");

    assert_eq!(parsed.database_path(), directory.path("database.sqlite3"));
    assert_eq!(parsed.custody_path(), directory.path("process.custody"));
    assert_eq!(
        parsed.certificate_der_paths(),
        &[
            directory.path("leaf.der"),
            directory.path("intermediate.der"),
        ]
    );
    assert_eq!(
        parsed.private_key_der_path(),
        directory.path("private-key.der")
    );
    assert_eq!(
        parsed.bootstrap_capability_path(),
        directory.path("bootstrap.cap")
    );
    assert_eq!(parsed.ready_file_path(), directory.path("ready.json"));
    assert_eq!(
        parsed.listener_limits().admission,
        Duration::from_millis(11)
    );
    assert_eq!(
        parsed.listener_limits().handshake,
        Duration::from_millis(12)
    );
    assert_eq!(
        parsed.listener_limits().next_request,
        Duration::from_millis(13)
    );
    assert_eq!(parsed.listener_limits().drain, Duration::from_millis(14));
    assert_eq!(parsed.admission_capacity().get(), 3);
    assert_eq!(parsed.requests_per_connection().get(), 4);
    assert!(Arc::ptr_eq(parsed.cancel_handle(), &cancel));
}

#[test]
fn parser_rejects_missing_duplicate_unknown_relative_empty_zero_and_overflow() {
    let directory = TemporaryDirectory::new("parser-rejections");
    let base = parser_arguments(&directory);

    assert!(matches!(
        parse(Vec::new()),
        Err(ForgeConfigError::MissingOption { .. })
    ));
    assert!(matches!(
        parse(vec![OsString::from("--database")]),
        Err(ForgeConfigError::MissingValue { .. })
    ));

    let mut duplicate = base.clone();
    append_path(
        &mut duplicate,
        "--database",
        directory.path("another.sqlite3"),
    );
    assert!(matches!(
        parse(duplicate),
        Err(ForgeConfigError::Duplicate { .. })
    ));

    let mut unknown = base.clone();
    unknown.push(OsString::from("--data-dir"));
    unknown.push(directory.path("legacy").into_os_string());
    assert!(matches!(
        parse(unknown),
        Err(ForgeConfigError::UnknownOption)
    ));

    let mut relative = base.clone();
    replace_option_value(&mut relative, "--database", OsString::from("forge.sqlite3"));
    assert!(matches!(
        parse(relative),
        Err(ForgeConfigError::RelativePath { .. })
    ));

    let mut empty = base.clone();
    replace_option_value(&mut empty, "--ready-file", OsString::new());
    assert!(matches!(
        parse(empty),
        Err(ForgeConfigError::EmptyPath { .. })
    ));

    let mut zero = base.clone();
    replace_option_value(&mut zero, "--admission-capacity", OsString::from("0"));
    assert!(matches!(
        parse(zero),
        Err(ForgeConfigError::ZeroCapacity { .. })
    ));

    let mut capacity_overflow = base.clone();
    replace_option_value(
        &mut capacity_overflow,
        "--requests-per-connection",
        OsString::from("4294967296"),
    );
    assert!(matches!(
        parse(capacity_overflow),
        Err(ForgeConfigError::NumberOverflow { .. })
    ));

    let mut timeout_overflow = base.clone();
    replace_option_value(
        &mut timeout_overflow,
        "--request-timeout-ms",
        OsString::from("18446744073709551616"),
    );
    assert!(matches!(
        parse(timeout_overflow),
        Err(ForgeConfigError::NumberOverflow { .. })
    ));

    let mut malformed = base;
    replace_option_value(&mut malformed, "--drain-timeout-ms", OsString::from("-1"));
    assert!(matches!(
        parse(malformed),
        Err(ForgeConfigError::InvalidNumber { .. })
    ));
}

#[test]
fn explicit_paths_are_the_only_configuration_and_secret_diagnostics_stay_clean() {
    let directory = TemporaryDirectory::new("explicit");
    let credentials = credentials(&directory);
    let cancel = Arc::new(CancelHandle::new());
    let config = config(&directory, &credentials, cancel);
    let debug = format!("{config:?}");
    let capability_hex = "5a".repeat(LOCAL_CAPABILITY_BYTES);
    assert_eq!(config.database_path(), directory.path("forge.sqlite3"));
    assert_eq!(config.custody_path(), directory.path("forge.custody"));
    assert_eq!(
        config.certificate_der_paths(),
        &[credentials.certificate_path.clone()]
    );
    assert_eq!(
        config.private_key_der_path(),
        credentials.private_key_path.as_path()
    );
    assert_eq!(
        config.bootstrap_capability_path(),
        credentials.capability_path.as_path()
    );
    assert_eq!(config.ready_file_path(), directory.path("forge.ready"));
    assert!(!debug.contains(&capability_hex));
    assert!(!debug.contains("database.sqlite3"));

    let invalid_capability = directory.path("invalid.cap");
    fs::write(&invalid_capability, [0xa5; LOCAL_CAPABILITY_BYTES - 1])
        .expect("invalid capability should be written");
    let invalid_config = ForgeLaunchConfig::new(
        directory.path("not-default.sqlite3"),
        directory.path("not-default.custody"),
        vec![credentials.certificate_path.clone()],
        credentials.private_key_path.clone(),
        invalid_capability,
        directory.path("not-default.ready"),
        limits(),
        NonZeroU32::new(1).expect("one is nonzero"),
        NonZeroU32::new(1).expect("one is nonzero"),
        Arc::new(CancelHandle::new()),
    )
    .expect("paths should be explicit even when files are not ready");
    let error = run_config(invalid_config);
    assert_eq!(error.exit_code(), forge_runtime::EXIT_CODE_CONFIGURATION);
    let display = error.to_string();
    let debug = format!("{error:?}");
    assert!(!display.contains("a5a5"));
    assert!(!debug.contains("a5a5"));
    assert!(!directory.path("not-default.sqlite3").exists());
    assert!(!directory.path("not-default.custody").exists());
    assert!(!directory.path("not-default.ready").exists());
}

#[test]
fn exact_capability_length_is_checked_before_custody() {
    let directory = TemporaryDirectory::new("capability-length");
    let credentials = credentials(&directory);
    fs::write(
        &credentials.capability_path,
        [0x7f; LOCAL_CAPABILITY_BYTES + 1],
    )
    .expect("wrong-length capability should be written");
    let error = run_config(config(
        &directory,
        &credentials,
        Arc::new(CancelHandle::new()),
    ));
    assert_eq!(error.exit_code(), forge_runtime::EXIT_CODE_CONFIGURATION);
    assert!(!directory.path("forge.custody").exists());
    assert!(!directory.path("forge.sqlite3").exists());
    assert!(!directory.path("forge.ready").exists());
}

#[test]
fn custody_contention_returns_75_before_sqlite_creation() {
    let directory = TemporaryDirectory::new("custody-contention");
    let credentials = credentials(&directory);
    let custody_path = directory.path("forge.custody");
    let owner = ForgeProcessCustody::acquire(&custody_path).expect("test should own custody");

    let error = run_config(config(
        &directory,
        &credentials,
        Arc::new(CancelHandle::new()),
    ));
    assert_eq!(error.exit_code(), forge_runtime::EXIT_CODE_CUSTODY);
    assert!(!directory.path("forge.sqlite3").exists());
    assert!(!directory.path("forge.ready").exists());
    drop(owner);
}

#[test]
fn storage_failure_returns_70_without_readiness_and_releases_custody() {
    let directory = TemporaryDirectory::new("storage-failure");
    let credentials = credentials(&directory);
    fs::write(directory.path("forge.sqlite3"), b"not an sqlite database")
        .expect("malformed database should be written");

    let error = run_config(config(
        &directory,
        &credentials,
        Arc::new(CancelHandle::new()),
    ));
    assert_eq!(
        error.exit_code(),
        forge_runtime::EXIT_CODE_APPLICATION_STARTUP
    );
    assert!(!directory.path("forge.ready").exists());
    let custody = ForgeProcessCustody::acquire(directory.path("forge.custody"))
        .expect("custody should be released after storage startup failure");
    drop(custody);
}

#[test]
fn invalid_tls_and_existing_readiness_paths_return_71_after_cleanup() {
    let tls_directory = TemporaryDirectory::new("invalid-tls");
    let tls_credentials = credentials(&tls_directory);
    fs::write(&tls_credentials.private_key_path, [0xa5; 8])
        .expect("invalid private key should be written");
    let tls_error = run_config(config(
        &tls_directory,
        &tls_credentials,
        Arc::new(CancelHandle::new()),
    ));
    assert_eq!(
        tls_error.exit_code(),
        forge_runtime::EXIT_CODE_SERVER_STARTUP
    );
    assert!(!tls_directory.path("forge.ready").exists());
    assert!(!format!("{tls_error:?}").contains("a5a5"));
    let tls_custody = ForgeProcessCustody::acquire(tls_directory.path("forge.custody"))
        .expect("custody should be released after TLS startup failure");
    drop(tls_custody);

    let bind_directory = TemporaryDirectory::new("invalid-bind");
    let bind_credentials = credentials(&bind_directory);
    let bind_config = ForgeLaunchConfig::new(
        bind_directory.path("forge.sqlite3"),
        bind_directory.path("forge.custody"),
        vec![bind_credentials.certificate_path.clone()],
        bind_credentials.private_key_path.clone(),
        bind_credentials.capability_path.clone(),
        bind_directory.path("forge.ready"),
        ListenerLimits {
            admission: Duration::MAX,
            ..limits()
        },
        NonZeroU32::new(1).expect("one is nonzero"),
        NonZeroU32::new(1).expect("one is nonzero"),
        Arc::new(CancelHandle::new()),
    )
    .expect("listener-level invalid limits are still explicit configuration");
    let bind_error = run_config(bind_config);
    assert_eq!(
        bind_error.exit_code(),
        forge_runtime::EXIT_CODE_SERVER_STARTUP
    );
    assert!(!bind_directory.path("forge.ready").exists());
    let bind_custody = ForgeProcessCustody::acquire(bind_directory.path("forge.custody"))
        .expect("custody should be released after listener bind failure");
    drop(bind_custody);

    let readiness_directory = TemporaryDirectory::new("existing-readiness");
    let readiness_credentials = credentials(&readiness_directory);
    let sentinel = b"pre-existing readiness target";
    fs::write(readiness_directory.path("forge.ready"), sentinel)
        .expect("sentinel readiness target should be written");
    let readiness_error = run_config(config(
        &readiness_directory,
        &readiness_credentials,
        Arc::new(CancelHandle::new()),
    ));
    assert_eq!(
        readiness_error.exit_code(),
        forge_runtime::EXIT_CODE_SERVER_STARTUP
    );
    assert_eq!(
        fs::read(readiness_directory.path("forge.ready")).expect("sentinel should survive"),
        sentinel
    );
    let readiness_custody = ForgeProcessCustody::acquire(readiness_directory.path("forge.custody"))
        .expect("custody should be released after readiness failure");
    drop(readiness_custody);
}

#[test]
fn readiness_is_exact_and_shutdown_removes_only_this_receipt() {
    let directory = TemporaryDirectory::new("ready-lifecycle");
    let credentials = credentials(&directory);
    let cancel = Arc::new(CancelHandle::new());
    let mut worker = Some(spawn_runtime(config(
        &directory,
        &credentials,
        Arc::clone(&cancel),
    )));
    let ready_path = directory.path("forge.ready");
    let (bytes, value) = wait_for_readiness_or_stop(&ready_path, &cancel, &mut worker);

    assert_eq!(value["schema"].as_str(), Some(forge_runtime::READY_SCHEMA));
    let endpoint: SocketAddr = value["endpoint"]
        .as_str()
        .expect("readiness endpoint should be text")
        .parse()
        .expect("readiness endpoint should be a socket address");
    assert!(endpoint.ip().is_loopback());
    assert_ne!(endpoint.port(), 0);
    let expected_fingerprint = PinnedIdentity::from_certificate(&credentials.certificate).to_hex();
    assert_eq!(
        value["certificate_sha256"].as_str(),
        Some(expected_fingerprint.as_str())
    );
    assert_eq!(value["pid"].as_u64(), Some(u64::from(process::id())));
    let expected = format!(
        "{{\"schema\":\"{}\",\"endpoint\":\"{}\",\"certificate_sha256\":\"{}\",\"pid\":{}}}\n",
        forge_runtime::READY_SCHEMA,
        endpoint,
        expected_fingerprint,
        process::id(),
    );
    assert_eq!(bytes, expected.as_bytes());
    assert!(!String::from_utf8_lossy(&bytes).contains(&"5a".repeat(32)));
    assert!(
        !worker
            .as_ref()
            .expect("readiness worker should still be owned")
            .is_finished(),
        "service must remain alive before cancel"
    );

    cancel.cancel();
    let result = join_within(
        worker
            .take()
            .expect("readiness worker should still be owned"),
        SHUTDOWN_WAIT,
    );
    assert!(
        result.is_ok(),
        "cancellation should be a clean shutdown: {result:?}"
    );
    assert!(!ready_path.exists(), "this run's receipt should be removed");
    assert_no_readiness_temporary(&directory);

    let custody = ForgeProcessCustody::acquire(directory.path("forge.custody"))
        .expect("custody should be reacquirable after clean shutdown");
    drop(custody);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("reopen runtime should build");
    runtime.block_on(async {
        tokio::time::timeout(FUTURE_WAIT, async {
            let app = ForgeApp::start(ForgeConfig::new(SqliteConfig::file(
                directory.path("forge.sqlite3"),
            )))
            .await
            .expect("database should be released and reopenable");
            app.shutdown()
                .await
                .expect("reopened database should close");
        })
        .await
        .expect("reopened database future should be bounded");
    });
}

#[test]
fn accepted_service_failure_maps_to_72_and_keeps_listener_error() {
    let directory = TemporaryDirectory::new("service-failure");
    let credentials = credentials(&directory);
    let cancel = Arc::new(CancelHandle::new());
    let mut worker = Some(spawn_runtime(config(
        &directory,
        &credentials,
        Arc::clone(&cancel),
    )));
    let (_, value) =
        wait_for_readiness_or_stop(&directory.path("forge.ready"), &cancel, &mut worker);
    let address = readiness_endpoint(&value);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("client runtime should build");
    let client = {
        let _entered = runtime.enter();
        client_endpoint(&credentials)
    };
    let connection = runtime.block_on(authenticate_client(&client, address));
    connection.close(VarInt::from_u32(2), b"test service failure");
    let _ =
        runtime.block_on(async { tokio::time::timeout(FUTURE_WAIT, connection.closed()).await });

    let error = join_within(
        worker
            .take()
            .expect("service-failure worker should still be owned"),
        SHUTDOWN_WAIT,
    )
    .expect_err("the accepted listener failure should end the process");
    assert_eq!(error.exit_code(), forge_runtime::EXIT_CODE_SERVICE);
    match &error {
        ForgeRuntimeError::Service(listener_error) => {
            assert!(listener_error.is_service_failure());
            assert!(listener_error.service_cause().is_some());
            assert!(listener_error.as_request_error().is_some());
            assert!(listener_error.drain_error().is_none());
        }
        other => panic!("expected the complete accepted service error, got {other:?}"),
    }
}

#[test]
fn accepted_drain_only_failure_maps_to_73() {
    let directory = TemporaryDirectory::new("drain-failure");
    let credentials = credentials(&directory);
    let cancel = Arc::new(CancelHandle::new());
    let drain_limits = ListenerLimits {
        drain: Duration::ZERO,
        ..limits()
    };
    let launch = ForgeLaunchConfig::new(
        directory.path("forge.sqlite3"),
        directory.path("forge.custody"),
        vec![credentials.certificate_path.clone()],
        credentials.private_key_path.clone(),
        credentials.capability_path.clone(),
        directory.path("forge.ready"),
        drain_limits,
        NonZeroU32::new(1).expect("one admission is nonzero"),
        NonZeroU32::new(1).expect("one request is nonzero"),
        Arc::clone(&cancel),
    )
    .expect("zero drain is explicit configuration");
    let mut worker = Some(spawn_runtime(launch));
    let (_, value) =
        wait_for_readiness_or_stop(&directory.path("forge.ready"), &cancel, &mut worker);
    let address = readiness_endpoint(&value);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("client runtime should build");
    let client = {
        let _entered = runtime.enter();
        client_endpoint(&credentials)
    };
    let connection = runtime.block_on(connect_without_auth(&client, address));
    cancel.cancel();
    let _ =
        runtime.block_on(async { tokio::time::timeout(FUTURE_WAIT, connection.closed()).await });

    let error = join_within(
        worker
            .take()
            .expect("drain-failure worker should still be owned"),
        SHUTDOWN_WAIT,
    )
    .expect_err("the zero-limit listener drain should fail");
    assert_eq!(error.exit_code(), forge_runtime::EXIT_CODE_SHUTDOWN);
    match &error {
        ForgeRuntimeError::ListenerDrain(listener_error) => {
            assert!(!listener_error.is_service_failure());
            assert!(listener_error.is_drain_failure());
            assert!(listener_error.drain_error().is_some());
            assert!(listener_error.service_cause().is_none());
        }
        other => panic!("expected the accepted drain-only error, got {other:?}"),
    }
}

#[test]
fn service_primary_survives_readiness_cleanup_failure_with_typed_cleanup() {
    let directory = TemporaryDirectory::new("service-primary-cleanup");
    let credentials = credentials(&directory);
    let cancel = Arc::new(CancelHandle::new());
    let mut worker = Some(spawn_runtime(config(
        &directory,
        &credentials,
        Arc::clone(&cancel),
    )));
    let ready_path = directory.path("forge.ready");
    let (_, value) = wait_for_readiness_or_stop(&ready_path, &cancel, &mut worker);
    let address = readiness_endpoint(&value);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("client runtime should build");
    let client = {
        let _entered = runtime.enter();
        client_endpoint(&credentials)
    };
    let connection = runtime.block_on(authenticate_client(&client, address));

    fs::remove_file(&ready_path).expect("the owned readiness receipt should be removable");
    fs::write(&ready_path, b"replacement readiness target")
        .expect("the replacement readiness target should be written");
    connection.close(VarInt::from_u32(2), b"test primary failure");
    let _ =
        runtime.block_on(async { tokio::time::timeout(FUTURE_WAIT, connection.closed()).await });

    let error = join_within(
        worker
            .take()
            .expect("primary-cleanup worker should still be owned"),
        SHUTDOWN_WAIT,
    )
    .expect_err("service failure should remain observable");
    assert_eq!(error.exit_code(), forge_runtime::EXIT_CODE_SERVICE);
    let composite = error
        .as_primary_with_cleanup()
        .expect("cleanup must be correlated with the service primary");
    assert!(matches!(
        composite.primary(),
        ForgeRuntimeError::Service(listener_error) if listener_error.is_service_failure()
    ));
    assert!(
        composite
            .cleanup_failures()
            .iter()
            .any(|failure| matches!(failure, ForgeRuntimeError::ReadinessCleanup(_)))
    );
    assert_eq!(
        fs::read(&ready_path).expect("replacement readiness target should survive cleanup"),
        b"replacement readiness target"
    );
    assert_no_readiness_temporary(&directory);
    assert_eq!(
        composite.primary().exit_code(),
        forge_runtime::EXIT_CODE_SERVICE
    );
}

#[test]
fn second_forge_cannot_start_until_first_releases_custody() {
    let directory = TemporaryDirectory::new("second-forge");
    let credentials = credentials(&directory);
    let first_cancel = Arc::new(CancelHandle::new());
    let mut first = Some(spawn_runtime(config(
        &directory,
        &credentials,
        Arc::clone(&first_cancel),
    )));
    let _ready =
        wait_for_readiness_or_stop(&directory.path("forge.ready"), &first_cancel, &mut first);

    let second_error = run_config(config(
        &directory,
        &credentials,
        Arc::new(CancelHandle::new()),
    ));
    assert_eq!(second_error.exit_code(), forge_runtime::EXIT_CODE_CUSTODY);

    first_cancel.cancel();
    assert!(
        join_within(
            first
                .take()
                .expect("first Forge worker should still be owned"),
            SHUTDOWN_WAIT,
        )
        .is_ok()
    );
    let reacquired = ForgeProcessCustody::acquire(directory.path("forge.custody"))
        .expect("custody should be available after first Forge shutdown");
    drop(reacquired);
}

#[test]
fn helper_dispatch_absent_keeps_normal_runtime_unconstructed() {
    assert!(
        artisan_backend::directory_helper::run_if_requested().is_none(),
        "the ordinary test invocation must not select helper mode"
    );
}

fn run_config(config: ForgeLaunchConfig) -> ForgeRuntimeError {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime should build");
    runtime
        .block_on(async { tokio::time::timeout(FUTURE_WAIT, forge_runtime::run(config)).await })
        .expect("Forge runtime future should be bounded")
        .expect_err("test scenario should produce a typed failure")
}

fn spawn_runtime(config: ForgeLaunchConfig) -> JoinHandle<Result<(), ForgeRuntimeError>> {
    thread::Builder::new()
        .name("forge-runtime-test".to_owned())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("worker runtime should build");
            runtime
                .block_on(async {
                    tokio::time::timeout(FUTURE_WAIT, forge_runtime::run(config)).await
                })
                .expect("Forge runtime future should be bounded")
        })
        .expect("Forge runtime worker should spawn")
}

fn wait_for_readiness(path: &Path) -> Result<(Vec<u8>, serde_json::Value), &'static str> {
    let deadline = Instant::now() + STARTUP_WAIT;
    loop {
        if let Ok(bytes) = fs::read(path) {
            if bytes.ends_with(b"\n") {
                if let Ok(value) = serde_json::from_slice(&bytes) {
                    return Ok((bytes, value));
                }
            }
        }
        if Instant::now() >= deadline {
            return Err("readiness should appear within the bounded startup wait");
        }
        thread::yield_now();
    }
}

fn wait_for_readiness_or_stop(
    path: &Path,
    cancel: &CancelHandle,
    worker: &mut Option<JoinHandle<Result<(), ForgeRuntimeError>>>,
) -> (Vec<u8>, serde_json::Value) {
    match wait_for_readiness(path) {
        Ok(readiness) => readiness,
        Err(error) => {
            cancel.cancel();
            let worker_result = join_within(
                worker
                    .take()
                    .expect("readiness worker should still be owned"),
                SHUTDOWN_WAIT,
            );
            panic!("{error}; worker result: {worker_result:?}");
        }
    }
}

fn readiness_endpoint(value: &serde_json::Value) -> SocketAddr {
    value["endpoint"]
        .as_str()
        .expect("readiness endpoint should be text")
        .parse()
        .expect("readiness endpoint should be a socket address")
}

fn assert_no_readiness_temporary(directory: &TemporaryDirectory) {
    let prefix = format!(".artisan-forge-ready-{}-", process::id());
    let leftovers = fs::read_dir(&directory.path)
        .expect("Forge test directory should remain readable")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(&prefix))
        })
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    assert!(
        leftovers.is_empty(),
        "Forge readiness temporary files should be removed: {leftovers:?}"
    );
}

fn join_within<T>(handle: JoinHandle<T>, timeout: Duration) -> T {
    let deadline = Instant::now() + timeout;
    while !handle.is_finished() {
        assert!(
            Instant::now() < deadline,
            "Forge worker exceeded bounded shutdown"
        );
        thread::yield_now();
    }
    handle.join().expect("Forge worker should not panic")
}
