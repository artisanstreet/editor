//! The MCP gateway: a plain-HTTP listener on loopback where the engines
//! Forge runs reach Forge's own MCP servers, each at `/mcp/<server>/`. Each
//! thread's address carries its credential, so the caller is known without
//! a session.

use anyhow::{Context, Result};
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

/// Binds the gateway, tells `hub` its address, and serves until `shutdown`.
pub async fn start(hub: &Hub, shutdown: CancellationToken, tasks: &TaskTracker) -> Result<()> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("listen for the MCP gateway on loopback")?;
    let address = listener.local_addr()?;
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

/// One of Forge's servers, with the hub that knows whose credential is whose.
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

/// The gateway answers every request in its response; it opens no stream
/// and keeps no session to end.
async fn no_stream() -> StatusCode {
    StatusCode::METHOD_NOT_ALLOWED
}
