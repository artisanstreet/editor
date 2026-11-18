use super::*;
use futures::StreamExt as _;

#[test]
fn updates_keep_nightly_and_custom_feeds_but_release_pins_follow_stable() {
    for (from, expected) in [
        (
            "https://github.com/artisanstreet/editor/releases/download/nightly/release.json",
            "https://github.com/artisanstreet/editor/releases/download/nightly/release.json",
        ),
        (
            "https://example.com/releases/release.json",
            "https://example.com/releases/release.json",
        ),
        (
            "https://github.com/artisanstreet/editor/releases/download/v1.2.3/release.json",
            DEFAULT_FEED,
        ),
    ] {
        let source = Source::new(Some(from), None, false).expect("feed source");
        assert_eq!(source.update_feed(), Some(expected));
    }
    let source = Source::new(Some("."), None, false).expect("local build");
    assert_eq!(source.update_feed(), None);
}

#[test]
fn nightly_timestamps_order_updates_even_when_the_base_version_is_unchanged() {
    assert!(
        newer(
            "0.0.0-nightly.20261111031700+abcdef",
            "0.0.0-nightly.20261110031700+123456"
        ),
        "a later nightly updates an earlier nightly"
    );
    assert!(
        !newer(
            "0.0.0-nightly.20261110031700+123456",
            "0.0.0-nightly.20261111031700+abcdef"
        ),
        "an earlier nightly does not replace a later nightly"
    );
}

#[test]
fn a_nightly_build_updates_from_the_nightly_feed_and_a_release_from_the_latest() {
    assert_eq!(
        feed_for("0.0.0-nightly.20261111220000+abcdef123456"),
        NIGHTLY_FEED
    );
    assert_eq!(feed_for("1.4.0"), DEFAULT_FEED);
    assert_eq!(feed_for("0.5.0-dev+abc1234"), DEFAULT_FEED);
}

#[test]
fn the_file_that_installs_a_platform_is_the_editor_on_windows_and_ae_elsewhere() {
    assert_eq!(carrier("windows"), App::Editor);
    assert_eq!(carrier("linux"), App::Ae);
    assert_eq!(carrier("macos"), App::Ae);
}

#[test]
fn a_version_pin_installs_from_its_immutable_tag_and_updates_from_its_channel() {
    for (version, manifest, channel) in [
        (
            "1.2.3",
            "https://github.com/artisanstreet/editor/releases/download/v1.2.3/release.json",
            DEFAULT_FEED,
        ),
        (
            "0.0.0-nightly.20261111220000+abcdef123456",
            "https://github.com/artisanstreet/editor/releases/download/v0.0.0-nightly.20261111220000/release.json",
            NIGHTLY_FEED,
        ),
    ] {
        let source = Source::new(None, Some(version), false).expect("pinned source");
        assert_eq!(source.argument(), manifest);
        assert_eq!(source.update_feed(), Some(channel));
    }
    assert!(Source::new(None, Some("../latest"), false).is_err());
}

#[test]
fn a_pinned_version_accepts_only_that_release() {
    let stable = semver::Version::parse("1.2.3").expect("version");
    assert!(pins(&stable, "1.2.3"));
    assert!(!pins(&stable, "1.2.4"));
    assert!(!pins(&stable, "1.2.3-nightly.1"));
    let nightly = semver::Version::parse("0.0.0-nightly.20261111220000").expect("version");
    assert!(pins(&nightly, "0.0.0-nightly.20261111220000+abcdef123456"));
    assert!(!pins(&nightly, "0.0.0-nightly.20261112220000+abcdef123456"));
    let exact =
        semver::Version::parse("0.0.0-nightly.20261111220000+abcdef123456").expect("version");
    assert!(pins(&exact, "0.0.0-nightly.20261111220000+abcdef123456"));
    assert!(!pins(&exact, "0.0.0-nightly.20261111220000+000000000000"));
}

#[test]
fn channel_feeds_read_the_signature_and_files_from_the_release_they_point_at() {
    for (channel, version, expected) in [
        (
            NIGHTLY_FEED,
            "0.0.0-nightly.20261111220000+abcdef123456",
            "https://github.com/artisanstreet/editor/releases/download/v0.0.0-nightly.20261111220000/ae-linux-x86_64",
        ),
        (
            DEFAULT_FEED,
            "1.2.3",
            "https://github.com/artisanstreet/editor/releases/download/v1.2.3/ae-linux-x86_64",
        ),
        (
            "https://example.com/releases/release.json",
            "1.2.3",
            "https://example.com/releases/ae-linux-x86_64",
        ),
    ] {
        let assets = Source::Url(channel.to_owned())
            .immutable(version)
            .expect("asset source");
        assert_eq!(assets.file("ae-linux-x86_64"), expected);
    }
    assert!(
        Source::Url(NIGHTLY_FEED.to_owned())
            .immutable("../../evil")
            .is_err()
    );
}

struct SignedRelease {
    folder: tempfile::TempDir,
    key: ed25519_dalek::SigningKey,
}

impl SignedRelease {
    fn new() -> Self {
        let release = Self {
            folder: tempfile::tempdir().expect("release folder"),
            key: ed25519_dalek::SigningKey::from_bytes(&[7; 32]),
        };
        release.write(
            br#"{"version":"1.2.3","commit":"a","artifacts":[]}"#,
            &release.key,
        );
        release
    }

    fn write(&self, manifest: &[u8], key: &ed25519_dalek::SigningKey) {
        use ed25519_dalek::Signer as _;
        let signature =
            base64::engine::general_purpose::STANDARD.encode(key.sign(manifest).to_bytes());
        std::fs::write(self.folder.path().join(MANIFEST), manifest).expect("write manifest");
        std::fs::write(self.folder.path().join("release.json.sig"), signature)
            .expect("write signature");
    }

    fn resolve(&self) -> Result<String> {
        let source = Source::Dir(self.folder.path().to_owned());
        let runtime = tokio::runtime::Runtime::new().expect("runtime");
        match runtime.block_on(resolve_signed(
            &source,
            Carried::default(),
            &self.key.verifying_key(),
            LIMITS,
        ))? {
            Release::Feed(feed) => Ok(feed.version().to_owned()),
            Release::Local { .. } | Release::Bundled(_) => bail!("not a signed release"),
        }
    }
}

#[test]
fn a_signed_release_resolves_to_its_manifest() {
    assert_eq!(
        SignedRelease::new().resolve().expect("signed release"),
        "1.2.3"
    );
}

#[test]
fn a_manifest_changed_after_signing_is_rejected() {
    let release = SignedRelease::new();
    std::fs::write(
        release.folder.path().join(MANIFEST),
        br#"{"version":"1.2.4","commit":"a","artifacts":[]}"#,
    )
    .expect("tamper");
    let error = release.resolve().expect_err("tampered manifest");
    assert!(format!("{error:#}").contains("not signed by Artisan's release key"));
}

#[test]
fn altered_or_foreign_signatures_are_rejected() {
    let release = SignedRelease::new();
    let path = release.folder.path().join("release.json.sig");
    let mut signature = base64::engine::general_purpose::STANDARD
        .decode(std::fs::read(&path).expect("signature"))
        .expect("base64");
    signature[0] ^= 1;
    std::fs::write(
        &path,
        base64::engine::general_purpose::STANDARD.encode(&signature),
    )
    .expect("alter signature");
    assert!(release.resolve().is_err());
    release.write(
        br#"{"version":"1.2.3","commit":"a","artifacts":[]}"#,
        &ed25519_dalek::SigningKey::from_bytes(&[8; 32]),
    );
    let error = release.resolve().expect_err("foreign signature");
    assert!(format!("{error:#}").contains("not signed by Artisan's release key"));
}

fn limits(deadline: Duration, bytes: u64) -> Limits {
    Limits {
        connect: Duration::from_secs(5),
        idle: Duration::from_secs(30),
        metadata: deadline,
        metadata_bytes: bytes,
        artifact: deadline,
        artifact_bytes: bytes,
    }
}

async fn feed_serving(
    body: impl Fn() -> axum::body::Body + Clone + Send + Sync + 'static,
) -> test_support::http::StubServer {
    test_support::http::StubServer::serve(axum::Router::new().route(
        "/release.json",
        axum::routing::get(move || {
            let body = body.clone();
            async move { body() }
        }),
    ))
    .await
}

fn stalled() -> axum::body::Body {
    axum::body::Body::from_stream(
        futures::stream::iter([Ok::<_, std::io::Error>(vec![b'{'])])
            .chain(futures::stream::pending()),
    )
}

fn endless() -> axum::body::Body {
    axum::body::Body::from_stream(futures::stream::repeat_with(|| {
        Ok::<_, std::io::Error>(vec![b' '; 1024])
    }))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_feed_that_stops_sending_gives_up_at_its_deadline() {
    let server = feed_serving(stalled).await;
    let limits = limits(Duration::from_millis(300), 1024 * 1024);
    let http = http_client(&limits).unwrap();

    let fetched = test_support::wait::within(
        "the stalled download to give up",
        Duration::from_secs(10),
        fetch_metadata(&http, &format!("{}/release.json", server.url()), &limits),
    )
    .await;

    assert!(
        matches!(fetched, Err(DownloadError::TimedOut { .. })),
        "{fetched:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_endless_manifest_is_cut_off_at_its_size_limit() {
    let server = feed_serving(endless).await;
    let limits = limits(Duration::from_secs(30), 64 * 1024);
    let http = http_client(&limits).unwrap();

    let fetched = test_support::wait::within(
        "the endless download to be cut off",
        Duration::from_secs(10),
        fetch_metadata(&http, &format!("{}/release.json", server.url()), &limits),
    )
    .await;

    assert!(
        matches!(fetched, Err(DownloadError::TooLarge { .. })),
        "{fetched:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_oversized_artifact_leaves_no_file_behind() {
    let server = feed_serving(|| axum::body::Body::from(vec![b'x'; 8 * 1024])).await;
    let limits = limits(Duration::from_secs(30), 4 * 1024);
    let http = http_client(&limits).unwrap();
    let folder = tempfile::tempdir().unwrap();

    let fetched = fetch_artifact(
        &http,
        &format!("{}/release.json", server.url()),
        &limits,
        folder.path(),
        None,
    )
    .await;

    assert!(matches!(fetched, Err(DownloadError::TooLarge { .. })));
    assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 0);
}

#[test]
fn an_unreadable_local_version_is_reported_instead_of_invented() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("linux/VERSION")).unwrap();
    let release = Release::Local {
        dir: dir.path().to_owned(),
    };

    assert!(release.version("linux").is_err());
    assert_eq!(
        Release::Local {
            dir: dir.path().join("elsewhere"),
        }
        .version("linux")
        .unwrap(),
        "0.0.0-local"
    );
}
