//! Plain HTTPS calls to one Forge. Every method runs on the network runtime,
//! so it can be awaited from GPUI's executor.

use crate::runtime::runtime;
use anyhow::{Context, Result, anyhow};
use artisan_api::tls::Identity;
use artisan_api::{
    Answer, ApiError, BlobHash, BlobRef, CreateProject, CreateThread, DirListing, EngineId,
    EngineStatus, Item, ItemId, ItemsPage, ItemsQuery, LinkTitle, LinkTitleQuery, PairRequest,
    PairResponse, Project, ProjectId, SendMessage, State, Thread, ThreadId, UpdateThread,
};
use serde::Serialize;
use serde::de::DeserializeOwned;

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
        runtime()
            .spawn(async move {
                let response = request.send().await.context("reach Forge")?;
                read(response).await
            })
            .await
            .context("network task")?
    }

    async fn call_empty(&self, request: reqwest::RequestBuilder) -> Result<()> {
        runtime()
            .spawn(async move {
                let response = request.send().await.context("reach Forge")?;
                check(response).await.map(drop)
            })
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
        self.call(self.http.get(self.url("/state"))).await
    }

    pub async fn items(&self, thread: &ThreadId, query: &ItemsQuery) -> Result<ItemsPage> {
        self.call(
            self.http
                .get(self.url(&format!("/threads/{thread}/items")))
                .query(query),
        )
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

    pub async fn set_setting(&self, key: &str, value: &serde_json::Value) -> Result<()> {
        self.call_empty(self.json(reqwest::Method::PUT, &format!("/settings/{key}"), value))
            .await
    }

    pub async fn draft(&self, scope: &str) -> Result<serde_json::Value> {
        self.call(self.http.get(self.url(&format!("/drafts/{scope}"))))
            .await
    }

    pub async fn set_draft(&self, scope: &str, value: &serde_json::Value) -> Result<()> {
        self.call_empty(self.json(reqwest::Method::PUT, &format!("/drafts/{scope}"), value))
            .await
    }

    /// Uploads WebP bytes (see `editor-composer` for the conversion).
    pub async fn put_blob(&self, webp: Vec<u8>) -> Result<BlobRef> {
        self.call(self.http.put(self.url("/blobs")).body(webp))
            .await
    }

    pub async fn blob(&self, hash: &BlobHash) -> Result<Vec<u8>> {
        let request = self.http.get(self.url(&format!("/blobs/{hash}")));
        runtime()
            .spawn(async move {
                let response = check(request.send().await.context("reach Forge")?).await?;
                Ok(response.bytes().await.context("read blob")?.to_vec())
            })
            .await
            .context("network task")?
    }

    pub async fn engines(&self) -> Result<Vec<EngineStatus>> {
        self.call(self.http.get(self.url("/engines"))).await
    }

    /// `install`, `update`, `rollback`, `use` (with version), `login`,
    /// `login-code` (with code), `cancel-login`, `logout`, `refresh`,
    /// `versions` (the published versions, into the engine's status).
    pub async fn engine_action(
        &self,
        engine: EngineId,
        action: &str,
        body: serde_json::Value,
    ) -> Result<EngineStatus> {
        self.call(self.json(
            reqwest::Method::POST,
            &format!("/engines/{engine}/{action}"),
            &body,
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
        action: &str,
    ) -> Result<Vec<artisan_api::CatalogSkill>> {
        self.call(self.http.post(self.url(&format!("/skills/{id}/{action}"))))
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

    /// The page title of an `http(s)` link, resolved and cached by Forge.
    pub async fn link_title(&self, url: &str) -> Result<LinkTitle> {
        let query = LinkTitleQuery {
            url: url.to_owned(),
        };
        self.call(self.http.get(self.url("/links/title")).query(&query))
            .await
    }

    pub async fn pair(&self, request: &PairRequest) -> Result<PairResponse> {
        self.call(self.json(reqwest::Method::POST, "/pair", request))
            .await
    }
}

async fn check(response: reqwest::Response) -> Result<reqwest::Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    Err(match serde_json::from_str::<ApiError>(&text) {
        Ok(error) => anyhow!(error),
        Err(_) => anyhow!("Forge answered {status}: {}", text.trim()),
    })
}

async fn read<T: DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    let response = check(response).await?;
    response.json().await.context("read Forge's answer")
}
