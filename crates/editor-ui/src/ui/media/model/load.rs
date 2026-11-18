use super::{Format, Model, Up, parse};
use crate::ui::media::{
    loads::Loads,
    resource_cache::{Load, ResourceLoader},
    source::{self, MediaFetcher, MediaLoadError, MediaSource, Owned, Parsed},
};
use aqua_core::gpui_mesh::{GpuiMesh, GpuiMeshVertex};
use editor_core::media_http;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RendererCapability {
    Mesh,
    Software,
}

#[derive(Clone, Debug, PartialEq)]
struct Request {
    source: Parsed,
    scope: Option<String>,
    format: Option<Format>,
    up: Option<Up>,
    renderer: RendererCapability,
}

pub enum Geometry {
    Mesh(Arc<GpuiMesh>),
    Software(super::Mesh),
}

pub struct LoadedModel {
    pub geometry: Geometry,
    pub triangles: usize,
    pub format: Format,
    resident_bytes: usize,
}

impl LoadedModel {
    pub(crate) fn from_model(model: Model, renderer: RendererCapability) -> Self {
        let Model {
            mesh,
            triangles,
            format,
        } = model;
        let (geometry, resident_bytes) = match renderer {
            RendererCapability::Software => {
                let bytes = mesh.positions.capacity() * std::mem::size_of::<glam::Vec3>()
                    + mesh.normals.capacity() * std::mem::size_of::<glam::Vec3>()
                    + mesh.colors.as_ref().map_or(0, |colors| {
                        colors.capacity() * std::mem::size_of::<glam::Vec3>()
                    });
                (Geometry::Software(mesh), bytes)
            }
            RendererCapability::Mesh => {
                let vertices: Vec<_> = mesh
                    .positions
                    .iter()
                    .zip(&mesh.normals)
                    .enumerate()
                    .map(|(index, (position, normal))| {
                        GpuiMeshVertex::new(
                            position.to_array(),
                            normal.to_array(),
                            mesh.colors.as_ref().map(|colors| colors[index].to_array()),
                        )
                    })
                    .collect();
                let bytes = vertices.capacity() * std::mem::size_of::<GpuiMeshVertex>();
                (Geometry::Mesh(Arc::new(GpuiMesh::new(vertices))), bytes)
            }
        };
        Self {
            geometry,
            triangles,
            format,
            resident_bytes,
        }
    }
}

struct Loader;
impl ResourceLoader for Loader {
    type Source = Owned<Request>;
    type Output = Result<Arc<LoadedModel>, String>;
}
type Loaded = Result<Arc<LoadedModel>, String>;

#[derive(Clone)]
pub struct ModelAssets {
    loads: Loads<Loader, Loaded>,
}
impl Default for ModelAssets {
    fn default() -> Self {
        Self::new()
    }
}
impl ModelAssets {
    pub fn new() -> Self {
        Self {
            loads: Loads::new(),
        }
    }
    pub fn bind(&self, ctx: &aqua::AppCtx<'_>, window: aqua::prelude::AquaWindowId) {
        self.loads.bind(ctx, window);
    }
    pub fn request(
        &self,
        fetcher: Option<&MediaFetcher>,
        source: &MediaSource,
        part: &str,
        format: Option<Format>,
        up: Option<Up>,
        renderer: RendererCapability,
        waiter: u64,
        wake: aqua::mailbox::Wake,
    ) -> Option<Result<Arc<LoadedModel>, String>> {
        self.loads.discard_stale();
        match source.parsed() {
            Parsed::Url(_) => return Some(Err("only model files on disk load here".to_owned())),
            Parsed::Invalid => return Some(Err("the path is not valid".to_owned())),
            Parsed::File { .. } if fetcher.is_none() => {
                return Some(Err("the path is not valid".to_owned()));
            }
            _ => {}
        }
        let immutable = matches!(source.parsed(), Parsed::Data(_));
        let owner = fetcher.map_or_else(|| part.to_owned(), |f| f.asset_owner(part, immutable));
        let key = Owned::new(
            owner,
            Request {
                source: source.parsed().clone(),
                scope: match (source.parsed(), fetcher) {
                    (Parsed::File { .. }, Some(fetcher)) => Some(fetcher.scope().to_owned()),
                    _ => None,
                },
                format,
                up,
                renderer,
            },
        );
        let cache = self.loads.cache(fetcher);
        let membership = fetcher.map_or_else(|| Arc::<[u64]>::from([]), |f| Arc::clone(f.owner()));
        let source_bytes = match source.parsed() {
            Parsed::Data(data) => data.bytes.len(),
            _ => source.raw().len(),
        } + key.owner.len();
        match cache.load::<Loader>(&membership, &key, source_bytes, waiter, wake) {
            Load::Ready(result) => Some(result),
            Load::Pending => None,
            Load::TooLarge => Some(Err(MediaLoadError::TooLarge.to_string())),
            Load::Start(ticket) => {
                let source = source.clone();
                let transport = fetcher.map(MediaFetcher::provider);
                self.loads.start(ticket, async move {
                    let fetcher = transport.map(|provider| {
                        let scope = provider.scope().to_owned();
                        MediaFetcher::new(scope, move |request| provider.fetch(request))
                    });
                    let loaded =
                        source::load_image_bytes(&source, fetcher.as_ref(), &media_http::client())
                            .await;
                    match loaded {
                        Err(error) => Err(error.to_string()),
                        Ok(bytes) => tokio::task::spawn_blocking(move || {
                            let _permit = bytes.permit;
                            parse(&bytes.bytes, format, up)
                                .map(|model| Arc::new(LoadedModel::from_model(model, renderer)))
                        })
                        .await
                        .unwrap_or_else(|_| Err("the file could not be read".to_owned())),
                    }
                });
                None
            }
        }
    }
    pub fn drain(&self) -> bool {
        self.loads.drain(|result| {
            let size = result.as_ref().map_or(0, |model| model.resident_bytes);
            (result, size)
        })
    }
}
#[cfg(test)]
#[path = "../../../../tests/ui/media/model/load.rs"]
mod tests;
