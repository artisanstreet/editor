//! Plain HTTPS calls to one Forge. Every method runs on the network runtime,
//! so it can be awaited from GPUI's executor.

use crate::runtime::runtime;
use anyhow::{Context, Result, anyhow};
use artisan_api::tls::Identity;
use artisan_api::{
    Answer, ApiError, BlobHash, BlobRef, CreateProject, CreateThread, DirListing, EngineId,
    EngineStatus, Item, ItemId, ItemsPage, ItemsQuery, PairRequest, PairResponse, Project,
    ProjectId, SendMessage, State, Thread, ThreadId, UpdateThread,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio_util::task::AbortOnDropHandle;

#[derive(Clone)]
pub struct ForgeClient {
    http: reqwest::Client,
    transport: Transport,
}

#[derive(Clone)]
enum Transport {
    /// Mutual TLS to `https://host:port`.
    Direct(String),
    /// A Noise channel through the account Relay.
    Relay(Arc<artisan_account::relay_client::RelayClient>),
}

/// Requests for a Relay client are still built with `reqwest`, which needs an
/// absolute URL; only the path and query travel the tunnel.
const RELAY_ORIGIN: &str = "https://relay.tunnel";

impl ForgeClient {
    /// A client for the Forge at `address` (`host:port`) whose certificate
    /// has `fingerprint`, presenting this device's identity.
    pub fn new(address: &str, fingerprint: &str, identity: &Identity) -> Result<Self> {
        let tls = artisan_api::tls::client_config(identity, fingerprint)?;
        let http = reqwest::Client::builder()
            .use_preconfigured_tls(tls)
            .connect_timeout(std::time::Duration::from_secs(10))
            // Compressing costs more than it saves over loopback.
            .zstd(!is_loopback(address))
            .build()
            .context("build the HTTP client")?;
        Ok(Self {
            http,
            transport: Transport::Direct(format!("https://{address}")),
        })
    }

    /// Whether this client tunnels through the Relay instead of direct mTLS.
    #[must_use]
    pub const fn is_relay(&self) -> bool {
        matches!(self.transport, Transport::Relay(_))
    }

    /// The Relay transport behind a `via_relay` client, if any.
    #[must_use]
    pub const fn relay(&self) -> Option<&Arc<artisan_account::relay_client::RelayClient>> {
        match &self.transport {
            Transport::Relay(relay) => Some(relay),
            Transport::Direct(_) => None,
        }
    }

    /// Ends a Relay route at once, even while views still hold clones of
    /// this client. Direct clients ignore it.
    pub fn close_relay(&self) {
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

    async fn call<T: DeserializeOwned + Send + 'static>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T> {
        if self.is_relay() {
            return self
                .call_relay(request, 32 * 1024 * 1024, None, |bytes| {
                    serde_json::from_slice(&bytes).context("read Forge's answer")
                })
                .await;
        }
        self.call_decoded(request, |bytes| {
            serde_json::from_slice(&bytes).context("read Forge's answer")
        })
        .await
    }

    async fn call_decoded<T: Send + 'static>(
        &self,
        request: reqwest::RequestBuilder,
        decode: impl FnOnce(Vec<u8>) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        if self.is_relay() {
            return self
                .call_relay(request, 32 * 1024 * 1024, None, decode)
                .await;
        }
        self.call_decoded_limited(request, 32 * 1024 * 1024, None, decode)
            .await
    }

    async fn call_decoded_limited<T: Send + 'static>(
        &self,
        request: reqwest::RequestBuilder,
        limit: usize,
        admission: Option<std::sync::Arc<tokio::sync::Semaphore>>,
        decode: impl FnOnce(Vec<u8>) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        if self.is_relay() {
            return self.call_relay(request, limit, admission, decode).await;
        }
        AbortOnDropHandle::new(runtime().spawn(async move {
            // GPUI can stop polling a cancelled view. Keep queued semaphore
            // reservations under the same abort owner as the actual request.
            let permit = match admission {
                Some(admission) => Some(admission.acquire_owned().await?),
                None => None,
            };
            let response = request
                .timeout(std::time::Duration::from_secs(30))
                .send()
                .await
                .context("reach Forge")?;
            let bytes = read_body(check(response).await?, limit).await?;
            runtime()
                .spawn_blocking(move || {
                    let _permit = permit;
                    decode(bytes)
                })
                .await
                .context("decode Forge response task")?
        }))
        .await
        .context("network task")?
    }

    /// One tunneled call through the shared helpers: converts the built
    /// HTTP request (method, origin-form path+query, JSON or `$bytes` body)
    /// into a `TunnelRequest`, preserving status/error behavior, the byte
    /// limit, semaphore admission, and abort-on-drop cancellation.
    /// Runs on the network runtime (never the GPUI executor).
    async fn call_relay<T: Send + 'static>(
        &self,
        request: reqwest::RequestBuilder,
        limit: usize,
        admission: Option<std::sync::Arc<tokio::sync::Semaphore>>,
        decode: impl FnOnce(Vec<u8>) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let relay = self.relay().cloned().context("Relay transport missing")?;
        AbortOnDropHandle::new(runtime().spawn(async move {
            let permit = match admission {
                Some(admission) => Some(admission.acquire_owned().await?),
                None => None,
            };
            let (method, path, body) = relay_parts(request)?;
            let bytes = relay.raw_bytes(&method, &path, body, limit).await?;
            runtime()
                .spawn_blocking(move || {
                    let _permit = permit;
                    decode(bytes)
                })
                .await
                .context("decode Forge response task")?
        }))
        .await
        .context("network task")?
    }

    async fn call_empty(&self, request: reqwest::RequestBuilder) -> Result<()> {
        if self.is_relay() {
            return self
                .call_relay(request, 32 * 1024 * 1024, None, |_bytes| Ok(()))
                .await;
        }
        AbortOnDropHandle::new(runtime().spawn(async move {
            let response = request
                .timeout(std::time::Duration::from_secs(30))
                .send()
                .await
                .context("reach Forge")?;
            check(response).await.map(drop)
        }))
        .await
        .context("network task")?
    }

    /// Raw-byte download for both transports (images, exported sources):
    /// direct streams the body, Relay decodes the `$bytes` envelope.
    /// Enforces the 32 MiB ceiling either way, never truncates.
    async fn call_bytes(&self, request: reqwest::RequestBuilder) -> Result<Vec<u8>> {
        const LIMIT: usize = 32 * 1024 * 1024;
        if let Some(relay) = self.relay().cloned() {
            return AbortOnDropHandle::new(runtime().spawn(async move {
                let (method, path, body) = relay_parts(request)?;
                relay.raw_bytes(&method, &path, body, LIMIT).await
            }))
            .await
            .context("network task")?;
        }
        AbortOnDropHandle::new(runtime().spawn(async move {
            use futures::StreamExt as _;
            let response = check(
                request
                    .timeout(std::time::Duration::from_secs(30))
                    .send()
                    .await
                    .context("reach Forge")?,
            )
            .await?;

            let mut stream = response.bytes_stream();
            let mut bytes = Vec::new();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                anyhow::ensure!(
                    bytes.len().saturating_add(chunk.len()) <= LIMIT,
                    "image exceeds 32 MiB"
                );
                bytes.extend_from_slice(&chunk);
            }
            Ok(bytes)
        }))
        .await
        .context("network task")?
    }

    fn json<B: Serialize>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: &B,
    ) -> reqwest::RequestBuilder {
        self.http.request(method, self.url(path)).json(body)
    }

    pub async fn state(&self) -> Result<State> {
        self.call_decoded(self.http.get(self.url("/state")), |bytes| {
            decode_state(&bytes)
        })
        .await
    }

    pub async fn items(&self, thread: &ThreadId, query: &ItemsQuery) -> Result<ItemsPage> {
        self.call(
            self.http
                .get(self.url(&format!("/threads/{thread}/items")))
                .query(query),
        )
        .await
    }

    /// The root transcript in rows, the work between them as spans.
    pub async fn transcript(&self, thread: &ThreadId, query: &ItemsQuery) -> Result<ItemsPage> {
        self.call(
            self.http
                .get(self.url(&format!("/threads/{thread}/transcript")))
                .query(query),
        )
        .await
    }

    /// Root work between `after` and `before`, newest first.
    pub async fn work(&self, thread: &ThreadId, query: &ItemsQuery) -> Result<ItemsPage> {
        self.call(
            self.http
                .get(self.url(&format!("/threads/{thread}/work")))
                .query(query),
        )
        .await
    }

    pub async fn agent_items(
        &self,
        thread: &ThreadId,
        agent: &ItemId,
        query: &ItemsQuery,
    ) -> Result<ItemsPage> {
        self.call(
            self.http
                .get(self.url(&format!("/threads/{thread}/agents/{agent}/items")))
                .query(query),
        )
        .await
    }

    /// Reads a resource where the producing Forge resolves the path.
    pub async fn resource(
        &self,
        thread: &ThreadId,
        path: &str,
        from_home: bool,
    ) -> Result<Vec<u8>> {
        self.call_bytes(
            self.http
                .get(self.url(&format!("/threads/{thread}/resource")))
                .query(&[
                    ("path", path),
                    ("from_home", if from_home { "true" } else { "false" }),
                ]),
        )
        .await
    }

    pub async fn mutate_mcp(
        &self,
        mutation: &artisan_api::McpMutation,
    ) -> Result<Vec<artisan_api::McpServer>> {
        self.call(self.json(reqwest::Method::PATCH, "/mcp", mutation))
            .await
    }

    /// Starts signing in an HTTP MCP server; Forge answers the page to open.
    pub async fn mcp_sign_in_start(
        &self,
        start: &artisan_api::McpSignInStart,
    ) -> Result<artisan_api::McpSignInStarted> {
        self.call(self.json(reqwest::Method::POST, "/mcp/sign-in/start", start))
            .await
    }

    /// Hands Forge the code a sign-in's redirect brought back; answers the
    /// server's name.
    pub async fn mcp_sign_in_finish(
        &self,
        finish: &artisan_api::McpSignInFinish,
    ) -> Result<String> {
        self.call(self.json(reqwest::Method::POST, "/mcp/sign-in/finish", finish))
            .await
    }

    pub async fn mcp_sign_in_cancel(&self, state: &str) -> Result<()> {
        self.call_empty(self.json(
            reqwest::Method::POST,
            "/mcp/sign-in/cancel",
            &serde_json::json!({ "state": state }),
        ))
        .await
    }

    pub async fn mcp_sign_out(&self, out: &artisan_api::McpSignOut) -> Result<()> {
        self.call_empty(self.json(reqwest::Method::POST, "/mcp/sign-out", out))
            .await
    }

    pub async fn set_default_model(
        &self,
        engine: EngineId,
        model: &str,
    ) -> Result<artisan_api::EngineConfig> {
        self.call(self.json(
            reqwest::Method::PUT,
            &format!("/engines/{engine}/default-model"),
            &model,
        ))
        .await
    }

    /// Remembers the config chosen for one of `engine`'s models.
    pub async fn set_model_config(
        &self,
        config: &artisan_api::EngineConfig,
    ) -> Result<artisan_api::EngineConfig> {
        self.call(self.json(
            reqwest::Method::PUT,
            &format!("/engines/{}/model-config", config.engine),
            config,
        ))
        .await
    }

    /// Remembers whether one of `engine`'s model groups is open.
    pub async fn set_model_group(
        &self,
        engine: EngineId,
        group: &artisan_api::ModelGroup,
    ) -> Result<()> {
        self.call_empty(self.json(
            reqwest::Method::PUT,
            &format!("/engines/{engine}/model-groups"),
            group,
        ))
        .await
    }

    /// Saves one of Forge's plain settings: a switch, or Claude's transport.
    pub async fn set_setting(&self, key: &str, value: &serde_json::Value) -> Result<()> {
        self.call_empty(self.json(reqwest::Method::PUT, &format!("/settings/{key}"), value))
            .await
    }

    pub async fn create_project(&self, request: &CreateProject) -> Result<Project> {
        self.call(self.json(reqwest::Method::POST, "/projects", request))
            .await
    }

    pub async fn remove_project(&self, id: &ProjectId) -> Result<()> {
        self.call_empty(self.http.delete(self.url(&format!("/projects/{id}"))))
            .await
    }

    pub async fn create_thread(&self, request: &CreateThread) -> Result<Thread> {
        self.call(self.json(reqwest::Method::POST, "/threads", request))
            .await
    }

    pub async fn threads(
        &self,
        query: &artisan_api::ThreadsQuery,
    ) -> Result<artisan_api::ThreadsPage> {
        self.call(self.http.get(self.url("/threads")).query(query))
            .await
    }

    pub async fn thread(&self, id: &ThreadId) -> Result<Thread> {
        self.call(self.http.get(self.url(&format!("/threads/{id}"))))
            .await
    }

    pub async fn update_thread(&self, id: &ThreadId, update: &UpdateThread) -> Result<Thread> {
        self.call(self.json(reqwest::Method::PATCH, &format!("/threads/{id}"), update))
            .await
    }

    pub async fn send_message(&self, thread: &ThreadId, message: &SendMessage) -> Result<Item> {
        self.call(self.json(
            reqwest::Method::POST,
            &format!("/threads/{thread}/messages"),
            message,
        ))
        .await
    }

    pub async fn inspector(&self, id: &ThreadId) -> Result<artisan_api::InspectorSummary> {
        self.call(self.http.get(self.url(&format!("/threads/{id}/inspector"))))
            .await
    }

    /// One immutable UTF-8 source window. Exact identity and progress are checked
    /// before a caller can cache or append it.
    #[expect(
        clippy::suspicious_operation_groupings,
        reason = "range totals are checked against the reference's byte count on purpose"
    )]
    pub async fn content_range(
        &self,
        source: &artisan_api::ContentRef,
        offset: u64,
        limit: u32,
    ) -> Result<artisan_api::ContentRange> {
        anyhow::ensure!(source.id.is_valid(), "invalid source identity");
        anyhow::ensure!(
            offset <= source.bytes && (4..=artisan_api::CONTENT_RANGE_BYTES).contains(&limit),
            "invalid source range"
        );
        // Admission belongs to the abortable network task, including its wait.
        let expected = source.clone();
        self.call_decoded_limited(
            self.http
                .get(self.url(&format!("/content/{}", source.id)))
                .query(&artisan_api::ContentQuery {
                    offset,
                    limit: Some(limit),
                }),
            limit as usize * 6 + 4096,
            Some(std::sync::Arc::clone(source_ranges())),
            move |bytes| {
                let mut range: artisan_api::ContentRange = serde_json::from_slice(&bytes)?;
                anyhow::ensure!(
                    range.id == expected.id
                        && range.total == expected.bytes
                        && range.start <= offset
                        && offset - range.start <= 3
                        && range.end <= range.total
                        && range.end >= range.start
                        && range.end - range.start == range.text.len() as u64
                        && range.text.len() <= limit as usize
                        && (range.end > offset || offset == expected.bytes),
                    "Forge returned an inconsistent source range"
                );
                range.text.shrink_to_fit();
                Ok(range)
            },
        )
        .await
    }

    /// One canonical collection page. Full count stays visible while callers
    /// mount only fetched records; a missing range is an error, never an empty Vec.
    #[expect(
        clippy::suspicious_operation_groupings,
        reason = "range totals are checked against the reference's record count on purpose"
    )]
    pub async fn collection_range<T: serde::de::DeserializeOwned + Send + 'static>(
        &self,
        reference: &artisan_api::CollectionRef,
        offset: u64,
        limit: u32,
    ) -> Result<artisan_api::CollectionRange<T>> {
        anyhow::ensure!(
            reference.source.id.is_valid()
                && offset <= reference.count
                && (1..=artisan_api::COLLECTION_RANGE_ENTRIES).contains(&limit),
            "invalid collection range"
        );
        let expected = reference.clone();
        self.call_decoded_limited(
            self.http
                .get(self.url(&format!("/content/{}/records", reference.source.id)))
                .query(&artisan_api::CollectionQuery {
                    offset,
                    limit: Some(limit),
                }),
            artisan_api::COLLECTION_RANGE_BYTES * 6 + 4096,
            Some(std::sync::Arc::clone(source_ranges())),
            move |bytes| {
                let range: artisan_api::CollectionRange<T> = serde_json::from_slice(&bytes)?;
                anyhow::ensure!(
                    range.source == expected.source
                        && range.total == expected.count
                        && range.start == offset
                        && range.end >= offset
                        && range.end <= expected.count
                        && range.end - offset == range.records.len() as u64
                        && range.records.len() <= limit as usize
                        && (range.end > offset || offset == expected.count),
                    "Forge returned an inconsistent canonical collection range"
                );
                Ok(range)
            },
        )
        .await
    }

    /// A bounded immutable semantic topology page; absent offset selects the tail.
    #[expect(
        clippy::suspicious_operation_groupings,
        reason = "leaf offsets are checked against the source's byte count on purpose"
    )]
    pub async fn source_index_range(
        &self,
        source: &artisan_api::ContentRef,
        format: artisan_api::SourceIndexFormat,
        offset: Option<u64>,
        limit: u32,
    ) -> Result<artisan_api::SourceIndexRange> {
        static INDEX_REQUESTS: std::sync::OnceLock<std::sync::Arc<tokio::sync::Semaphore>> =
            std::sync::OnceLock::new();
        anyhow::ensure!(
            source.id.is_valid() && (1..=artisan_api::SOURCE_INDEX_PAGE_LEAVES).contains(&limit),
            "invalid semantic source query"
        );
        let admission = std::sync::Arc::clone(
            INDEX_REQUESTS.get_or_init(|| std::sync::Arc::new(tokio::sync::Semaphore::new(4))),
        );
        let expected = source.clone();
        self.call_decoded_limited(
            self.http
                .get(self.url(&format!("/content/{}/index", source.id)))
                .query(&artisan_api::SourceIndexQuery {
                    format,
                    offset,
                    limit: Some(limit),
                }),
            65536,
            Some(admission),
            move |bytes| {
                let range: artisan_api::SourceIndexRange = serde_json::from_slice(&bytes)?;
                anyhow::ensure!(
                    range.index.source == expected
                        && range.index.format == format
                        && range.index.version == artisan_api::SOURCE_INDEX_VERSION
                        && range.index.metadata.id.is_valid()
                        && range.start <= range.end
                        && range.end <= range.index.leaves
                        && range.end - range.start == range.leaves.len() as u64
                        && range.leaves.len() <= limit as usize
                        && offset
                            .is_none_or(|offset| range.start == offset.min(range.index.leaves))
                        && (offset.is_some() || range.end == range.index.leaves),
                    "inconsistent semantic source header"
                );
                for (index, leaf) in range.leaves.iter().enumerate() {
                    anyhow::ensure!(
                        leaf.ordinal == range.start + index as u64
                            && leaf.source_start <= leaf.source_end
                            && leaf.source_end <= expected.bytes
                            && leaf.metadata_start < leaf.metadata_end
                            && leaf.metadata_end <= range.index.metadata.bytes,
                        "inconsistent semantic leaf descriptor"
                    );
                }
                Ok(range)
            },
        )
        .await
    }

    /// Explicit raw-source copy. Native clipboard APIs require one final String,
    /// so reject beyond the caller's budget and the application ceiling, never truncate.
    pub async fn copy_content(
        &self,
        source: &artisan_api::ContentRef,
        limit: usize,
    ) -> Result<String> {
        anyhow::ensure!(
            source.bytes <= limit.min(32 * 1024 * 1024) as u64,
            "Source is too large for the clipboard. Export it to a file instead."
        );
        let mut text = String::with_capacity(usize::try_from(source.bytes)?);
        let mut offset = 0;
        while offset < source.bytes {
            let range = self
                .content_range(source, offset, artisan_api::CONTENT_RANGE_BYTES)
                .await?;
            anyhow::ensure!(range.start == offset, "noncontiguous source export");
            offset = range.end;
            text.push_str(&range.text);
        }
        Ok(text)
    }

    /// User-chosen file export streams bounded ranges into a standard temporary
    /// file beside the destination, publishing it only after the complete source.
    pub async fn export_content(
        &self,
        source: &artisan_api::ContentRef,
        path: std::path::PathBuf,
    ) -> Result<()> {
        let parent = path
            .parent()
            .context("export destination has no parent")?
            .to_owned();
        let mut file = runtime()
            .spawn_blocking(move || tempfile::NamedTempFile::new_in(parent))
            .await??;
        let mut offset = 0;
        while offset < source.bytes {
            let range = self
                .content_range(source, offset, artisan_api::CONTENT_RANGE_BYTES)
                .await?;
            anyhow::ensure!(range.start == offset, "noncontiguous source export");
            offset = range.end;
            file = runtime()
                .spawn_blocking(move || -> Result<_> {
                    use std::io::Write as _;
                    file.write_all(range.text.as_bytes())?;
                    Ok(file)
                })
                .await??;
        }
        runtime()
            .spawn_blocking(move || -> Result<()> {
                use std::io::Write as _;
                file.flush()?;
                file.persist(path).map_err(|error| error.error)?;
                Ok(())
            })
            .await??;
        Ok(())
    }

    pub async fn item(&self, id: &ItemId) -> Result<Item> {
        self.call(self.http.get(self.url(&format!("/items/{id}"))))
            .await
    }

    pub async fn settle_thread(
        &self,
        id: &ThreadId,
        observed_seq: artisan_api::Seq,
    ) -> Result<Thread> {
        self.call(self.json(
            reqwest::Method::POST,
            &format!("/threads/{id}/settle"),
            &artisan_api::SettleThread { observed_seq },
        ))
        .await
    }

    pub async fn mark_unread(&self, id: &ThreadId) -> Result<Thread> {
        self.call(self.http.post(self.url(&format!("/threads/{id}/unread"))))
            .await
    }

    pub async fn cancel(&self, thread: &ThreadId) -> Result<()> {
        self.call_empty(
            self.http
                .post(self.url(&format!("/threads/{thread}/cancel"))),
        )
        .await
    }

    /// Carries on the thread's stopped turn with no new message.
    pub async fn continue_turn(&self, thread: &ThreadId) -> Result<()> {
        self.call_empty(
            self.http
                .post(self.url(&format!("/threads/{thread}/continue"))),
        )
        .await
    }

    /// Clears the thread's goal: the engine stops working toward it.
    pub async fn clear_goal(&self, thread: &ThreadId) -> Result<()> {
        self.call_empty(
            self.http
                .post(self.url(&format!("/threads/{thread}/goal/clear"))),
        )
        .await
    }

    /// Answers a call Forge made for an agent ([`artisan_api::EditorCall`]).
    pub async fn reply_editor_call(
        &self,
        id: &str,
        reply: &artisan_api::EditorReply,
    ) -> Result<()> {
        self.call_empty(self.json(reqwest::Method::POST, &format!("/editor/calls/{id}"), reply))
            .await
    }

    pub async fn answer(&self, item: &ItemId, answer: &Answer) -> Result<()> {
        self.call_empty(self.json(
            reqwest::Method::POST,
            &format!("/items/{item}/answer"),
            answer,
        ))
        .await
    }

    pub async fn draft(&self, scope: &str) -> Result<artisan_api::DraftSnapshot> {
        self.call(self.http.get(self.url(&format!("/drafts/{scope}"))))
            .await
    }

    pub async fn set_draft(
        &self,
        scope: &str,
        value: &artisan_api::WriteDraft,
    ) -> Result<artisan_api::DraftWriteResult> {
        self.call(self.json(reqwest::Method::PUT, &format!("/drafts/{scope}"), value))
            .await
    }

    /// Uploads WebP bytes (see `editor-composer` for the conversion).
    pub async fn put_blob(&self, webp: Vec<u8>) -> Result<BlobRef> {
        self.call(self.http.put(self.url("/blobs")).body(webp))
            .await
    }

    pub async fn blob(&self, hash: &BlobHash) -> Result<Vec<u8>> {
        self.call_bytes(self.http.get(self.url(&format!("/blobs/{hash}"))))
            .await
    }

    pub async fn engines(&self) -> Result<Vec<EngineStatus>> {
        self.call(self.http.get(self.url("/engines"))).await
    }

    /// Sends one typed engine command, including its required payload.
    pub async fn engine_action(
        &self,
        engine: EngineId,
        command: &artisan_api::EngineCommand,
    ) -> Result<EngineStatus> {
        self.call(self.json(
            reqwest::Method::POST,
            &format!("/engines/{engine}/commands"),
            command,
        ))
        .await
    }

    /// The project's icon blob, if its folder has one.
    pub async fn project_icon(&self, project: &ProjectId) -> Result<Option<BlobRef>> {
        self.call(
            self.http
                .get(self.url(&format!("/projects/{project}/icon"))),
        )
        .await
    }

    /// The project's git branch and remote page, read now.
    pub async fn project_repository(
        &self,
        project: &ProjectId,
    ) -> Result<Option<artisan_api::ProjectRepository>> {
        self.call(
            self.http
                .get(self.url(&format!("/projects/{project}/repository"))),
        )
        .await
    }

    pub async fn skills(&self) -> Result<Vec<artisan_api::Skill>> {
        self.call(self.http.get(self.url("/skills"))).await
    }

    /// Every skill with its origin and where it stands in each engine.
    pub async fn skill_catalog(&self) -> Result<Vec<artisan_api::CatalogSkill>> {
        self.call(self.http.get(self.url("/skills/catalog"))).await
    }

    /// `enable`, `disable` or `adopt`; answers the catalog after the change.
    pub async fn skill_action(
        &self,
        id: &str,
        action: artisan_api::SkillAction,
    ) -> Result<Vec<artisan_api::CatalogSkill>> {
        self.call(
            self.http
                .post(self.url(&format!("/skills/{id}/{}", action.as_str()))),
        )
        .await
    }

    /// skills.sh's skills for `query`, through Forge.
    pub async fn marketplace_search(&self, query: &str) -> Result<Vec<artisan_api::Listing>> {
        self.call(
            self.http
                .get(self.url("/marketplace/search"))
                .query(&[("q", query)]),
        )
        .await
    }

    /// Installs a listed skill; answers the catalog after.
    pub async fn marketplace_install(&self, id: &str) -> Result<Vec<artisan_api::CatalogSkill>> {
        let install = artisan_api::InstallListing { id: id.to_owned() };
        self.call(self.json(reqwest::Method::POST, "/marketplace/install", &install))
            .await
    }

    /// The global prompt with its version and where it stands per engine.
    pub async fn global_prompt(&self) -> Result<artisan_api::GlobalPrompt> {
        self.call(self.http.get(self.url("/prompt"))).await
    }

    pub async fn save_global_prompt(
        &self,
        save: &artisan_api::SavePrompt,
    ) -> Result<artisan_api::GlobalPrompt> {
        self.call(self.json(reqwest::Method::PUT, "/prompt", save))
            .await
    }

    pub async fn resolve_global_prompt(
        &self,
        resolution: &artisan_api::ResolvePrompt,
    ) -> Result<artisan_api::GlobalPrompt> {
        self.call(self.json(reqwest::Method::POST, "/prompt/resolve", resolution))
            .await
    }

    pub async fn list_dir(&self, path: Option<&str>) -> Result<DirListing> {
        let mut request = self.http.get(self.url("/fs/list"));
        if let Some(path) = path {
            request = request.query(&[("path", path)]);
        }
        self.call(request).await
    }

    pub async fn pair(&self, request: &PairRequest) -> Result<PairResponse> {
        self.call(self.json(reqwest::Method::POST, "/pair", request))
            .await
    }
}

/// Split a built HTTP request into its tunneled parts: uppercased method,
/// origin-form path plus the exact query string `reqwest` encoded, and a
/// JSON or `$bytes`-enveloped body. JSON content travels as JSON; anything
/// else (e.g. a `PUT /blobs` WebP upload) travels as a `$bytes` envelope
/// carrying its content type, which the Forge decodes back to bytes.
fn relay_parts(
    request: reqwest::RequestBuilder,
) -> Result<(String, String, Option<serde_json::Value>)> {
    let built = request.build().context("build the Forge request")?;
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
        .map(|body| {
            body.as_bytes()
                .context("streaming request bodies cannot travel the Relay; buffer them first")
        })
        .transpose()?
        .map(<[u8]>::to_vec);
    let body = match raw {
        None => None,
        Some(bytes) if bytes.is_empty() => None,
        Some(bytes) => {
            if content_type.starts_with("application/json") {
                Some(serde_json::from_slice(&bytes).context("encode the tunneled call")?)
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
    /// Open a Relay channel to one Forge and return the host inventory
    /// entry plus a connected, grant-refreshing client. Consumed by the
    /// account controller, which puts the client into the existing host
    /// inventory via `connect_with_client` (no relay-only persistence).
    ///
    /// Runtime-safe from any executor: the dial hops onto the network
    /// runtime (WS connect, timers, and tungstenite need its reactor);
    /// dropping the returned future aborts the dial.
    pub async fn via_relay(
        request: crate::services::RelayDialRequest,
        ctx: &artisan_account::relay_client::RelayDialContext,
    ) -> Result<Self> {
        let ctx = ctx.clone();
        AbortOnDropHandle::new(
            runtime().spawn(async move { Self::via_relay_inner(request, &ctx).await }),
        )
        .await
        .context("dial the Relay")
        .and_then(std::convert::identity)
    }

    async fn via_relay_inner(
        request: crate::services::RelayDialRequest,
        ctx: &artisan_account::relay_client::RelayDialContext,
    ) -> Result<Self> {
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
        .await?;
        // Verify the route is live before handing the client to inventory,
        // including the Forge protocol version.
        let live: serde_json::Value = relay.request_json("GET", "/state", None).await?;
        let protocol = live.get("protocol").and_then(serde_json::Value::as_u64);
        if protocol != Some(u64::from(artisan_api::PROTOCOL_VERSION)) {
            anyhow::bail!("Forge speaks an incompatible protocol; update Forge or the Editor");
        }
        Ok(Self {
            http: reqwest::Client::new(),
            transport: Transport::Relay(Arc::new(relay)),
        })
    }
}

pub async fn check(response: reqwest::Response) -> Result<reqwest::Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let text = String::from_utf8_lossy(&read_body(response, 64 * 1024).await.unwrap_or_default())
        .into_owned();
    let mut error = serde_json::from_str::<ApiError>(&text).unwrap_or_else(|_| ApiError {
        code: artisan_api::ErrorCode::Internal,
        message: format!("Forge answered {status}: {}", text.trim()),
    });
    if matches!(status.as_u16(), 401 | 403) {
        error.code = artisan_api::ErrorCode::Unauthorized;
    }
    Err(anyhow!(error))
}

fn decode_state(bytes: &[u8]) -> Result<State> {
    // Probe compatibility before reading DTO fields that changed between protocols.
    #[derive(Deserialize)]
    struct Version {
        protocol: u32,
        forge_version: String,
    }
    let version: Version = serde_json::from_slice(bytes).context("read Forge protocol")?;
    if version.protocol != artisan_api::PROTOCOL_VERSION {
        return Err(anyhow!(ApiError {
            code: artisan_api::ErrorCode::VersionMismatch,
            message: format!(
                "Forge {} speaks protocol {}; update Forge or the Editor",
                version.forge_version, version.protocol
            ),
        }));
    }
    serde_json::from_slice(bytes).context("read Forge state")
}

fn is_loopback(address: &str) -> bool {
    let host = address.rsplit_once(':').map_or(address, |(host, _)| host);
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

pub async fn read_body(response: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    use futures::StreamExt as _;
    anyhow::ensure!(
        response
            .content_length()
            .is_none_or(|size| size <= limit as u64),
        "HTTP response exceeds {limit} bytes"
    );
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("read HTTP response")?;
        anyhow::ensure!(
            bytes.len().saturating_add(chunk.len()) <= limit,
            "HTTP response exceeds {limit} bytes"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn source_ranges() -> &'static std::sync::Arc<tokio::sync::Semaphore> {
    static RANGES: std::sync::OnceLock<std::sync::Arc<tokio::sync::Semaphore>> =
        std::sync::OnceLock::new();
    RANGES.get_or_init(|| std::sync::Arc::new(tokio::sync::Semaphore::new(4)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_request_builders_preserve_paths_queries_and_bodies() {
        let client = ForgeClient {
            http: reqwest::Client::new(),
            transport: Transport::Direct("https://forge.test".to_owned()),
        };
        let (method, path, body) = relay_parts(client.http.get(client.url("/state"))).unwrap();
        assert_eq!(method, "GET");
        assert_eq!(path, "/state");
        assert!(body.is_none());
        let (method, path, body) = relay_parts(
            client
                .http
                .post(client.url("/threads?since=7"))
                .json(&serde_json::json!({"title": "hello"})),
        )
        .unwrap();
        assert_eq!(method, "POST");
        assert_eq!(path, "/threads?since=7");
        assert_eq!(body, Some(serde_json::json!({"title": "hello"})));
    }

    #[gpui::test]
    fn source_admission_and_release_progress_without_polling_the_ui_again(
        cx: &gpui::TestAppContext,
    ) {
        use std::io::{Read as _, Write as _};
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        use std::time::{Duration, Instant};
        // This fixture deliberately uses actual network/decoder threads.
        cx.executor().allow_parking();

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let server = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(4);
            loop {
                match listener.accept() {
                    Ok((mut socket, _)) => {
                        socket
                            .set_read_timeout(Some(Duration::from_secs(2)))
                            .unwrap();
                        let mut request = [0; 1024];
                        assert!(socket.read(&mut request).unwrap() > 0);
                        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]").unwrap();
                        return;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "owned request did not reach its test server"
                        );
                        std::thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("test server accept: {error}"),
                }
            }
        });
        let admission = Arc::new(tokio::sync::Semaphore::new(1));
        let held = Arc::clone(&admission).try_acquire_owned().unwrap();
        let decoded = Arc::new(AtomicBool::new(false));
        let decoded_by_request = Arc::clone(&decoded);
        let admitted = Arc::clone(&admission);
        let http = reqwest::Client::new();
        let client = ForgeClient {
            http: http.clone(),
            transport: Transport::Direct(format!("http://{address}")),
        };
        let request = http.get(client.url("/owned-admission"));
        let task = cx.spawn(async move |_| {
            client
                .call_decoded_limited(request, 16, Some(admitted), move |bytes| {
                    assert_eq!(bytes, b"[]");
                    decoded_by_request.store(true, Ordering::Release);
                    Ok(())
                })
                .await
                .unwrap();
        });
        cx.run_until_parked();
        drop(held);
        // Deliberately do not drive GPUI. A queued reservation must be acquired,
        // used and released by the network owner, even while its view is idle.
        let deadline = Instant::now() + Duration::from_secs(4);
        while !decoded.load(Ordering::Acquire) || admission.available_permits() != 1 {
            assert!(
                Instant::now() < deadline,
                "source admission depended on another UI poll"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        server.join().unwrap();
        cx.run_until_parked();
        drop(task);
    }
    #[test]
    fn old_state_shapes_report_protocol_mismatch_before_dto_errors() {
        let bytes =
            br#"{"protocol":0,"forge_version":"old","threads":"an obsolete representation"}"#
                .to_vec();
        let error = decode_state(&bytes).unwrap_err();
        assert_eq!(
            error.downcast_ref::<ApiError>().unwrap().code,
            artisan_api::ErrorCode::VersionMismatch
        );
    }
    #[test]
    fn matching_protocol_still_validates_the_complete_snapshot() {
        let bytes = format!(
            "{{\"protocol\":{},\"forge_version\":\"current\",\"threads\":\"invalid\"}}",
            artisan_api::PROTOCOL_VERSION
        )
        .into_bytes();
        let error = decode_state(&bytes).unwrap_err();
        assert!(error.downcast_ref::<serde_json::Error>().is_some());
    }

    /// The Relay dial hops onto the network runtime itself, so a GPUI test
    /// executor with no Tokio reactor sees a refused connection, not a panic.
    #[gpui::test]
    #[expect(
        clippy::future_not_send,
        reason = "gpui TestAppContext is !Send; the test executor polls this future"
    )]
    async fn relay_dial_without_tokio_reactor_fails_with_connection_refused(
        cx: &mut gpui::TestAppContext,
    ) {
        use base64::Engine as _;
        cx.executor().allow_parking();
        let closed = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let ctx = artisan_account::relay_client::RelayDialContext {
            relay_base_url: format!("ws://127.0.0.1:{closed}"),
            core: artisan_account::CoreSessionClient::new("http://127.0.0.1:1").unwrap(),
            session: artisan_account::SessionToken::new("test-session".to_owned()),
        };
        let request = crate::services::RelayDialRequest {
            account_id: "acct_00000000000000000000000000".to_owned(),
            machine_id: "mch_11111111111111111111111111".to_owned(),
            forge_ed25519_b64u: base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode([0x42_u8; 32]),
            grant_jwt: "bogus.grant.jwt".to_owned(),
            cnf_secret: [0x77_u8; 32],
        };
        let text = match ForgeClient::via_relay(request, &ctx).await {
            Ok(_) => panic!("dial to a closed port must fail"),
            Err(error) => format!("{error:#}"),
        };
        assert!(text.contains("connect to the Relay"), "got: {text}");
        assert!(!text.to_lowercase().contains("reactor"), "got: {text}");
    }
}
