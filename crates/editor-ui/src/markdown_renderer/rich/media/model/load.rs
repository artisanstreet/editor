//! Loading a model `src`: bytes through [`super::super::source`], parsed off
//! the main thread through GPUI's asset cache, so every frame after the
//! first reuses the parsed mesh. Each block keeps the mesh it first loaded
//! ([`Owned`]); a later block naming a rewritten file loads it afresh.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{App, Asset, SharedString, Window};

use super::super::source::{Located, MediaFetcher, MediaRequest, Owned, locate};
use super::formats::{self, Format, Model};
use super::mesh::Up;

/// The largest model file read from this machine's disk; the Forge serves
/// files up to the same size.
const MAX_FILE_BYTES: u64 = 12 * 1024 * 1024;

/// Where a model's bytes come from.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum Bytes {
    Data(Arc<[u8]>),
    Fetched(MediaRequest),
    Path(PathBuf),
}

/// One model to load, with the spec's overrides.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(super) struct ModelRequest {
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
    src: &str,
    format: Option<Format>,
    up: Option<Up>,
) -> Loaded {
    let bytes = match locate(src, cx) {
        Located::Data(bytes) => Bytes::Data(bytes.into()),
        Located::Fetched(request) => Bytes::Fetched(request),
        Located::Path(path) => Bytes::Path(path),
        Located::Url(_) => return Loaded::Failed("only model files on disk load here".into()),
        Located::Invalid => return Loaded::Failed("the path is not valid".into()),
    };
    let request = Owned::new(owner, ModelRequest { bytes, format, up });
    match window.use_asset::<ModelLoader>(&request, cx) {
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
        cx: &mut App,
    ) -> impl std::future::Future<Output = Self::Output> + Send + 'static {
        let fetch = match &source.bytes {
            Bytes::Fetched(request) => cx
                .try_global::<MediaFetcher>()
                .map(|fetcher| fetcher.fetch(request.clone())),
            Bytes::Data(_) | Bytes::Path(_) => None,
        };
        async move {
            let bytes: Arc<[u8]> = match source.bytes {
                Bytes::Data(bytes) => bytes,
                Bytes::Fetched(_) => match fetch {
                    Some(fetch) => fetch.await.ok_or("the file could not be read")?,
                    None => return Err("the file could not be read".into()),
                },
                Bytes::Path(path) => read(&path)?.into(),
            };
            formats::parse(&bytes, source.format, source.up)
                .map(Arc::new)
                .map_err(SharedString::from)
        }
    }
}

fn read(path: &std::path::Path) -> Result<Vec<u8>, SharedString> {
    let metadata = std::fs::metadata(path).map_err(|_| "the file does not exist")?;
    if metadata.len() > MAX_FILE_BYTES {
        return Err("the file is larger than 12 MiB".into());
    }
    std::fs::read(path).map_err(|_| "the file could not be read".into())
}

#[cfg(test)]
mod tests {
    use super::{Bytes, ModelLoader, ModelRequest, Owned};

    const ONE: &str = "v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
    const TWO: &str = "v 0 0 0\nv 1 0 0\nv 0 1 0\nv 1 1 0\nf 1 2 3\nf 2 4 3\n";

    #[gpui::test]
    async fn a_block_keeps_its_model_and_a_later_block_reads_the_rewrite(
        cx: &mut gpui::TestAppContext,
    ) {
        let dir = std::env::temp_dir().join(format!("artisan-model-load-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("figure.obj");
        let request = |owner: &str| Owned {
            owner: owner.to_owned().into(),
            source: ModelRequest {
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
