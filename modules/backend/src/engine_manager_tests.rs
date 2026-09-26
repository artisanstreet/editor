use std::{collections::HashMap, io::Write, sync::Mutex, time::Duration};

use artisan_domain::{EngineIntegrity, EngineVersionSelection};
use artisan_native_engine::{Distribution, Feed, FeedRequest, HostPlatform, TransportError};
use sha2::{Digest, Sha256};

use super::*;

const CLAUDE: &str = "https://downloads.claude.ai/claude-code-releases";

/// In-memory vendor whose documents can change between update passes.
#[derive(Default)]
struct Vendor {
    documents: Mutex<HashMap<String, Vec<u8>>>,
}

impl Vendor {
    fn publish_claude(&self, latest: &str, releases: &[&str]) {
        let Distribution::Supported(plan) =
            ManagedEngine::Claude.distribution(HostPlatform::current())
        else {
            return;
        };
        let Feed::ClaudeReleases {
            platform_key,
            binary,
        } = plan.feed
        else {
            return;
        };
        let mut documents = self.documents.lock().unwrap();
        documents.insert(format!("{CLAUDE}/latest"), latest.as_bytes().to_vec());
        for version in releases {
            let body = format!("claude {version}").into_bytes();
            let checksum: String =
                Sha256::digest(&body)
                    .iter()
                    .fold(String::new(), |mut hex, byte| {
                        use std::fmt::Write as _;
                        let _ = write!(hex, "{byte:02x}");
                        hex
                    });
            documents.insert(
                format!("{CLAUDE}/{version}/manifest.json"),
                format!(
                    r#"{{"version":"{version}","platforms":{{"{platform_key}":{{"binary":"{binary}","checksum":"{checksum}","size":{}}}}}}}"#,
                    body.len()
                )
                .into_bytes(),
            );
            documents.insert(format!("{CLAUDE}/{version}/{platform_key}/{binary}"), body);
        }
    }
}

impl ReleaseTransport for Vendor {
    fn fetch(&self, request: &FeedRequest) -> Result<Vec<u8>, TransportError> {
        self.documents
            .lock()
            .unwrap()
            .get(&request.url)
            .cloned()
            .ok_or(TransportError::Rejected)
    }

    fn download(
        &self,
        url: &str,
        bound_bytes: u64,
        sink: &mut dyn Write,
    ) -> Result<u64, TransportError> {
        let body = self
            .documents
            .lock()
            .unwrap()
            .get(url)
            .cloned()
            .ok_or(TransportError::Rejected)?;
        if body.len() as u64 > bound_bytes {
            return Err(TransportError::TooLarge);
        }
        sink.write_all(&body).map_err(|_| TransportError::Sink)?;
        Ok(body.len() as u64)
    }
}

fn wait_for(
    manager: &EngineManager,
    what: &str,
    predicate: impl Fn(&EngineInstallStatus) -> bool,
) -> EngineInstallStatus {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = manager
            .snapshot()
            .engine("claude")
            .filter(|status| predicate(status))
        {
            return status.clone();
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}: {:?}",
            manager.snapshot().engine("claude")
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn ready_at(version: &str) -> impl Fn(&EngineInstallStatus) -> bool + '_ {
    move |status| {
        status.phase == EngineInstallPhase::Ready
            && status.active_version.as_deref() == Some(version)
    }
}

#[test]
fn installs_latest_at_start_and_reports_every_engine() {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("forge.db");
    let vendor = Arc::new(Vendor::default());
    vendor.publish_claude("2.1.283", &["2.1.283"]);
    let manager = EngineManager::start_with(&database, Some(vendor), UPDATE_INTERVAL, || {});
    let status = wait_for(&manager, "claude ready", ready_at("2.1.283"));
    assert_eq!(status.latest_version.as_deref(), Some("2.1.283"));
    assert!(!status.update_available());
    let snapshot = manager.snapshot();
    assert_eq!(snapshot.engines().len(), ManagedEngine::ALL.len());
    let claude = snapshot.engine("claude").unwrap();
    assert_eq!(claude.integrity, EngineIntegrity::VendorChecksum);
    assert!(claude.vendor_version_list);
    for engine in ["grok", "cursor"] {
        let status = snapshot.engine(engine).unwrap();
        assert_eq!(status.integrity, EngineIntegrity::TrustOnFirstDownload);
        if status.phase != EngineInstallPhase::Unsupported {
            assert!(!status.vendor_version_list);
        }
    }
}

#[test]
fn trusted_engines_report_when_their_hash_was_recorded() {
    let Distribution::Supported(plan) = ManagedEngine::Grok.distribution(HostPlatform::current())
    else {
        return;
    };
    let Feed::GrokReleases {
        platform_key,
        binary,
    } = plan.feed
    else {
        return;
    };
    let suffix = if std::path::Path::new(binary)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
    {
        ".exe"
    } else {
        ""
    };
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("forge.db");
    let vendor = Arc::new(Vendor::default());
    {
        let mut documents = vendor.documents.lock().unwrap();
        documents.insert("https://x.ai/cli/stable".to_owned(), b"1.0.41".to_vec());
        documents.insert(
            format!("https://x.ai/cli/grok-1.0.41-{platform_key}{suffix}"),
            b"grok 1.0.41".to_vec(),
        );
    }
    let manager = EngineManager::start_with(&database, Some(vendor), UPDATE_INTERVAL, || {});
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        let status = manager.snapshot().engine("grok").cloned().unwrap();
        if status.phase == EngineInstallPhase::Ready {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "grok never became ready: {status:?}"
        );
        thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(status.integrity, EngineIntegrity::TrustOnFirstDownload);
    assert!(status.trusted_since.unwrap().ends_with('Z'));
}

#[test]
fn automatic_updates_apply_only_while_following_latest() {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("forge.db");
    let vendor = Arc::new(Vendor::default());
    vendor.publish_claude("2.1.282", &["2.1.282", "2.1.283"]);
    let manager = EngineManager::start_with(
        &database,
        Some(Arc::clone(&vendor) as Arc<dyn ReleaseTransport>),
        Duration::from_millis(100),
        || {},
    );
    wait_for(&manager, "initial install", ready_at("2.1.282"));

    manager
        .select("claude", &EngineVersionSelection::Version("2.1.282".into()))
        .unwrap();
    wait_for(&manager, "held", |status| {
        status.held_version.as_deref() == Some("2.1.282")
            && status.phase == EngineInstallPhase::Ready
    });
    vendor.publish_claude("2.1.283", &["2.1.282", "2.1.283"]);
    let held = wait_for(&manager, "newer latest observed", |status| {
        status.latest_version.as_deref() == Some("2.1.283")
    });
    assert_eq!(held.active_version.as_deref(), Some("2.1.282"));
    thread::sleep(Duration::from_millis(300));
    assert_eq!(
        manager
            .snapshot()
            .engine("claude")
            .unwrap()
            .active_version
            .as_deref(),
        Some("2.1.282"),
        "a held engine is never updated automatically"
    );

    manager
        .select("claude", &EngineVersionSelection::Latest)
        .unwrap();
    wait_for(&manager, "follows latest again", ready_at("2.1.283"));
    manager.rollback("claude").unwrap();
    let rolled_back = wait_for(&manager, "rollback", ready_at("2.1.282"));
    assert_eq!(rolled_back.held_version.as_deref(), Some("2.1.282"));
    assert_eq!(rolled_back.rollback_version.as_deref(), Some("2.1.283"));
}

#[test]
fn invalid_requests_are_refused_without_queueing() {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("forge.db");
    let vendor = Arc::new(Vendor::default());
    let manager = EngineManager::start_with(&database, Some(vendor), UPDATE_INTERVAL, || {});
    assert_eq!(
        manager
            .select("gemini", &EngineVersionSelection::Latest)
            .unwrap_err(),
        EngineManagerError::UnknownEngine
    );
    assert_eq!(
        manager
            .select("claude", &EngineVersionSelection::Version("2.1.100".into()))
            .unwrap_err(),
        EngineManagerError::InvalidSelection
    );
    let failed = wait_for(&manager, "feed failure reported", |status| {
        status.phase == EngineInstallPhase::Failed
    });
    assert!(failed.reason.unwrap().contains("release feed"));
}

#[tokio::test]
async fn version_listing_marks_the_active_version() {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("forge.db");
    let vendor = Arc::new(Vendor::default());
    vendor.publish_claude("2.1.283", &["2.1.283"]);
    vendor.documents.lock().unwrap().insert(
        "https://registry.npmjs.org/@anthropic-ai%2fclaude-code".to_owned(),
        br#"{"versions":{"2.1.283":{},"2.1.100":{}}}"#.to_vec(),
    );
    let manager = EngineManager::start_with(&database, Some(vendor), UPDATE_INTERVAL, || {});
    wait_for(&manager, "installed", ready_at("2.1.283"));
    let listing = manager.list_versions("claude").await.unwrap();
    assert_eq!(listing.versions()[0].version, "2.1.283");
    assert!(listing.versions()[0].active);
    assert!(listing.versions()[1].below_floor);
}

#[tokio::test]
async fn handler_answers_reads_changes_and_wakes_delivery_on_change() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use artisan_domain::{
        ChangeEngineVersion, EngineVersionChange, ListEngineVersions, Query, ReadEngineInstalls,
        RequestId,
    };
    use artisan_protocol::{ErrorCode, ResponsePayload};

    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("forge.db");
    let vendor = Arc::new(Vendor::default());
    vendor.publish_claude("2.1.283", &["2.1.283"]);
    let wakes = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&wakes);
    let manager = EngineManager::start_with(&database, Some(vendor), UPDATE_INTERVAL, move || {
        counted.fetch_add(1, Ordering::SeqCst);
    });
    wait_for(&manager, "installed", ready_at("2.1.283"));
    assert!(
        wakes.load(Ordering::SeqCst) > 0,
        "changes wake connection delivery"
    );

    let service = crate::account_usage_service::AccountUsageService::with_readers(
        Vec::new(),
        Duration::from_secs(60),
        Duration::from_secs(5),
    )
    .with_engine_manager(manager);
    let request_id = RequestId::parse("engine-request").unwrap();
    let answer = |query: Query| {
        let service = &service;
        let request_id = &request_id;
        async move { crate::engine_install_handler::answer(Some(service), request_id, &query).await }
    };
    let read = answer(Query::ReadEngineInstalls(ReadEngineInstalls))
        .await
        .unwrap();
    let ResponsePayload::EngineInstalls(snapshot) = read.payload else {
        panic!("expected the install snapshot");
    };
    assert_eq!(
        snapshot.engine("claude").unwrap().active_version.as_deref(),
        Some("2.1.283")
    );
    let unknown = answer(Query::ChangeEngineVersion(ChangeEngineVersion {
        engine_id: "gemini".into(),
        change: EngineVersionChange::Rollback,
    }))
    .await
    .unwrap_err();
    assert_eq!(unknown.code, ErrorCode::InvalidInput);
    let failed_listing = answer(Query::ListEngineVersions(ListEngineVersions {
        engine_id: "claude".into(),
    }))
    .await
    .unwrap_err();
    assert_eq!(failed_listing.code, ErrorCode::Internal);
    assert!(failed_listing.retryable);
    let unavailable = crate::engine_install_handler::answer(
        None,
        &request_id,
        &Query::ReadEngineInstalls(ReadEngineInstalls),
    )
    .await
    .unwrap_err();
    assert_eq!(unavailable.code, ErrorCode::UnsupportedFeature);
}
