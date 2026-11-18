use artisan_api::tls::{Identity, client_config, fingerprint_of};
use artisan_api::{ApiError, PairRequest};
use std::net::SocketAddr;
use std::time::Duration;

pub(crate) struct Editor {
    http: reqwest::Client,
}

#[derive(Debug)]
pub(crate) enum Outcome<T> {
    Accepted(T),
    Refused { status: u16, error: ApiError },
    Lost,
}

impl Editor {
    pub(crate) fn new(forge_certificate: &str) -> Self {
        let identity = Identity::generate("artisan-editor").expect("make the Editor's identity");
        let fingerprint =
            fingerprint_of(forge_certificate).expect("read Forge's certificate fingerprint");
        let tls = client_config(&identity, &fingerprint).expect("configure the Editor's TLS");
        let http = reqwest::Client::builder()
            .use_preconfigured_tls(tls)
            .pool_max_idle_per_host(0)
            .build()
            .expect("build the Editor's HTTP client");
        Self { http }
    }

    pub(crate) async fn pair(&self, address: SocketAddr, code: String) {
        let paired = self
            .post::<serde_json::Value>(
                address,
                "/pair",
                &PairRequest {
                    code,
                    device_name: "simulation".to_owned(),
                },
                None,
            )
            .await;
        assert!(
            matches!(paired, Outcome::Accepted(_)),
            "the Editor pairs: {paired:?}"
        );
    }

    pub(crate) fn url(address: SocketAddr, path: &str) -> String {
        format!("https://localhost:{}{path}", address.port())
    }

    pub(crate) fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub(crate) async fn get<T: serde::de::DeserializeOwned>(
        &self,
        address: SocketAddr,
        path: &str,
    ) -> T {
        let response = self
            .http
            .get(Self::url(address, path))
            .send()
            .await
            .unwrap_or_else(|error| panic!("GET {path}: {error}"));
        let status = response.status();
        assert!(status.is_success(), "GET {path} answered {status}");
        response
            .json()
            .await
            .unwrap_or_else(|error| panic!("decode GET {path}: {error}"))
    }

    pub(crate) async fn post<T: serde::de::DeserializeOwned>(
        &self,
        address: SocketAddr,
        path: &str,
        body: &(impl serde::Serialize + Sync),
        cut_after: Option<Duration>,
    ) -> Outcome<T> {
        self.send(
            self.http.post(Self::url(address, path)).json(body),
            path,
            cut_after,
        )
        .await
    }

    pub(crate) async fn put<T: serde::de::DeserializeOwned>(
        &self,
        address: SocketAddr,
        path: &str,
        body: &(impl serde::Serialize + Sync),
    ) -> Outcome<T> {
        self.send(
            self.http.put(Self::url(address, path)).json(body),
            path,
            None,
        )
        .await
    }

    async fn send<T: serde::de::DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
        path: &str,
        cut_after: Option<Duration>,
    ) -> Outcome<T> {
        let exchange = async {
            let response = request.send().await?;
            let status = response.status();
            let body = response.bytes().await?;
            Ok::<_, reqwest::Error>((status, body))
        };
        let answered = match cut_after {
            None => Some(exchange.await),
            Some(limit) => tokio::time::timeout(limit, exchange).await.ok(),
        };
        let Some(answered) = answered else {
            return Outcome::Lost;
        };
        let (status, body) =
            answered.unwrap_or_else(|error| panic!("{path} reaches a running Forge: {error}"));
        if status.is_success() {
            Outcome::Accepted(
                serde_json::from_slice(&body)
                    .unwrap_or_else(|error| panic!("decode the answer to {path}: {error}")),
            )
        } else {
            let error = serde_json::from_slice(&body).unwrap_or_else(|error| {
                panic!(
                    "{path} failed with {status} but no typed error ({error}): {}",
                    String::from_utf8_lossy(&body)
                )
            });
            Outcome::Refused {
                status: status.as_u16(),
                error,
            }
        }
    }
}
