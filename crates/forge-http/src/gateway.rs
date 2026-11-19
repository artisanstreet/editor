use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use forge_computer::ComputerMcp;
use forge_mcp::{ArtisanMcp, ToolSet};
use forge_runs::Hub;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

#[derive(Debug, thiserror::Error)]
pub enum StartGatewayError {
    #[error("listen for the MCP gateway on loopback")]
    Bind(#[source] std::io::Error),
    #[error("read the MCP gateway's address")]
    Address(#[source] std::io::Error),
}

pub(crate) async fn start(
    hub: &Hub,
    shutdown: CancellationToken,
    tasks: &TaskTracker,
) -> Result<(), StartGatewayError> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(StartGatewayError::Bind)?;
    let address = listener.local_addr().map_err(StartGatewayError::Address)?;
    let computer = ComputerMcp::for_host(hub);
    hub.set_gateway(format!("http://{address}/mcp/"), computer.is_some());
    let artisan = Served {
        hub: hub.clone(),
        set: ArtisanMcp::new(hub.clone()),
    };
    let mut app = Router::new()
        .route(
            "/mcp/artisan/{token}",
            post(serve::<ArtisanMcp>).get(no_stream).delete(no_stream),
        )
        .with_state(artisan);
    if let Some(set) = computer {
        app = app.merge(
            Router::new()
                .route(
                    "/mcp/computer/{token}",
                    post(serve::<ComputerMcp>).get(no_stream).delete(no_stream),
                )
                .with_state(Served {
                    hub: hub.clone(),
                    set,
                }),
        );
    }
    tasks.spawn(async move {
        if let Err(error) = axum::serve(listener, app)
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
        {
            tracing::error!(%address, "MCP gateway stopped: {error}");
        }
    });
    tracing::info!(%address, "MCP gateway is serving");
    Ok(())
}

#[derive(Clone)]
struct Served<T> {
    hub: Hub,
    set: T,
}

async fn serve<T: ToolSet + Clone + 'static>(
    State(served): State<Served<T>>,
    Path(token): Path<String>,
    body: Bytes,
) -> Response {
    let Some(caller) = served.hub.gateway_thread(&token) else {
        return (
            StatusCode::NOT_FOUND,
            "This MCP address is no longer valid.",
        )
            .into_response();
    };
    let body: Value = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(error) => {
            let reply = json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": { "code": -32700, "message": format!("That isn't JSON: {error}") },
            });
            return (StatusCode::BAD_REQUEST, axum::Json(reply)).into_response();
        }
    };
    forge_mcp::server::handle(&served.set, &caller, body)
        .await
        .map_or_else(
            || StatusCode::ACCEPTED.into_response(),
            |reply| axum::Json(reply).into_response(),
        )
}

async fn no_stream() -> StatusCode {
    StatusCode::METHOD_NOT_ALLOWED
}
