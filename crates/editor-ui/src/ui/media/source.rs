use std::future::Future;
use std::hash::{Hash, Hasher};
use std::pin::Pin;
use std::sync::{Arc, LazyLock};

use super::resource_cache::ResourceCache;
use serde::Deserialize;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct MediaRequest {
    pub scope: String,
    pub path: String,
    pub from_home: bool,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MediaFailure {
    Missing,
    Unreachable,
    Unreadable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaLoadError {
    Invalid,
    Fetch(MediaFailure),
}

pub type MediaFetch = Pin<Box<dyn Future<Output = Result<Arc<[u8]>, MediaFailure>> + Send>>;

static MEDIA_WORK: LazyLock<Arc<tokio::sync::Semaphore>> =
    LazyLock::new(|| Arc::new(tokio::sync::Semaphore::new(4)));

pub struct MediaBytes {
    pub bytes: Arc<[u8]>,
    pub permit: tokio::sync::OwnedSemaphorePermit,
}

impl MediaBytes {
    pub fn decode(
        self,
        images: &aqua::ImageAssets,
    ) -> Result<aqua::ImageHandle, aqua::ImageAssetError> {
        images.decode_preview_with_guard(self.bytes, self.permit)
    }
}

#[derive(Clone)]
pub struct MediaFetcher {
    scope: String,
    owner: Arc<[u64]>,
    revision: u64,
    fetch: Arc<dyn Fn(MediaRequest) -> MediaFetch + Send + Sync>,
    resources: Arc<ResourceCache>,
}

impl MediaFetcher {
    pub fn new(
        scope: impl Into<String>,
        fetch: impl Fn(MediaRequest) -> MediaFetch + Send + Sync + 'static,
    ) -> Self {
        Self {
            scope: scope.into(),
            owner: Arc::from([]),
            revision: 0,
            fetch: Arc::new(fetch),
            resources: ResourceCache::new(),
        }
    }

    pub fn fetch(&self, request: MediaRequest) -> MediaFetch {
        (self.fetch)(request)
    }

    pub fn owned_by(mut self, owner: &[u64], revision: u64) -> Self {
        self.owner = Arc::from(owner);
        self.revision = revision;
        self
    }

    pub fn owner(&self) -> &Arc<[u64]> {
        &self.owner
    }

    pub fn scope(&self) -> &str {
        &self.scope
    }

    pub fn resources(&self) -> &Arc<ResourceCache> {
        &self.resources
    }

    pub fn asset_owner(&self, part: &str, immutable: bool) -> String {
        if immutable {
            format!("resource-{}", self.resources.id())
        } else if self.owner.is_empty() {
            format!("{part}-revision-{}", self.revision)
        } else {
            format!(
                "resource-{}-{:?}-revision-{}",
                self.resources.id(),
                self.owner,
                self.revision
            )
        }
    }

    pub fn retain_resources(&self, owners: &[Vec<u64>]) {
        self.resources.retain_owners(owners);
    }

    pub fn clear_resources(&self) {
        self.resources.clear();
    }

    pub fn provider(&self) -> MediaProvider {
        MediaProvider {
            scope: self.scope.clone(),
            owner: self.resources.id(),
            fetch: Arc::clone(&self.fetch),
        }
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

#[derive(Clone)]
pub struct MediaProvider {
    scope: String,
    owner: u64,
    fetch: Arc<dyn Fn(MediaRequest) -> MediaFetch + Send + Sync>,
}

impl MediaProvider {
    pub fn scope(&self) -> &str {
        &self.scope
    }
    pub fn fetch(&self, request: MediaRequest) -> MediaFetch {
        (self.fetch)(request)
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

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Owned<T> {
    pub owner: String,
    pub source: T,
}

impl<T> Owned<T> {
    pub fn new(owner: impl Into<String>, source: T) -> Self {
        Self {
            owner: owner.into(),
            source,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Payload {
    id: u64,
    pub bytes: Arc<[u8]>,
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

#[derive(Clone, Debug, Deserialize)]
#[serde(from = "String")]
pub struct MediaSource {
    pub raw: String,
    pub parsed: Parsed,
}

impl From<String> for MediaSource {
    fn from(raw: String) -> Self {
        Self {
            parsed: parse(&raw),
            raw,
        }
    }
}

impl std::ops::Deref for MediaSource {
    type Target = str;

    fn deref(&self) -> &str {
        &self.raw
    }
}

impl MediaSource {
    pub(crate) fn resident_bytes(&self) -> usize {
        self.raw.capacity()
            + match &self.parsed {
                Parsed::Url(url) => url.capacity(),
                Parsed::Data(payload) => payload.bytes.len(),
                Parsed::File { path, .. } => path.capacity(),
                Parsed::Invalid => 0,
            }
    }
    pub fn is_empty(&self) -> bool {
        self.raw.trim().is_empty()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Parsed {
    Url(String),
    Data(Payload),
    File { path: String, from_home: bool },
    Invalid,
}

pub fn parse(src: &str) -> Parsed {
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
            let mut hasher = std::hash::DefaultHasher::new();
            bytes.hash(&mut hasher);
            Parsed::Data(Payload {
                id: hasher.finish(),
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
    let rest = if src == "~" {
        Some("")
    } else {
        src.strip_prefix("~/").or_else(|| src.strip_prefix("~\\"))
    };
    let (path, from_home) = rest.map_or((src, false), |rest| (rest, true));
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

fn data_url(source: &str) -> Option<Vec<u8>> {
    const MAX_ENCODED: usize = 44 * 1024 * 1024;
    const MAX_DECODED: usize = 32 * 1024 * 1024;
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

pub async fn load_image_bytes(
    source: &MediaSource,
    fetcher: Option<&MediaFetcher>,
    http: &editor_core::media_http::MediaHttpClient,
) -> Result<MediaBytes, MediaLoadError> {
    let permit = MEDIA_WORK
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| MediaLoadError::Invalid)?;
    let bytes = match &source.parsed {
        Parsed::Url(url) => {
            let response = http
                .get(url, true)
                .await
                .map_err(|_| MediaLoadError::Fetch(MediaFailure::Unreachable))?;
            match response.status.as_u16() {
                200..=299 => Ok(response.body.into()),
                404 | 410 => Err(MediaLoadError::Fetch(MediaFailure::Missing)),
                _ => Err(MediaLoadError::Fetch(MediaFailure::Unreadable)),
            }
        }
        Parsed::Data(payload) => Ok(payload.bytes.clone()),
        Parsed::File { path, from_home } => {
            let fetcher = fetcher.ok_or(MediaLoadError::Invalid)?;
            fetcher
                .fetch(MediaRequest {
                    scope: fetcher.scope.clone(),
                    path: path.clone(),
                    from_home: *from_home,
                })
                .await
                .map_err(MediaLoadError::Fetch)
        }
        Parsed::Invalid => Err(MediaLoadError::Invalid),
    }?;
    Ok(MediaBytes { bytes, permit })
}
