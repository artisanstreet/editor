//! Media resolves through an immutable host/thread resource provider.
//! Production file sources require it; local roots are explicit fixtures.
//! A queued load captures its provider and never consults current navigation.

use std::future::Future;
use std::hash::{Hash, Hasher};
#[cfg(test)]
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use gpui::{App, SharedString};
use serde::Deserialize;

/// One file a reply names, as the host fetches it from where it lives.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct MediaRequest {
    /// The host's scope for relative paths: the open thread.
    pub scope: SharedString,
    /// The path or `file://` URI as written. The Forge resolves it on its
    /// platform, relative to [`Self::from_home`]'s folder unless absolute.
    pub path: String,
    /// Whether `path` is relative to the home folder (`~/…`) rather than
    /// the project.
    pub from_home: bool,
}

/// The bytes of one fetched file, or `None` when it cannot be served.
pub type MediaFetch = Pin<Box<dyn Future<Output = Option<Arc<[u8]>>> + Send>>;

/// The host's way to fetch media files from where they live, scoped to the
/// open thread. Owned by the renderer and captured by every queued load.
#[derive(Clone)]
pub struct MediaFetcher {
    scope: SharedString,
    pub(super) owner: Arc<[u64]>,
    revision: u64,
    reference_scope: Option<super::super::actions::ResourceScope>,
    local_files: bool,
    fetch: Arc<dyn Fn(MediaRequest) -> MediaFetch + Send + Sync>,
    pub(super) resources: Arc<super::resource_cache::ResourceCache>,
}

impl std::fmt::Debug for MediaFetcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaFetcher")
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}
impl PartialEq for MediaFetcher {
    fn eq(&self, other: &Self) -> bool {
        self.scope == other.scope && self.resources.id() == other.resources.id()
    }
}
impl Eq for MediaFetcher {}
impl Hash for MediaFetcher {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.scope.hash(state);
        self.resources.id().hash(state);
    }
}

impl MediaFetcher {
    /// Fetches files for `scope` through `fetch`.
    pub fn new(
        scope: impl Into<SharedString>,
        fetch: impl Fn(MediaRequest) -> MediaFetch + Send + Sync + 'static,
    ) -> Self {
        Self {
            scope: scope.into(),
            owner: Arc::from([]),
            revision: 0,
            reference_scope: None,
            local_files: false,
            fetch: Arc::new(fetch),
            resources: Arc::new(super::resource_cache::ResourceCache::default()),
        }
    }

    /// Whether the owning Forge uses this computer's native filesystem.
    #[must_use]
    pub const fn local_files(mut self, local: bool) -> Self {
        self.local_files = local;
        self
    }

    pub(in crate::markdown_renderer) fn owned_by(mut self, owner: &[u64], revision: u64) -> Self {
        self.owner = Arc::from(owner);
        self.revision = revision;
        self
    }

    pub(super) fn asset_owner(&self, part: &SharedString, immutable: bool) -> SharedString {
        if immutable {
            // Inline payload identity already includes the exact bytes. Every
            // compatible reference in this thread can share the decoded result.
            format!("resource-{}", self.resources.id()).into()
        } else if self.owner.is_empty() {
            // Unscoped callers have no semantic snapshot to share.
            format!("{part}-revision-{}", self.revision).into()
        } else {
            // A mutable path/URL is shared only within its canonical Item source
            // revision, never with a later reply that may name rewritten bytes.
            format!(
                "resource-{}-{:?}-revision-{}",
                self.resources.id(),
                self.owner,
                self.revision
            )
            .into()
        }
    }

    pub(in crate::markdown_renderer) fn retain_resources(&self, owners: &[Vec<u64>], cx: &mut App) {
        self.resources.retain_owners(owners, cx);
    }

    pub(in crate::markdown_renderer) fn clear_resources(&self, cx: &mut App) {
        self.resources.clear(cx);
    }

    #[must_use]
    pub fn reference_scope(mut self, scope: super::super::actions::ResourceScope) -> Self {
        self.reference_scope = Some(scope);
        self
    }
    pub(in crate::markdown_renderer) fn resource_scope(
        &self,
    ) -> Option<super::super::actions::ResourceScope> {
        self.reference_scope.clone()
    }

    /// The scope files are fetched for.
    #[must_use]
    pub const fn scope(&self) -> &SharedString {
        &self.scope
    }

    /// Starts fetching `request`.
    #[must_use]
    pub fn fetch(&self, request: MediaRequest) -> MediaFetch {
        (self.fetch)(request)
    }
}

/// Worker-only provider captures the transport without retaining its UI cache.
#[derive(Clone)]
pub(super) struct MediaProvider {
    scope: SharedString,
    owner: u64,
    fetch: Arc<dyn Fn(MediaRequest) -> MediaFetch + Send + Sync>,
}
impl std::fmt::Debug for MediaProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaProvider")
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}
impl PartialEq for MediaProvider {
    fn eq(&self, other: &Self) -> bool {
        self.scope == other.scope && self.owner == other.owner
    }
}
impl Eq for MediaProvider {}
impl Hash for MediaProvider {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.scope.hash(state);
        self.owner.hash(state);
    }
}
impl MediaProvider {
    pub(super) fn fetch(&self, request: MediaRequest) -> MediaFetch {
        (self.fetch)(request)
    }
}
impl MediaFetcher {
    fn provider(&self) -> MediaProvider {
        MediaProvider {
            scope: self.scope.clone(),
            owner: self.resources.id(),
            fetch: Arc::clone(&self.fetch),
        }
    }
}

/// One compatible resource load. Inline bytes share their exact identity;
/// mutable resources retain their semantic source revision. The scoped cache
/// separately tracks every Item using the result and removes GPUI's pending
/// key when the worker publishes or the last Item unloads.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct Owned<T> {
    /// The scope/snapshot identity shared by compatible references. Semantic
    /// Item membership lives in the resource cache, not in this asset key.
    pub(super) owner: SharedString,
    pub(super) source: T,
}

impl<T> Owned<T> {
    /// `source` as the block `owner` loads it.
    pub(super) fn new(owner: &SharedString, source: T) -> Self {
        Self {
            owner: owner.clone(),
            source,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct Payload {
    pub(super) id: u64,
    pub(super) bytes: Arc<[u8]>,
}
impl PartialEq for Payload {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && (Arc::ptr_eq(&self.bytes, &other.bytes) || self.bytes == other.bytes)
    }
}
impl Eq for Payload {}
impl Hash for Payload {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

/// Immutable media source prepared once with its owning rich spec.
#[derive(Clone, Debug, Deserialize)]
#[serde(from = "String")]
pub(super) struct MediaSource {
    raw: SharedString,
    parsed: Parsed,
}
impl From<String> for MediaSource {
    fn from(raw: String) -> Self {
        Self {
            parsed: parse(&raw),
            raw: raw.into(),
        }
    }
}
impl std::ops::Deref for MediaSource {
    type Target = str;
    fn deref(&self) -> &str {
        &self.raw
    }
}
impl std::fmt::Display for MediaSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.raw.fmt(f)
    }
}
impl From<MediaSource> for SharedString {
    fn from(source: MediaSource) -> Self {
        source.raw
    }
}
impl MediaSource {
    pub(super) fn resident_bytes(&self) -> usize {
        self.raw.len()
            + match &self.parsed {
                Parsed::Url(url) => url.capacity(),
                Parsed::File { path, .. } => path.capacity(),
                Parsed::Data(payload) => payload.bytes.len(),
                Parsed::Invalid => 0,
            }
    }
    pub(super) fn is_empty(&self) -> bool {
        self.raw.trim().is_empty()
    }

    pub(super) fn raw(&self) -> SharedString {
        self.raw.clone()
    }
}

/// A `src` as the reply wrote it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Parsed {
    /// An `http`/`https` URL.
    Url(String),
    /// The decoded payload of a `data:` URL.
    Data(Payload),
    /// A file path or untouched `file://` URI for the owning Forge.
    File { path: String, from_home: bool },
    /// Nothing loadable: an empty `src` or malformed inline/HTTP data.
    Invalid,
}

/// Parses `src` without consulting any file system.
pub(super) fn parse(src: &str) -> Parsed {
    let src = src.trim();
    if src.is_empty() {
        return Parsed::Invalid;
    }
    if has_scheme(src, "http://") || has_scheme(src, "https://") {
        return url::Url::parse(src)
            .ok()
            .filter(|url| url.host().is_some())
            .map_or(Parsed::Invalid, |_| Parsed::Url(src.to_owned()));
    }
    if has_scheme(src, "data:") {
        return data_url(src).map_or(Parsed::Invalid, |bytes| {
            Parsed::Data(Payload {
                id: {
                    let mut hasher = std::hash::DefaultHasher::new();
                    bytes.hash(&mut hasher);
                    hasher.finish()
                },
                bytes: bytes.into(),
            })
        });
    }
    if has_scheme(src, "file://") {
        return Parsed::File {
            path: src.to_owned(),
            from_home: false,
        };
    }
    home_relative(src)
}

/// `~`, `~/…`, and `~\…` name the home folder; everything else is a path.
fn home_relative(path: &str) -> Parsed {
    let rest = if path == "~" {
        Some("")
    } else {
        path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\"))
    };
    let (path, from_home) = rest.map_or((path, false), |rest| (rest, true));
    Parsed::File {
        path: path.to_owned(),
        from_home,
    }
}

fn has_scheme(source: &str, prefix: &str) -> bool {
    source
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

#[cfg(test)]
thread_local! { static DATA_DECODES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }

/// Maintained WHATWG data URL decoding, bounded before each output append.
fn data_url(source: &str) -> Option<Vec<u8>> {
    const MAX_ENCODED: usize = 44 * 1024 * 1024;
    const MAX_DECODED: usize = 32 * 1024 * 1024;
    #[cfg(test)]
    DATA_DECODES.with(|count| count.set(count.get() + 1));
    if source.len() > MAX_ENCODED {
        return None;
    }
    let url = data_url::DataUrl::process(source).ok()?;
    let mut body = Vec::new();
    url.decode(|bytes| {
        if body.len().saturating_add(bytes.len()) > MAX_DECODED {
            return Err(());
        }
        body.extend_from_slice(bytes);
        Ok(())
    })
    .ok()?;
    Some(body)
}

/// Where a `src`'s bytes come from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Located {
    Url(String),
    Data(Payload),
    /// An explicit test fixture file read on this machine.
    #[cfg(test)]
    Path(PathBuf),
    /// A file fetched from where the open thread's files live.
    Fetched(MediaRequest, MediaProvider),
    Invalid,
}

/// Locates `src` through its captured provider. A production file source has
/// no Client filesystem fallback. Tests can call `resolve` with an explicit root.
pub(super) fn locate(src: &MediaSource, fetcher: Option<&MediaFetcher>, _cx: &App) -> Located {
    let parsed = src.parsed.clone();
    if let (Parsed::File { path, from_home }, Some(fetcher)) = (&parsed, fetcher) {
        return Located::Fetched(
            MediaRequest {
                scope: fetcher.scope().clone(),
                path: path.clone(),
                from_home: *from_home,
            },
            fetcher.provider(),
        );
    }
    match parsed {
        Parsed::Url(url) => Located::Url(url),
        Parsed::Data(bytes) => Located::Data(bytes),
        Parsed::File { .. } | Parsed::Invalid => Located::Invalid,
    }
}

/// Resolves a parsed `src` on this machine against `root` and `home`.
#[cfg(test)]
pub(super) fn resolve(parsed: Parsed, root: Option<&Path>, home: Option<&Path>) -> Located {
    match parsed {
        Parsed::Url(url) => Located::Url(url),
        Parsed::Data(bytes) => Located::Data(bytes),
        Parsed::Invalid => Located::Invalid,
        Parsed::File {
            path,
            from_home: true,
        } => home.map_or(Located::Invalid, |home| Located::Path(home.join(path))),
        Parsed::File { path, .. } => {
            let path = if has_scheme(&path, "file://") {
                let Some(path) = url::Url::parse(&path)
                    .ok()
                    .and_then(|url| url.to_file_path().ok())
                else {
                    return Located::Invalid;
                };
                path
            } else {
                PathBuf::from(path)
            };
            match root {
                Some(root) if path.is_relative() => Located::Path(root.join(path)),
                _ => Located::Path(path),
            }
        }
    }
}

#[cfg(test)]
mod data_url_tests {
    use super::*;
    #[test]
    fn percent_encoded_binary_and_base64_use_the_same_bytes() {
        assert_eq!(
            data_url("data:application/octet-stream,%89%50%4e%47%ff"),
            Some(vec![0x89, b'P', b'N', b'G', 0xff])
        );
        assert_eq!(
            data_url("DATA:application/octet-stream;base64,iVBOR/8="),
            Some(vec![0x89, b'P', b'N', b'G', 0xff])
        );
        assert_eq!(
            data_url("data:text/plain,a%23b#fragment"),
            Some(b"a#b".to_vec())
        );
        assert_eq!(data_url("data:text/plain;base64,???"), None);
    }
}

#[cfg(test)]
mod prepared_source_tests {
    use super::*;

    #[gpui::test]
    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "gpui::test hands every test its context as &mut"
    )]
    fn repaint_lookup_never_decodes_or_rehashes_prepared_data(cx: &mut gpui::TestAppContext) {
        DATA_DECODES.with(|count| count.set(0));
        let source = MediaSource::from("data:application/octet-stream;base64,aGVsbG8=".to_owned());
        assert_eq!(DATA_DECODES.with(std::cell::Cell::get), 1);
        let Parsed::Data(original) = &source.parsed else {
            panic!("inline payload")
        };
        for _ in 0..1000 {
            let located = cx.update(|cx| locate(&source.clone(), None, cx));
            let Located::Data(payload) = located else {
                panic!("inline payload")
            };
            assert!(Arc::ptr_eq(&original.bytes, &payload.bytes));
            assert_eq!(*original, payload);
            let mut hasher = std::hash::DefaultHasher::new();
            payload.hash(&mut hasher);
            assert_eq!(hasher.finish(), {
                let mut expected = std::hash::DefaultHasher::new();
                original.id.hash(&mut expected);
                expected.finish()
            });
        }
        assert_eq!(DATA_DECODES.with(std::cell::Cell::get), 1);
    }

    #[gpui::test]
    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "gpui::test hands every test its context as &mut"
    )]
    fn unscoped_files_never_resolve_against_client_working_directory(
        cx: &mut gpui::TestAppContext,
    ) {
        for path in [
            "./image.png",
            "/tmp/image.png",
            "file:///tmp/image.png",
            "~/image.png",
        ] {
            let source = MediaSource::from(path.to_owned());
            assert!(cx.update(|cx| matches!(locate(&source, None, cx), Located::Invalid)));
        }
        let provider = MediaFetcher::new("forge-a/thread-a", |_| Box::pin(async { None }));
        let source = MediaSource::from("file:///project/image%20one.png".to_owned());
        let located = cx.update(|cx| locate(&source, Some(&provider), cx));
        let Located::Fetched(request, _) = located else {
            panic!("owning Forge request")
        };
        assert_eq!(request.path, "file:///project/image%20one.png");
        assert_eq!(request.scope.as_ref(), "forge-a/thread-a");
    }
}

#[cfg(test)]
mod sharing_tests {
    use super::*;

    #[test]
    fn immutable_identity_shares_across_items_but_mutable_identity_keeps_source_revision() {
        let provider = MediaFetcher::new("forge/thread", |_| Box::pin(async { None }));
        let first = provider.clone().owned_by(&[42], 1);
        let repeated = provider.clone().owned_by(&[42], 1);
        let changed = provider.clone().owned_by(&[42], 2);
        let other_item = provider.owned_by(&[43], 1);
        let other_scope = MediaFetcher::new("other-forge/thread", |_| Box::pin(async { None }))
            .owned_by(&[42], 1);
        let left: SharedString = "left-block".into();
        let right: SharedString = "right-block".into();
        assert_eq!(
            first.asset_owner(&left, true),
            other_item.asset_owner(&right, true)
        );
        assert_eq!(
            first.asset_owner(&left, false),
            repeated.asset_owner(&right, false)
        );
        assert_ne!(
            first.asset_owner(&left, false),
            changed.asset_owner(&left, false)
        );
        assert_ne!(
            first.asset_owner(&left, false),
            other_item.asset_owner(&left, false)
        );
        assert_ne!(
            first.asset_owner(&left, true),
            other_scope.asset_owner(&left, true)
        );
    }
}
