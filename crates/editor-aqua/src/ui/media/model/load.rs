use super::{Format, Model, Up, parse};
use crate::core::{delivery::Delivery, media_http, runtime};
use crate::ui::media::{
    resource_cache::{Load, LoadTicket, ResourceCache, ResourceLoader},
    source::{self, MediaFailure, MediaFetcher, MediaLoadError, MediaSource, Owned, Parsed},
};
use aqua_core::gpui_mesh::{GpuiMesh, GpuiMeshVertex};
use std::{cell::RefCell, collections::HashMap, num::NonZeroUsize, rc::Rc, sync::Arc};
use tokio::task::AbortHandle;

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
    fn from_model(model: Model, renderer: RendererCapability) -> Self {
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
struct Completion {
    id: u64,
    result: Result<Arc<LoadedModel>, String>,
}
struct Job {
    ticket: LoadTicket<Loader>,
    abort: AbortHandle,
}
struct State {
    next: u64,
    jobs: HashMap<u64, Job>,
    delivery: Delivery<Completion>,
}
impl Drop for State {
    fn drop(&mut self) {
        for job in self.jobs.values() {
            job.ticket.cancel();
            job.abort.abort();
        }
    }
}

#[derive(Clone)]
pub struct ModelAssets {
    state: Rc<RefCell<State>>,
    mailbox: Rc<RefCell<aqua::mailbox::Mailbox<()>>>,
    fallback: Arc<ResourceCache>,
}
impl Default for ModelAssets {
    fn default() -> Self {
        Self::new()
    }
}
impl ModelAssets {
    pub fn new() -> Self {
        let mailbox = Rc::new(RefCell::new(aqua::mailbox::Mailbox::new(NonZeroUsize::MIN)));
        let sender = mailbox.borrow().sender();
        Self {
            state: Rc::new(RefCell::new(State {
                next: 0,
                jobs: HashMap::new(),
                delivery: Delivery::with_wake(NonZeroUsize::new(16).unwrap(), sender),
            })),
            mailbox,
            fallback: ResourceCache::new(),
        }
    }
    pub fn bind(&self, ctx: &aqua::AppCtx<'_>, window: aqua::prelude::AquaWindowId) {
        ctx.bind_mailbox(window, &self.mailbox.borrow());
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
        self.discard_stale();
        match &source.parsed {
            Parsed::Url(_) => return Some(Err("only model files on disk load here".to_owned())),
            Parsed::Invalid => return Some(Err("the path is not valid".to_owned())),
            Parsed::File { .. } if fetcher.is_none() => {
                return Some(Err("the path is not valid".to_owned()));
            }
            _ => {}
        }
        let immutable = matches!(source.parsed, Parsed::Data(_));
        let owner = fetcher.map_or_else(|| part.to_owned(), |f| f.asset_owner(part, immutable));
        let key = Owned::new(
            owner,
            Request {
                source: source.parsed.clone(),
                scope: match (&source.parsed, fetcher) {
                    (Parsed::File { .. }, Some(fetcher)) => Some(fetcher.scope().to_owned()),
                    _ => None,
                },
                format,
                up,
                renderer,
            },
        );
        let cache = fetcher.map_or(&self.fallback, MediaFetcher::resources);
        let membership = fetcher.map_or_else(|| Arc::<[u64]>::from([]), |f| Arc::clone(f.owner()));
        let source_bytes = match &source.parsed {
            Parsed::Data(data) => data.bytes.len(),
            _ => source.raw.len(),
        } + key.owner.len();
        match cache.load::<Loader>(&membership, &key, source_bytes, waiter, wake) {
            Load::Ready(result) => Some(result),
            Load::Pending => None,
            Load::Start(ticket) => {
                let mut state = self.state.borrow_mut();
                let id = state.next;
                state.next = state.next.wrapping_add(1);
                let sender = state.delivery.sender();
                let source = source.clone();
                let transport = fetcher.map(MediaFetcher::provider);
                let task = runtime::runtime().spawn(async move {
                    let fetcher = transport.map(|provider| {
                        let scope = provider.scope().to_owned();
                        MediaFetcher::new(scope, move |request| provider.fetch(request))
                    });
                    let loaded =
                        source::load_image_bytes(&source, fetcher.as_ref(), &media_http::client())
                            .await;
                    let result = match loaded {
                        Err(error) => Err(load_error(error).to_owned()),
                        Ok(bytes) => tokio::task::spawn_blocking(move || {
                            let _permit = bytes.permit;
                            parse(&bytes.bytes, format, up)
                                .map(|model| Arc::new(LoadedModel::from_model(model, renderer)))
                        })
                        .await
                        .unwrap_or_else(|_| Err("the file could not be read".to_owned())),
                    };
                    let _ = sender.send(Completion { id, result }).await;
                });
                let abort = task.abort_handle();
                if ticket.attach(abort.clone()) {
                    state.jobs.insert(id, Job { ticket, abort });
                } else {
                    abort.abort();
                }
                None
            }
        }
    }
    pub fn drain(&self) -> bool {
        self.mailbox.borrow_mut().drain();
        let completions = self.state.borrow_mut().delivery.drain();
        let mut changed = false;
        for completion in completions {
            let Some(job) = self.state.borrow_mut().jobs.remove(&completion.id) else {
                continue;
            };
            if !job.ticket.current() {
                job.abort.abort();
                continue;
            }
            let size = completion
                .result
                .as_ref()
                .map_or(0, |model| model.resident_bytes);
            job.ticket.publish(completion.result, size, drop);
            changed = true;
        }
        self.discard_stale();
        changed
    }
    fn discard_stale(&self) {
        self.state.borrow_mut().jobs.retain(|_, job| {
            let current = job.ticket.current();
            if !current {
                job.abort.abort();
            }
            current
        });
    }
}
fn load_error(error: MediaLoadError) -> &'static str {
    match error {
        MediaLoadError::Invalid => "the path is not valid",
        MediaLoadError::Fetch(MediaFailure::Missing) => "the file does not exist",
        MediaLoadError::Fetch(MediaFailure::Unreachable) => "the host did not answer",
        MediaLoadError::Fetch(MediaFailure::Unreadable) => "the file could not be read",
    }
}

#[cfg(test)]
#[path = "../../../../tests/ui/media/model/load.rs"]
mod tests;
