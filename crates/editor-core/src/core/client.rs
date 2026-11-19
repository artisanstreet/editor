mod error;

use crate::core::runtime::runtime;
use artisan_api::tls::Identity;
use artisan_api::{
    Answer, ApiError, BlobHash, BlobRef, CreateProject, CreateThread, DirListing, EngineId,
    EngineStatus, Item, ItemId, ItemsPage, ItemsQuery, PairRequest, PairResponse, Project,
    ProjectId, SendMessage, State, Thread, ThreadId, UpdateThread,
};
pub use error::{
    CallError, CallFailure, CopyContentError, ExportContentError, ForgeFault, NewClientError,
    RangeError, ReadBodyError, RelayClientError, ResponseError,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::sync::Arc;
use tokio_util::task::AbortOnDropHandle;

#[derive(Debug, Clone)]
pub struct ForgeClient {
    http: reqwest::Client,
    transport: Transport,
}

#[derive(Clone)]
enum Transport {
    Direct(String),
    Relay(Arc<artisan_account::relay_client::RelayClient>),
}

impl std::fmt::Debug for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Direct(base) => f.debug_tuple("Direct").field(base).finish(),
            Self::Relay(_) => f.write_str("Relay"),
        }
    }
}

struct Request {
    method: reqwest::Method,
    path: String,
    builder: reqwest::RequestBuilder,
}

impl Request {
    fn query<Q: Serialize + ?Sized>(self, query: &Q) -> Self {
        Self {
            builder: self.builder.query(query),
            ..self
        }
    }

    fn json<B: Serialize + ?Sized>(self, body: &B) -> Self {
        Self {
            builder: self.builder.json(body),
            ..self
        }
    }

    fn body(self, body: Vec<u8>) -> Self {
        Self {
            builder: self.builder.body(body),
            ..self
        }
    }
}

const RELAY_ORIGIN: &str = "https://relay.tunnel";
const ANSWER_BYTES: usize = 32 * 1024 * 1024;
const RELAY_BYTES: usize = artisan_api::TUNNEL_BODY_MAX_BYTES;
const RESOURCE_BYTES: usize = 128 * 1024 * 1024;
const REFUSAL_BYTES: usize = 64 * 1024;

impl ForgeClient {
    pub fn new(
        address: &str,
        fingerprint: &str,
        identity: &Identity,
    ) -> Result<Self, NewClientError> {
        let tls = artisan_api::tls::client_config(identity, fingerprint).map_err(|source| {
            NewClientError::Tls {
                address: address.to_owned(),
                source,
            }
        })?;
        let http = reqwest::Client::builder()
            .use_preconfigured_tls(tls)
            .connect_timeout(std::time::Duration::from_secs(10))
            .tcp_keepalive(std::time::Duration::from_secs(60))
            .pool_idle_timeout(None)
            .zstd(!crate::core::hosts::is_loopback(address))
            .build()
            .map_err(|source| NewClientError::Build {
                address: address.to_owned(),
                source,
            })?;
        Ok(Self {
            http,
            transport: Transport::Direct(format!("https://{address}")),
        })
    }

    #[must_use]
    pub(crate) const fn is_relay(&self) -> bool {
        matches!(self.transport, Transport::Relay(_))
    }

    #[must_use]
    pub(crate) const fn relay(&self) -> Option<&Arc<artisan_account::relay_client::RelayClient>> {
        match &self.transport {
            Transport::Relay(relay) => Some(relay),
            Transport::Direct(_) => None,
        }
    }

    pub(crate) fn close_relay(&self) {
        if let Some(relay) = self.relay() {
            relay.close_all();
        }
    }

    fn url(&self, path: &str) -> String {
        let origin = match &self.transport {
            Transport::Direct(base) => base.as_str(),
            Transport::Relay(_) => RELAY_ORIGIN,
        };
        format!("{origin}{path}")
    }

    pub(crate) const fn raw(&self) -> &reqwest::Client {
        &self.http
    }

    pub(crate) fn events_url(&self, since: artisan_api::Seq) -> String {
        self.url(&format!("/events?since={since}"))
    }

    fn request(&self, method: reqwest::Method, path: &str) -> Request {
        let builder = self.http.request(method.clone(), self.url(path));
        Request {
            method,
            path: path.to_owned(),
            builder,
        }
    }

    fn json<B: Serialize + ?Sized>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: &B,
    ) -> Request {
        self.request(method, path).json(body)
    }

    async fn call<T: DeserializeOwned + Send + 'static>(
        &self,
        request: Request,
    ) -> Result<T, CallError> {
        self.call_decoded(request, ANSWER_BYTES, None, |bytes| {
            serde_json::from_slice(&bytes).map_err(CallFailure::Decode)
        })
        .await
    }

    async fn call_empty(&self, request: Request) -> Result<(), CallError> {
        self.call_decoded(request, ANSWER_BYTES, None, |_bytes| Ok(()))
            .await
    }

    async fn call_bytes(&self, request: Request, limit: usize) -> Result<Vec<u8>, CallError> {
        self.call_decoded(request, limit, None, Ok).await
    }

    async fn call_decoded<T: Send + 'static>(
        &self,
        request: Request,
        limit: usize,
        admission: Option<Arc<tokio::sync::Semaphore>>,
        decode: impl FnOnce(Vec<u8>) -> Result<T, CallFailure> + Send + 'static,
    ) -> Result<T, CallError> {
        let transport = self.transport.clone();
        let Request {
            method,
            path,
            builder,
        } = request;
        let task =
            runtime().spawn(async move {
                let permit =
                    match admission {
                        Some(admission) => Some(admission.acquire_owned().await.expect(
                            "invariant: Forge request admission semaphores are never closed",
                        )),
                        None => None,
                    };
                let bytes = match transport {
                    Transport::Direct(_) => direct_bytes(builder, limit).await?,
                    Transport::Relay(relay) => {
                        let (method, path, body) = relay_parts(builder)?;
                        relay
                            .raw_bytes(&method, &path, body, limit.min(RELAY_BYTES))
                            .await
                            .map_err(CallFailure::Relay)?
                    }
                };
                runtime()
                    .spawn_blocking(move || {
                        let _permit = permit;
                        decode(bytes)
                    })
                    .await
                    .map_err(CallFailure::Task)?
            });
        let failed = |failure| CallError {
            method: method.clone(),
            path: path.clone(),
            failure: Box::new(failure),
        };
        AbortOnDropHandle::new(task)
            .await
            .map_err(|error| failed(CallFailure::Task(error)))?
            .map_err(failed)
    }

    pub async fn state(&self) -> Result<State, CallError> {
        self.call_decoded(
            self.request(reqwest::Method::GET, "/state"),
            ANSWER_BYTES,
            None,
            |bytes| decode_state(&bytes),
        )
        .await
    }

    pub async fn transcript(
        &self,
        thread: &ThreadId,
        query: &ItemsQuery,
    ) -> Result<ItemsPage, CallError> {
        self.call(
            self.request(
                reqwest::Method::GET,
                &format!("/threads/{thread}/transcript"),
            )
            .query(query),
        )
        .await
    }

    pub async fn work(
        &self,
        thread: &ThreadId,
        query: &ItemsQuery,
    ) -> Result<ItemsPage, CallError> {
        self.call(
            self.request(reqwest::Method::GET, &format!("/threads/{thread}/work"))
                .query(query),
        )
        .await
    }

    pub async fn agent_items(
        &self,
        thread: &ThreadId,
        agent: &ItemId,
        query: &ItemsQuery,
    ) -> Result<ItemsPage, CallError> {
        self.call(
            self.request(
                reqwest::Method::GET,
                &format!("/threads/{thread}/agents/{agent}/items"),
            )
            .query(query),
        )
        .await
    }

    pub async fn resource(
        &self,
        thread: &ThreadId,
        path: &str,
        from_home: bool,
        item: Option<&str>,
    ) -> Result<Vec<u8>, CallError> {
        let shown = item.map(|item| ("item", item));
        self.call_bytes(
            self.request(reqwest::Method::GET, &format!("/threads/{thread}/resource"))
                .query(&[
                    ("path", path),
                    ("from_home", if from_home { "true" } else { "false" }),
                ])
                .query(&shown.into_iter().collect::<Vec<_>>()),
            RESOURCE_BYTES,
        )
        .await
    }

    pub async fn mutate_mcp(
        &self,
        mutation: &artisan_api::McpMutation,
    ) -> Result<Vec<artisan_api::McpServer>, CallError> {
        self.call(self.json(reqwest::Method::PATCH, "/mcp", mutation))
            .await
    }

    pub(crate) async fn mcp_sign_in_start(
        &self,
        start: &artisan_api::McpSignInStart,
    ) -> Result<artisan_api::McpSignInStarted, CallError> {
        self.call(self.json(reqwest::Method::POST, "/mcp/sign-in/start", start))
            .await
    }

    pub(crate) async fn mcp_sign_in_finish(
        &self,
        finish: &artisan_api::McpSignInFinish,
    ) -> Result<String, CallError> {
        self.call(self.json(reqwest::Method::POST, "/mcp/sign-in/finish", finish))
            .await
    }

    pub(crate) async fn mcp_sign_in_cancel(&self, state: &str) -> Result<(), CallError> {
        self.call_empty(self.json(
            reqwest::Method::POST,
            "/mcp/sign-in/cancel",
            &serde_json::json!({ "state": state }),
        ))
        .await
    }

    pub async fn mcp_sign_out(&self, out: &artisan_api::McpSignOut) -> Result<(), CallError> {
        self.call_empty(self.json(reqwest::Method::POST, "/mcp/sign-out", out))
            .await
    }

    pub(crate) async fn set_model_config(
        &self,
        config: &artisan_api::EngineConfig,
    ) -> Result<artisan_api::EngineConfig, CallError> {
        self.call(self.json(
            reqwest::Method::PUT,
            &format!("/engines/{}/model-config", config.engine),
            config,
        ))
        .await
    }

    pub(crate) async fn set_model_group(
        &self,
        engine: EngineId,
        group: &artisan_api::ModelGroup,
    ) -> Result<(), CallError> {
        self.call_empty(self.json(
            reqwest::Method::PUT,
            &format!("/engines/{engine}/model-groups"),
            group,
        ))
        .await
    }

    pub async fn set_setting(&self, key: &str, value: &serde_json::Value) -> Result<(), CallError> {
        self.call_empty(self.json(reqwest::Method::PUT, &format!("/settings/{key}"), value))
            .await
    }

    pub async fn create_project(&self, request: &CreateProject) -> Result<Project, CallError> {
        self.call(self.json(reqwest::Method::POST, "/projects", request))
            .await
    }

    pub async fn create_thread(&self, request: &CreateThread) -> Result<Thread, CallError> {
        self.call(self.json(reqwest::Method::POST, "/threads", request))
            .await
    }

    pub async fn threads(
        &self,
        query: &artisan_api::ThreadsQuery,
    ) -> Result<artisan_api::ThreadsPage, CallError> {
        self.call(self.request(reqwest::Method::GET, "/threads").query(query))
            .await
    }

    pub async fn text_search(
        &self,
        query: &artisan_api::TextSearchQuery,
    ) -> Result<Vec<artisan_api::TextHit>, CallError> {
        self.call(
            self.request(reqwest::Method::GET, "/search/text")
                .query(query),
        )
        .await
    }

    pub async fn thread(&self, id: &ThreadId) -> Result<Thread, CallError> {
        self.call(self.request(reqwest::Method::GET, &format!("/threads/{id}")))
            .await
    }

    pub async fn update_thread(
        &self,
        id: &ThreadId,
        update: &UpdateThread,
    ) -> Result<Thread, CallError> {
        self.call(self.json(reqwest::Method::PATCH, &format!("/threads/{id}"), update))
            .await
    }

    pub async fn send_message(
        &self,
        thread: &ThreadId,
        message: &SendMessage,
    ) -> Result<Item, CallError> {
        self.call(self.json(
            reqwest::Method::POST,
            &format!("/threads/{thread}/messages"),
            message,
        ))
        .await
    }

    pub(crate) async fn inspector(
        &self,
        id: &ThreadId,
    ) -> Result<artisan_api::InspectorSummary, CallError> {
        self.call(self.request(reqwest::Method::GET, &format!("/threads/{id}/inspector")))
            .await
    }

    #[expect(
        clippy::suspicious_operation_groupings,
        reason = "range totals are checked against the reference's byte count on purpose"
    )]
    pub(crate) async fn content_range(
        &self,
        source: &artisan_api::ContentRef,
        offset: u64,
        limit: u32,
    ) -> Result<artisan_api::ContentRange, RangeError> {
        let source_id = source.id.clone();
        if !source.id.is_valid() {
            return Err(RangeError::InvalidSource { source_id });
        }
        if offset > source.bytes || !(4..=artisan_api::CONTENT_RANGE_BYTES).contains(&limit) {
            return Err(RangeError::InvalidRange {
                source_id,
                offset,
                limit: u64::from(limit),
            });
        }
        let expected = source.clone();
        self.call_decoded(
            self.request(reqwest::Method::GET, &format!("/content/{}", source.id))
                .query(&artisan_api::ContentQuery {
                    offset,
                    limit: Some(limit),
                }),
            limit as usize * 6 + 4096,
            Some(Arc::clone(source_ranges())),
            move |bytes| {
                let mut range: artisan_api::ContentRange =
                    serde_json::from_slice(&bytes).map_err(CallFailure::Decode)?;
                if !(range.id == expected.id
                    && range.total == expected.bytes
                    && range.start <= offset
                    && offset - range.start <= 3
                    && range.end <= range.total
                    && range.end >= range.start
                    && range.end - range.start == range.text.len() as u64
                    && range.text.len() <= limit as usize
                    && (range.end > offset || offset == expected.bytes))
                {
                    return Err(CallFailure::Inconsistent {
                        check: "source range",
                    });
                }
                range.text.shrink_to_fit();
                Ok(range)
            },
        )
        .await
        .map_err(|source| RangeError::Call {
            source_id,
            offset,
            source,
        })
    }

    #[expect(
        clippy::suspicious_operation_groupings,
        reason = "range totals are checked against the reference's record count on purpose"
    )]
    pub(crate) async fn collection_range<T: DeserializeOwned + Send + 'static>(
        &self,
        reference: &artisan_api::CollectionRef,
        offset: u64,
        limit: u32,
    ) -> Result<artisan_api::CollectionRange<T>, RangeError> {
        let source_id = reference.source.id.clone();
        if !reference.source.id.is_valid() {
            return Err(RangeError::InvalidSource { source_id });
        }
        if offset > reference.count || !(1..=artisan_api::COLLECTION_RANGE_ENTRIES).contains(&limit)
        {
            return Err(RangeError::InvalidRange {
                source_id,
                offset,
                limit: u64::from(limit),
            });
        }
        let expected = reference.clone();
        self.call_decoded(
            self.request(
                reqwest::Method::GET,
                &format!("/content/{}/records", reference.source.id),
            )
            .query(&artisan_api::CollectionQuery {
                offset,
                limit: Some(limit),
            }),
            artisan_api::COLLECTION_RANGE_BYTES * 6 + 4096,
            Some(Arc::clone(source_ranges())),
            move |bytes| {
                let range: artisan_api::CollectionRange<T> =
                    serde_json::from_slice(&bytes).map_err(CallFailure::Decode)?;
                if !(range.source == expected.source
                    && range.total == expected.count
                    && range.start == offset
                    && range.end >= offset
                    && range.end <= expected.count
                    && range.end - offset == range.records.len() as u64
                    && range.records.len() <= limit as usize
                    && (range.end > offset || offset == expected.count))
                {
                    return Err(CallFailure::Inconsistent {
                        check: "collection range",
                    });
                }
                Ok(range)
            },
        )
        .await
        .map_err(|source| RangeError::Call {
            source_id,
            offset,
            source,
        })
    }

    #[expect(
        clippy::suspicious_operation_groupings,
        reason = "leaf offsets are checked against the source's byte count on purpose"
    )]
    pub(crate) async fn source_index_range(
        &self,
        source: &artisan_api::ContentRef,
        format: artisan_api::SourceIndexFormat,
        offset: Option<u64>,
        limit: u32,
    ) -> Result<artisan_api::SourceIndexRange, RangeError> {
        static INDEX_REQUESTS: std::sync::OnceLock<Arc<tokio::sync::Semaphore>> =
            std::sync::OnceLock::new();
        let source_id = source.id.clone();
        if !source.id.is_valid() {
            return Err(RangeError::InvalidSource { source_id });
        }
        if !(1..=artisan_api::SOURCE_INDEX_PAGE_LEAVES).contains(&limit) {
            return Err(RangeError::InvalidRange {
                source_id,
                offset: offset.unwrap_or(0),
                limit: u64::from(limit),
            });
        }
        let admission =
            Arc::clone(INDEX_REQUESTS.get_or_init(|| Arc::new(tokio::sync::Semaphore::new(4))));
        let expected = source.clone();
        self.call_decoded(
            self.request(
                reqwest::Method::GET,
                &format!("/content/{}/index", source.id),
            )
            .query(&artisan_api::SourceIndexQuery {
                format,
                offset,
                limit: Some(limit),
            }),
            65536,
            Some(admission),
            move |bytes| {
                let range: artisan_api::SourceIndexRange =
                    serde_json::from_slice(&bytes).map_err(CallFailure::Decode)?;
                if !(range.index.source == expected
                    && range.index.format == format
                    && range.index.version == artisan_api::SOURCE_INDEX_VERSION
                    && range.index.metadata.id.is_valid()
                    && range.start <= range.end
                    && range.end <= range.index.leaves
                    && range.end - range.start == range.leaves.len() as u64
                    && range.leaves.len() <= limit as usize
                    && offset.is_none_or(|offset| range.start == offset.min(range.index.leaves))
                    && (offset.is_some() || range.end == range.index.leaves))
                {
                    return Err(CallFailure::Inconsistent {
                        check: "semantic source header",
                    });
                }
                let leaves_consistent = range.leaves.iter().enumerate().all(|(index, leaf)| {
                    leaf.ordinal == range.start + index as u64
                        && leaf.source_start <= leaf.source_end
                        && leaf.source_end <= expected.bytes
                        && leaf.metadata_start < leaf.metadata_end
                        && leaf.metadata_end <= range.index.metadata.bytes
                });
                if !leaves_consistent {
                    return Err(CallFailure::Inconsistent {
                        check: "semantic leaf descriptor",
                    });
                }
                Ok(range)
            },
        )
        .await
        .map_err(|source| RangeError::Call {
            source_id,
            offset: offset.unwrap_or(0),
            source,
        })
    }

    pub async fn copy_content(
        &self,
        source: &artisan_api::ContentRef,
        limit: usize,
    ) -> Result<String, CopyContentError> {
        let source_id = source.id.clone();
        let capacity = usize::try_from(source.bytes)
            .ok()
            .filter(|bytes| *bytes <= limit.min(32 * 1024 * 1024))
            .ok_or_else(|| CopyContentError::TooLarge {
                source_id: source_id.clone(),
                bytes: source.bytes,
            })?;
        let mut text = String::with_capacity(capacity);
        let mut offset = 0;
        while offset < source.bytes {
            let range = self
                .content_range(source, offset, artisan_api::CONTENT_RANGE_BYTES)
                .await
                .map_err(|source| CopyContentError::Range {
                    source_id: source_id.clone(),
                    source,
                })?;
            if range.start != offset {
                return Err(CopyContentError::Noncontiguous {
                    source_id,
                    offset,
                    start: range.start,
                });
            }
            offset = range.end;
            text.push_str(&range.text);
        }
        Ok(text)
    }

    pub async fn export_content(
        &self,
        source: &artisan_api::ContentRef,
        path: std::path::PathBuf,
    ) -> Result<(), ExportContentError> {
        use std::io::Write as _;
        let source_id = source.id.clone();
        let parent = path
            .parent()
            .ok_or_else(|| ExportContentError::NoParent { path: path.clone() })?
            .to_owned();
        let task = |path: &std::path::PathBuf| {
            let path = path.clone();
            move |source| ExportContentError::Task { path, source }
        };
        let mut file = runtime()
            .spawn_blocking(move || tempfile::NamedTempFile::new_in(parent))
            .await
            .map_err(task(&path))?
            .map_err(|source| ExportContentError::Create {
                path: path.clone(),
                source,
            })?;
        let mut offset = 0;
        while offset < source.bytes {
            let range = self
                .content_range(source, offset, artisan_api::CONTENT_RANGE_BYTES)
                .await
                .map_err(|source| ExportContentError::Range {
                    source_id: source_id.clone(),
                    source,
                })?;
            if range.start != offset {
                return Err(ExportContentError::Noncontiguous {
                    source_id,
                    offset,
                    start: range.start,
                });
            }
            offset = range.end;
            file = runtime()
                .spawn_blocking(move || file.write_all(range.text.as_bytes()).map(|()| file))
                .await
                .map_err(task(&path))?
                .map_err(|source| ExportContentError::Write {
                    path: path.clone(),
                    source,
                })?;
        }
        let destination = path.clone();
        runtime()
            .spawn_blocking(move || {
                file.flush().map_err(|source| ExportContentError::Write {
                    path: destination.clone(),
                    source,
                })?;
                file.persist(&destination)
                    .map(drop)
                    .map_err(|error| ExportContentError::Persist {
                        path: destination,
                        source: error.error,
                    })
            })
            .await
            .map_err(task(&path))?
    }

    pub async fn item(&self, id: &ItemId) -> Result<Item, CallError> {
        self.call(self.request(reqwest::Method::GET, &format!("/items/{id}")))
            .await
    }

    pub async fn settle_thread(
        &self,
        id: &ThreadId,
        observed_seq: artisan_api::Seq,
    ) -> Result<Thread, CallError> {
        self.call(self.json(
            reqwest::Method::POST,
            &format!("/threads/{id}/settle"),
            &artisan_api::SettleThread { observed_seq },
        ))
        .await
    }

    pub async fn mark_unread(&self, id: &ThreadId) -> Result<Thread, CallError> {
        self.call(self.request(reqwest::Method::POST, &format!("/threads/{id}/unread")))
            .await
    }

    pub async fn dismiss_database_rollback(&self) -> Result<(), CallError> {
        self.call_empty(self.request(reqwest::Method::DELETE, "/database-rollback"))
            .await
    }

    pub async fn cancel(&self, thread: &ThreadId) -> Result<(), CallError> {
        self.call_empty(self.request(reqwest::Method::POST, &format!("/threads/{thread}/cancel")))
            .await
    }

    pub async fn continue_turn(&self, thread: &ThreadId) -> Result<(), CallError> {
        self.call_empty(self.request(
            reqwest::Method::POST,
            &format!("/threads/{thread}/continue"),
        ))
        .await
    }

    pub async fn clear_goal(&self, thread: &ThreadId) -> Result<(), CallError> {
        self.call_empty(self.request(
            reqwest::Method::POST,
            &format!("/threads/{thread}/goal/clear"),
        ))
        .await
    }

    pub(crate) async fn reply_editor_call(
        &self,
        id: &str,
        reply: &artisan_api::EditorReply,
    ) -> Result<(), CallError> {
        self.call_empty(self.json(reqwest::Method::POST, &format!("/editor/calls/{id}"), reply))
            .await
    }

    pub async fn answer(&self, item: &ItemId, answer: &Answer) -> Result<(), CallError> {
        self.call_empty(self.json(
            reqwest::Method::POST,
            &format!("/items/{item}/answer"),
            answer,
        ))
        .await
    }

    pub async fn draft(&self, scope: &str) -> Result<artisan_api::DraftSnapshot, CallError> {
        self.call(self.request(reqwest::Method::GET, &format!("/drafts/{scope}")))
            .await
    }

    pub async fn set_draft(
        &self,
        scope: &str,
        value: &artisan_api::WriteDraft,
    ) -> Result<artisan_api::DraftWriteResult, CallError> {
        self.call(self.json(reqwest::Method::PUT, &format!("/drafts/{scope}"), value))
            .await
    }

    pub async fn put_blob(&self, webp: Vec<u8>) -> Result<BlobRef, CallError> {
        self.call(self.request(reqwest::Method::PUT, "/blobs").body(webp))
            .await
    }

    pub async fn blob(&self, hash: &BlobHash) -> Result<Vec<u8>, CallError> {
        self.call_bytes(
            self.request(reqwest::Method::GET, &format!("/blobs/{hash}")),
            RELAY_BYTES,
        )
        .await
    }

    pub async fn engine_action(
        &self,
        engine: EngineId,
        command: &artisan_api::EngineCommand,
    ) -> Result<EngineStatus, CallError> {
        self.call(self.json(
            reqwest::Method::POST,
            &format!("/engines/{engine}/commands"),
            command,
        ))
        .await
    }

    pub(crate) async fn project_icon(
        &self,
        project: &ProjectId,
    ) -> Result<Option<BlobRef>, CallError> {
        self.call(self.request(reqwest::Method::GET, &format!("/projects/{project}/icon")))
            .await
    }

    pub(crate) async fn project_repository(
        &self,
        project: &ProjectId,
    ) -> Result<Option<artisan_api::ProjectRepository>, CallError> {
        self.call(self.request(
            reqwest::Method::GET,
            &format!("/projects/{project}/repository"),
        ))
        .await
    }

    pub(crate) async fn skill_catalog(&self) -> Result<Vec<artisan_api::CatalogSkill>, CallError> {
        self.call(self.request(reqwest::Method::GET, "/skills/catalog"))
            .await
    }

    pub async fn skill_action(
        &self,
        id: &str,
        action: artisan_api::SkillAction,
    ) -> Result<Vec<artisan_api::CatalogSkill>, CallError> {
        self.call(self.request(
            reqwest::Method::POST,
            &format!("/skills/{id}/{}", action.as_str()),
        ))
        .await
    }

    pub async fn marketplace_search(
        &self,
        query: &str,
    ) -> Result<Vec<artisan_api::Listing>, CallError> {
        self.call(
            self.request(reqwest::Method::GET, "/marketplace/search")
                .query(&[("q", query)]),
        )
        .await
    }

    pub async fn marketplace_install(
        &self,
        id: &str,
    ) -> Result<Vec<artisan_api::CatalogSkill>, CallError> {
        let install = artisan_api::InstallListing { id: id.to_owned() };
        self.call(self.json(reqwest::Method::POST, "/marketplace/install", &install))
            .await
    }

    pub(crate) async fn global_prompt(&self) -> Result<artisan_api::GlobalPrompt, CallError> {
        self.call(self.request(reqwest::Method::GET, "/prompt"))
            .await
    }

    pub async fn save_global_prompt(
        &self,
        save: &artisan_api::SavePrompt,
    ) -> Result<artisan_api::GlobalPrompt, CallError> {
        self.call(self.json(reqwest::Method::PUT, "/prompt", save))
            .await
    }

    pub async fn resolve_global_prompt(
        &self,
        resolution: &artisan_api::ResolvePrompt,
    ) -> Result<artisan_api::GlobalPrompt, CallError> {
        self.call(self.json(reqwest::Method::POST, "/prompt/resolve", resolution))
            .await
    }

    pub async fn list_dir(&self, path: Option<&str>) -> Result<DirListing, CallError> {
        let mut request = self.request(reqwest::Method::GET, "/fs/list");
        if let Some(path) = path {
            request = request.query(&[("path", path)]);
        }
        self.call(request).await
    }

    pub(crate) async fn pair(&self, request: &PairRequest) -> Result<PairResponse, CallError> {
        self.call(self.json(reqwest::Method::POST, "/pair", request))
            .await
    }
}

async fn direct_bytes(
    builder: reqwest::RequestBuilder,
    limit: usize,
) -> Result<Vec<u8>, CallFailure> {
    let (client, request) = builder
        .timeout(std::time::Duration::from_secs(30))
        .build_split();
    let response = client
        .execute(request.map_err(CallFailure::Build)?)
        .await
        .map_err(CallFailure::Reach)?;
    let response = check(response).await.map_err(CallFailure::Answer)?;
    read_body(response, limit).await.map_err(CallFailure::Read)
}

fn relay_parts(
    builder: reqwest::RequestBuilder,
) -> Result<(String, String, Option<serde_json::Value>), CallFailure> {
    let built = builder.build().map_err(CallFailure::Build)?;
    let method = built.method().as_str().to_owned();
    let url = built.url();
    let mut path = url.path().to_owned();
    if let Some(query) = url.query()
        && !query.is_empty()
    {
        path.push('?');
        path.push_str(query);
    }
    let content_type = built
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let raw = built
        .body()
        .map(|body| body.as_bytes().ok_or(CallFailure::StreamingBody))
        .transpose()?
        .map(<[u8]>::to_vec);
    let body = match raw {
        None => None,
        Some(bytes) if bytes.is_empty() => None,
        Some(bytes) => {
            if content_type.starts_with("application/json") {
                Some(serde_json::from_slice(&bytes).map_err(CallFailure::EncodeBody)?)
            } else {
                use base64::Engine as _;
                Some(serde_json::json!({"$bytes": {
                    "content_type": if content_type.is_empty() {
                        "application/octet-stream".to_owned()
                    } else {
                        content_type
                    },
                    "b64u": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes),
                }}))
            }
        }
    };
    Ok((method, path, body))
}

impl ForgeClient {
    pub(crate) async fn via_relay(
        request: crate::core::services::RelayDialRequest,
        ctx: &artisan_account::relay_client::RelayDialContext,
    ) -> Result<Self, RelayClientError> {
        let ctx = ctx.clone();
        AbortOnDropHandle::new(
            runtime().spawn(async move { Self::via_relay_inner(request, &ctx).await }),
        )
        .await
        .map_err(RelayClientError::Task)?
    }

    async fn via_relay_inner(
        request: crate::core::services::RelayDialRequest,
        ctx: &artisan_account::relay_client::RelayDialContext,
    ) -> Result<Self, RelayClientError> {
        let relay = artisan_account::relay_client::RelayClient::connect(
            &ctx.relay_base_url,
            &request.account_id,
            &request.machine_id,
            &request.grant_jwt,
            &request.cnf_secret,
            &request.forge_ed25519_b64u,
            ctx.core.clone(),
            ctx.session.clone(),
        )
        .await
        .map_err(RelayClientError::Connect)?;
        let live: serde_json::Value = relay
            .request_json("GET", "/state", None)
            .await
            .map_err(RelayClientError::State)?;
        let protocol = live.get("protocol").and_then(serde_json::Value::as_u64);
        if protocol != Some(u64::from(artisan_api::PROTOCOL_VERSION)) {
            return Err(RelayClientError::Incompatible { protocol });
        }
        Ok(Self {
            http: reqwest::Client::new(),
            transport: Transport::Relay(Arc::new(relay)),
        })
    }
}

pub(crate) async fn check(response: reqwest::Response) -> Result<reqwest::Response, ResponseError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = read_body(response, REFUSAL_BYTES)
        .await
        .map_err(|source| ResponseError::Unreadable { status, source })?;
    match serde_json::from_slice::<ApiError>(&body) {
        Ok(error) => Err(ResponseError::Refused { status, error }),
        Err(_) => Err(ResponseError::Unexplained {
            status,
            body: String::from_utf8_lossy(&body).trim().to_owned(),
        }),
    }
}

fn decode_state(bytes: &[u8]) -> Result<State, CallFailure> {
    #[derive(serde::Deserialize)]
    struct Version {
        protocol: u32,
        forge_version: String,
    }
    let version: Version = serde_json::from_slice(bytes).map_err(CallFailure::Decode)?;
    if version.protocol != artisan_api::PROTOCOL_VERSION {
        return Err(CallFailure::Incompatible {
            forge_version: version.forge_version,
            protocol: version.protocol,
        });
    }
    serde_json::from_slice(bytes).map_err(CallFailure::Decode)
}

pub(crate) async fn read_body(
    response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, ReadBodyError> {
    use futures::StreamExt as _;
    if response
        .content_length()
        .is_some_and(|size| size > limit as u64)
    {
        return Err(ReadBodyError::TooLarge { limit });
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(ReadBodyError::Chunk)?;
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(ReadBodyError::TooLarge { limit });
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn source_ranges() -> &'static Arc<tokio::sync::Semaphore> {
    static RANGES: std::sync::OnceLock<Arc<tokio::sync::Semaphore>> = std::sync::OnceLock::new();
    RANGES.get_or_init(|| Arc::new(tokio::sync::Semaphore::new(4)))
}
