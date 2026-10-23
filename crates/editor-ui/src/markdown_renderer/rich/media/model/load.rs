//! Loading a model `src`: bytes through [`super::super::source`], parsed off
//! the main thread through GPUI's asset cache, so every frame after the
//! first reuses the parsed mesh. Compatible inline-byte references share one
//! result; mutable file references keep their semantic source revision so a
//! later reply naming a rewritten file loads it afresh.

#[cfg(test)]
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{App, Asset, SharedString, Window};

use super::super::source::{
    Located, MediaFetcher, MediaProvider, MediaRequest, MediaSource, Owned, Payload, locate,
};
use super::formats::{self, Format, Model};
use super::mesh::Up;

/// The largest model file read from this machine's disk; the Forge serves
/// files up to the same size.
#[cfg(test)]
const MAX_FILE_BYTES: u64 = 12 * 1024 * 1024;

/// Where a model's bytes come from.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum Bytes {
    Data(Payload),
    Fetched(MediaRequest, MediaProvider),
    #[cfg(test)]
    Path(PathBuf),
}

/// One model to load, with the spec's overrides.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct ModelRequest {
    gpu_enabled: bool,
    bytes: Bytes,
    format: Option<Format>,
    up: Option<Up>,
}

/// Where a model stands this frame.
pub(super) enum Loaded {
    Loading,
    Ready(Arc<Model>),
    Failed(SharedString),
}

/// Loads `src` for the block `owner` through the asset cache; the view
/// redraws when it lands.
pub(super) fn load(
    window: &mut Window,
    cx: &mut App,
    owner: &SharedString,
    src: &MediaSource,
    format: Option<Format>,
    up: Option<Up>,
    fetcher: Option<&MediaFetcher>,
) -> Loaded {
    let bytes = match locate(src, fetcher, cx) {
        Located::Data(bytes) => Bytes::Data(bytes),
        Located::Fetched(request, fetcher) => Bytes::Fetched(request, fetcher),
        #[cfg(test)]
        Located::Path(path) => Bytes::Path(path),
        Located::Url(_) => return Loaded::Failed("only model files on disk load here".into()),
        Located::Invalid => return Loaded::Failed("the path is not valid".into()),
    };
    let immutable = matches!(&bytes, Bytes::Data(_));
    let owner = fetcher.map_or_else(
        || owner.clone(),
        |fetcher| fetcher.asset_owner(owner, immutable),
    );
    let request = Owned::new(
        &owner,
        ModelRequest {
            bytes,
            format,
            up,
            gpu_enabled: window.supports_meshes(),
        },
    );
    let source_bytes = match &request.source.bytes {
        Bytes::Data(payload) => payload.bytes.len(),
        Bytes::Fetched(request, _) => request.path.len(),
        #[cfg(test)]
        Bytes::Path(path) => path.as_os_str().len(),
    } + owner.len();
    let output = match fetcher {
        Some(fetcher) => fetcher.resources.load::<ModelLoader>(
            &fetcher.owner,
            &request,
            window,
            cx,
            source_bytes,
            |output| output.as_ref().map_or(0, |model| model.resident_bytes()),
            |_, _| {},
        ),
        None => window.use_asset::<ModelLoader>(&request, cx),
    };
    match output {
        None => Loaded::Loading,
        Some(Ok(model)) => Loaded::Ready(model),
        Some(Err(reason)) => Loaded::Failed(reason),
    }
}

/// Reads and parses a [`ModelRequest`] on the background executor.
enum ModelLoader {}

impl Asset for ModelLoader {
    type Source = Owned<ModelRequest>;
    type Output = Result<Arc<Model>, SharedString>;

    fn load(
        Owned { source, .. }: Self::Source,
        _cx: &mut App,
    ) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
        let fetch = match &source.bytes {
            Bytes::Fetched(request, fetcher) => Some(fetcher.fetch(request.clone())),
            Bytes::Data(_) => None,
            #[cfg(test)]
            Bytes::Path(_) => None,
        };
        async move {
            let _capacity = super::super::work_permit().await;
            let bytes: Arc<[u8]> = match source.bytes {
                Bytes::Data(payload) => payload.bytes,
                Bytes::Fetched(_, _) => match fetch {
                    Some(fetch) => fetch.await.ok_or("the file could not be read")?,
                    None => return Err("the file could not be read".into()),
                },
                #[cfg(test)]
                Bytes::Path(path) => read(&path)?.into(),
            };
            formats::parse_for_renderer(&bytes, source.format, source.up, source.gpu_enabled)
                .map(Arc::new)
                .map_err(SharedString::from)
        }
    }
}

#[cfg(test)]
fn read(path: &std::path::Path) -> Result<Vec<u8>, SharedString> {
    use std::io::Read as _;
    let metadata = std::fs::metadata(path).map_err(|_missing| "the file does not exist")?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        return Err("the model must be a regular file no larger than 12 MiB".into());
    }
    let file = std::fs::File::open(path).map_err(|_unreadable| "the file could not be read")?;
    let mut bytes = Vec::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_unreadable| "the file could not be read")?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err("the file is larger than 12 MiB".into());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::{Bytes, ModelLoader, ModelRequest, Owned};

    const ONE: &str = "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
    const TWO: &str = "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 1 0\nf 1 2 3\nf 2 4 3\n";

    #[test]
    fn model_fixture_reads_are_bounded_and_require_regular_files() {
        let dir = tempfile::tempdir().unwrap();
        assert!(super::read(dir.path()).is_err());
        let path = dir.path().join("oversized.obj");
        std::fs::File::create(&path)
            .unwrap()
            .set_len(super::MAX_FILE_BYTES + 1)
            .unwrap();
        assert!(super::read(&path).is_err());
        std::fs::write(&path, ONE).unwrap();
        assert_eq!(super::read(&path).unwrap(), ONE.as_bytes());
    }

    #[gpui::test]
    #[expect(
        clippy::future_not_send,
        reason = "gpui::test runs the future on its own thread"
    )]
    async fn a_block_keeps_its_model_and_a_later_block_reads_the_rewrite(
        cx: &mut gpui::TestAppContext,
    ) {
        let dir = std::env::temp_dir().join(format!("artisan-model-load-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("figure.obj");
        let request = |owner: &str| Owned {
            owner: owner.to_owned().into(),
            source: ModelRequest {
                gpu_enabled: false,
                bytes: Bytes::Path(path.clone()),
                format: None,
                up: None,
            },
        };
        let load = |owner: &str, cx: &mut gpui::TestAppContext| {
            cx.update(|cx| cx.fetch_asset::<ModelLoader>(&request(owner)).0)
        };

        std::fs::write(&path, ONE).unwrap();
        let first = load("first", cx).await.unwrap();
        std::fs::write(&path, TWO).unwrap();
        let again = load("first", cx).await.unwrap();
        let second = load("second", cx).await.unwrap();
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(first.mesh.triangles(), 1);
        assert_eq!(again.mesh.triangles(), 1);
        assert_eq!(second.mesh.triangles(), 2);
    }
}
