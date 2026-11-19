use super::{
    loads::Loads,
    resource_cache::{Load, ResourceLoader},
    source::{self, MediaBytes, MediaFetcher, MediaLoadError, MediaSource, Owned, Parsed},
};
use editor_core::media_http;
use std::sync::Arc;

struct Loader;
impl ResourceLoader for Loader {
    type Source = Owned<Parsed>;
    type Output = Result<Arc<[u8]>, MediaLoadError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum AudioLoadError {
    #[error("only local audio files play here")]
    Remote,
    #[error("no Forge is connected to read the file from")]
    NoHost,
    #[error(transparent)]
    Load(#[from] MediaLoadError),
}

type Loaded = (Result<MediaBytes, MediaLoadError>, usize);

#[derive(Clone)]
pub struct AudioAssets {
    loads: Loads<Loader, Loaded>,
}

impl std::fmt::Debug for AudioAssets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioAssets").finish_non_exhaustive()
    }
}
impl Default for AudioAssets {
    fn default() -> Self {
        Self::new()
    }
}
impl AudioAssets {
    pub fn new() -> Self {
        Self {
            loads: Loads::new(),
        }
    }
    pub fn bind(&self, ctx: &aqua::AppCtx<'_>, window: aqua::prelude::AquaWindowId) {
        ctx.bind_mailbox(window, &self.wake_mailbox());
    }
    pub fn wake_mailbox(&self) -> std::cell::Ref<'_, aqua::mailbox::Mailbox<()>> {
        self.loads.wake_mailbox()
    }
    pub fn request(
        &self,
        fetcher: Option<&MediaFetcher>,
        source: &MediaSource,
        part: &str,
        waiter: u64,
        wake: aqua::mailbox::Wake,
    ) -> Option<Result<Arc<[u8]>, AudioLoadError>> {
        self.loads.discard_stale();
        match (source.parsed(), fetcher) {
            (Parsed::Url(_), _) => return Some(Err(AudioLoadError::Remote)),
            (Parsed::Invalid, _) => return Some(Err(MediaLoadError::Invalid.into())),
            (Parsed::File { .. }, None) => return Some(Err(AudioLoadError::NoHost)),
            (Parsed::File { .. } | Parsed::Data(_), _) => {}
        }
        let key = Self::key(fetcher, source, part);
        let cache = self.loads.cache(fetcher);
        let membership = fetcher.map_or_else(|| Arc::<[u64]>::from([]), |f| Arc::clone(f.owner()));
        let source_bytes = match source.parsed() {
            Parsed::Data(payload) => payload.bytes.len(),
            Parsed::Url(_) | Parsed::File { .. } | Parsed::Invalid => source.raw().len(),
        };
        match cache.load::<Loader>(&membership, &key, source_bytes, waiter, wake) {
            Load::Ready(result) => Some(result.map_err(AudioLoadError::from)),
            Load::Pending => None,
            Load::TooLarge => Some(Err(MediaLoadError::TooLarge.into())),
            Load::Start(ticket) => {
                let source = source.clone();
                let transport = fetcher.map(MediaFetcher::provider);
                self.loads.start(ticket, async move {
                    let fetcher = transport.map(|provider| {
                        let scope = provider.scope().to_owned();
                        MediaFetcher::new(scope, move |request| provider.fetch(request))
                    });
                    let bytes =
                        source::load_image_bytes(&source, fetcher.as_ref(), &media_http::client())
                            .await;
                    let retained = bytes
                        .as_ref()
                        .map_or(0, |loaded| unshared_bytes(&source, &loaded.bytes));
                    (bytes, retained)
                });
                None
            }
        }
    }
    fn key(fetcher: Option<&MediaFetcher>, source: &MediaSource, part: &str) -> Owned<Parsed> {
        let immutable = matches!(source.parsed(), Parsed::Data(_));
        let owner = fetcher.map_or_else(|| part.to_owned(), |f| f.asset_owner(part, immutable));
        Owned::new(owner, source.parsed().clone())
    }
    pub(crate) fn forget(&self, fetcher: Option<&MediaFetcher>, source: &MediaSource, part: &str) {
        let key = Self::key(fetcher, source, part);
        self.loads.cache(fetcher).forget::<Loader>(&key);
    }
    pub fn drain(&self) -> bool {
        self.loads
            .drain(|(bytes, retained)| (bytes.map(|loaded| loaded.bytes), retained))
    }
}

fn unshared_bytes(source: &MediaSource, bytes: &Arc<[u8]>) -> usize {
    match source.parsed() {
        Parsed::Data(payload) if Arc::ptr_eq(&payload.bytes, bytes) => 0,
        Parsed::Data(_) | Parsed::Url(_) | Parsed::File { .. } | Parsed::Invalid => bytes.len(),
    }
}

#[cfg(test)]
#[path = "../../../tests/ui/media/audio.rs"]
mod tests;
