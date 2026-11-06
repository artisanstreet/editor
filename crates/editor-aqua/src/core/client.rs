use crate::core::runtime::runtime;
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
    Direct(String),
    Relay(Arc<artisan_account::relay_client::RelayClient>),
}

const RELAY_ORIGIN: &str = "https://relay.tunnel";

impl ForgeClient {
    pub fn new(address: &str, fingerprint: &str, identity: &Identity) -> Result<Self> {
        let tls = artisan_api::tls::client_config(identity, fingerprint)?;
        let http = reqwest::Client::builder()
            .use_preconfigured_tls(tls)
            .connect_timeout(std::time::Duration::from_secs(10))
            .tcp_keepalive(std::time::Duration::from_secs(60))
            .pool_idle_timeout(None)
            .zstd(!is_loopback(address))
            .build()
            .context("build the HTTP client")?;
        Ok(Self {
            http,
            transport: Transport::Direct(format!("https://{address}")),
        })
    }

    #[must_use]
    pub const fn is_relay(&self) -> bool {
        matches!(self.transport, Transport::Relay(_))
    }

    #[must_use]
    pub const fn relay(&self) -> Option<&Arc<artisan_account::relay_client::RelayClient>> {
        match &self.transport {
            Transport::Relay(relay) => Some(relay),
            Transport::Direct(_) => None,
        }
    }

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

    pub async fn transcript(&self, thread: &ThreadId, query: &ItemsQuery) -> Result<ItemsPage> {
        self.call(
            self.http
                .get(self.url(&format!("/threads/{thread}/transcript")))
                .query(query),
        )
        .await
    }

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

    pub async fn mcp_sign_in_start(
        &self,
        start: &artisan_api::McpSignInStart,
    ) -> Result<artisan_api::McpSignInStarted> {
        self.call(self.json(reqwest::Method::POST, "/mcp/sign-in/start", start))
            .await
    }

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

    pub async fn continue_turn(&self, thread: &ThreadId) -> Result<()> {
        self.call_empty(
            self.http
                .post(self.url(&format!("/threads/{thread}/continue"))),
        )
        .await
    }

    pub async fn clear_goal(&self, thread: &ThreadId) -> Result<()> {
        self.call_empty(
            self.http
                .post(self.url(&format!("/threads/{thread}/goal/clear"))),
        )
        .await
    }

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

    pub async fn project_icon(&self, project: &ProjectId) -> Result<Option<BlobRef>> {
        self.call(
            self.http
                .get(self.url(&format!("/projects/{project}/icon"))),
        )
        .await
    }

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

    pub async fn skill_catalog(&self) -> Result<Vec<artisan_api::CatalogSkill>> {
        self.call(self.http.get(self.url("/skills/catalog"))).await
    }

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

    pub async fn marketplace_search(&self, query: &str) -> Result<Vec<artisan_api::Listing>> {
        self.call(
            self.http
                .get(self.url("/marketplace/search"))
                .query(&[("q", query)]),
        )
        .await
    }

    pub async fn marketplace_install(&self, id: &str) -> Result<Vec<artisan_api::CatalogSkill>> {
        let install = artisan_api::InstallListing { id: id.to_owned() };
        self.call(self.json(reqwest::Method::POST, "/marketplace/install", &install))
            .await
    }

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
    pub async fn via_relay(
        request: crate::core::services::RelayDialRequest,
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
        request: crate::core::services::RelayDialRequest,
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
