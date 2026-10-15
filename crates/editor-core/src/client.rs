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
use tokio_util::task::AbortOnDropHandle;

#[derive(Clone)]
pub struct ForgeClient {
    http: reqwest::Client,
    base: String,
}

impl ForgeClient {
    /// A client for the Forge at `address` (`host:port`) whose certificate
    /// has `fingerprint`, presenting this device's identity.
    pub fn new(address: &str, fingerprint: &str, identity: &Identity) -> Result<Self> {
        let tls = artisan_api::tls::client_config(identity, fingerprint)?;
        let http = reqwest::Client::builder()
            .use_preconfigured_tls(tls)
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            .context("build the HTTP client")?;
        Ok(Self {
            http,
            base: format!("https://{address}"),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    pub(crate) fn raw(&self) -> &reqwest::Client {
        &self.http
    }

    pub(crate) fn events_url(&self, since: artisan_api::Seq) -> String {
        self.url(&format!("/events?since={since}"))
    }

    async fn call<T: DeserializeOwned + Send + 'static>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T> {
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

    async fn call_empty(&self, request: reqwest::RequestBuilder) -> Result<()> {
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

    fn json<B: Serialize>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: &B,
    ) -> reqwest::RequestBuilder {
        self.http.request(method, self.url(path)).json(body)
    }

    pub async fn state(&self) -> Result<State> {
        self.call_decoded(self.http.get(self.url("/state")), decode_state)
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
        let request = self
            .http
            .get(self.url(&format!("/threads/{thread}/resource")))
            .query(&[
                ("path", path),
                ("from_home", if from_home { "true" } else { "false" }),
            ]);
        let task =
            tokio_util::task::AbortOnDropHandle::new(crate::runtime::runtime().spawn(async move {
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
                        bytes.len() + chunk.len() <= 32 * 1024 * 1024,
                        "resource exceeds 32 MiB"
                    );
                    bytes.extend_from_slice(&chunk);
                }
                Ok(bytes)
            }));
        task.await.context("resource task")?
    }

    pub async fn mutate_mcp(
        &self,
        mutation: &artisan_api::McpMutation,
    ) -> Result<Vec<artisan_api::McpServer>> {
        self.call(self.json(reqwest::Method::PATCH, "/mcp", mutation))
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
            Some(source_ranges().clone()),
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
            Some(source_ranges().clone()),
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
    pub async fn source_index_range(
        &self,
        source: &artisan_api::ContentRef,
        format: artisan_api::SourceIndexFormat,
        offset: Option<u64>,
        limit: u32,
    ) -> Result<artisan_api::SourceIndexRange> {
        anyhow::ensure!(
            source.id.is_valid() && (1..=artisan_api::SOURCE_INDEX_PAGE_LEAVES).contains(&limit),
            "invalid semantic source query"
        );
        static INDEX_REQUESTS: std::sync::OnceLock<std::sync::Arc<tokio::sync::Semaphore>> =
            std::sync::OnceLock::new();
        let admission = INDEX_REQUESTS
            .get_or_init(|| std::sync::Arc::new(tokio::sync::Semaphore::new(4)))
            .clone();
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
        let request = self.http.get(self.url(&format!("/blobs/{hash}")));
        AbortOnDropHandle::new(runtime().spawn(async move {
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
            use futures::StreamExt as _;
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.context("read blob")?;
                anyhow::ensure!(
                    bytes.len().saturating_add(chunk.len()) <= 32 * 1024 * 1024,
                    "image exceeds 32 MiB"
                );
                bytes.extend_from_slice(&chunk);
            }
            Ok(bytes)
        }))
        .await
        .context("network task")?
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

pub(crate) async fn check(response: reqwest::Response) -> Result<reqwest::Response> {
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

fn decode_state(bytes: Vec<u8>) -> Result<State> {
    // Probe compatibility before reading DTO fields that changed between protocols.
    #[derive(Deserialize)]
    struct Version {
        protocol: u32,
        forge_version: String,
    }
    let version: Version = serde_json::from_slice(&bytes).context("read Forge protocol")?;
    if version.protocol != artisan_api::PROTOCOL_VERSION {
        return Err(anyhow!(ApiError {
            code: artisan_api::ErrorCode::VersionMismatch,
            message: format!(
                "Forge {} speaks protocol {}; update Forge or the Editor",
                version.forge_version, version.protocol
            ),
        }));
    }
    serde_json::from_slice(&bytes).context("read Forge state")
}

pub(crate) async fn read_body(response: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[gpui::test]
    fn source_admission_and_release_progress_without_polling_the_ui_again(
        cx: &mut gpui::TestAppContext,
    ) {
        // This fixture deliberately uses actual network/decoder threads.
        cx.executor().allow_parking();
        use std::io::{Read as _, Write as _};
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };
        use std::time::{Duration, Instant};
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
        let held = admission.clone().try_acquire_owned().unwrap();
        let decoded = Arc::new(AtomicBool::new(false));
        let decoded_by_request = decoded.clone();
        let admitted = admission.clone();
        let http = reqwest::Client::new();
        let client = ForgeClient {
            http: http.clone(),
            base: format!("http://{address}"),
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
        let error = decode_state(bytes).unwrap_err();
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
        let error = decode_state(bytes).unwrap_err();
        assert!(error.downcast_ref::<serde_json::Error>().is_some());
    }
}

fn source_ranges() -> &'static std::sync::Arc<tokio::sync::Semaphore> {
    static RANGES: std::sync::OnceLock<std::sync::Arc<tokio::sync::Semaphore>> =
        std::sync::OnceLock::new();
    RANGES.get_or_init(|| std::sync::Arc::new(tokio::sync::Semaphore::new(4)))
}
