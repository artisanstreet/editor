#![cfg(target_os = "linux")]

use axum::Router;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use forge_http::upgrade::{Gateway, Ready};
use forge_runs::runtime::{Member, PROTOCOL};
use serde_json::Value;
use std::path::Path;

const TOKEN: &str = "gateway-token";

#[derive(Clone, Copy)]
enum Backend {
    Healthy,
    Unhealthy,
}

fn gateway(control: String) -> Gateway {
    let address = "127.0.0.1:9".parse().expect("fixed socket address");
    Gateway {
        id: "gateway-1".into(),
        pid: std::process::id(),
        control: control.clone(),
        token: TOKEN.into(),
        address,
        binary: std::env::current_exe().expect("the test binary path"),
        protocol: PROTOCOL,
        backend: Member {
            id: "generation-1".into(),
            control,
            token: "backend-token".into(),
            address,
            pid: std::process::id(),
            protocol: PROTOCOL,
            schema: 1,
        },
    }
}

async fn serve_control(backend: Backend) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind the fake control listener");
    let control = format!(
        "http://{}",
        listener.local_addr().expect("the fake control address")
    );
    let reply = gateway(control.clone());
    let app = Router::new().route(
        "/health",
        get(move |headers: HeaderMap| async move {
            let authorized = headers
                .get("authorization")
                .is_some_and(|value| value.as_bytes() == format!("Bearer {TOKEN}").as_bytes());
            let response: Response = match (authorized, backend) {
                (false, _) => StatusCode::UNAUTHORIZED.into_response(),
                (true, Backend::Healthy) => axum::Json(Ready { gateway: reply }).into_response(),
                (true, Backend::Unhealthy) => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "backend schema 1 does not match the gateway's schema 2",
                )
                    .into_response(),
            };
            response
        }),
    );
    tokio::spawn(async move { axum::serve(listener, app).await });
    control
}

fn install_running_forge(home: &Path, control: String) {
    let forge = home.join("data/artisan-street/forge");
    let data = forge.join("data");
    std::fs::create_dir_all(data.join("runtime")).expect("create Forge's runtime folder");
    std::fs::write(forge.join("forge"), b"").expect("write the Forge binary");
    std::fs::write(forge.join("version"), "1.0.0\n").expect("write the Forge version");
    std::fs::write(data.join("forge.pid"), std::process::id().to_string())
        .expect("write the Forge pid");
    forge_http::upgrade::write(&data.join("runtime/gateway.json"), &gateway(control))
        .expect("write the gateway record");
}

async fn status(backend: Backend) -> Value {
    let dir = tempfile::tempdir().expect("create the isolated home");
    let control = serve_control(backend).await;
    install_running_forge(dir.path(), control);
    let home = dir.path().to_owned();
    let output = tokio::task::spawn_blocking(move || {
        std::process::Command::new(env!("CARGO_BIN_EXE_ae"))
            .args(["status", "--json"])
            .env("HOME", home.join("home"))
            .env("XDG_DATA_HOME", home.join("data"))
            .env("XDG_CONFIG_HOME", home.join("config"))
            .output()
            .expect("run ae status")
    })
    .await
    .expect("join ae status");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("ae status prints JSON")
}

fn health_problems(report: &Value) -> Vec<String> {
    report["problems"]
        .as_array()
        .expect("problems is a list")
        .iter()
        .map(|problem| problem.as_str().expect("each problem is text").to_owned())
        .filter(|problem| problem.contains("healthy"))
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn status_reports_an_unhealthy_forge_with_the_reason() {
    let report = status(Backend::Unhealthy).await;
    assert_eq!(report["forge"]["running"], true);
    assert_eq!(report["forge"]["healthy"], false);
    let problems = health_problems(&report);
    assert_eq!(problems.len(), 1, "{report}");
    assert!(
        problems[0].contains("backend schema 1 does not match the gateway's schema 2"),
        "{}",
        problems[0]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn status_reports_a_healthy_forge_without_problems() {
    let report = status(Backend::Healthy).await;
    assert_eq!(report["forge"]["healthy"], true);
    assert!(health_problems(&report).is_empty(), "{report}");
}

#[test]
fn status_reports_a_pid_file_it_cannot_read_instead_of_a_stopped_forge() {
    let dir = tempfile::tempdir().expect("create the isolated home");
    let forge = dir.path().join("data/artisan-street/forge");
    let data = forge.join("data");
    std::fs::create_dir_all(&data).expect("create Forge's data folder");
    std::fs::write(forge.join("forge"), b"").expect("write the Forge binary");
    std::fs::write(forge.join("version"), "1.0.0\n").expect("write the Forge version");
    std::fs::write(data.join("forge.pid"), "forge\n").expect("write a damaged pid file");

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ae"))
        .args(["status", "--json"])
        .env("HOME", dir.path().join("home"))
        .env("XDG_DATA_HOME", dir.path().join("data"))
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .output()
        .expect("run ae status");
    let report: Value = serde_json::from_slice(&output.stdout).expect("ae status prints JSON");

    let problems = report["problems"].as_array().expect("problems is a list");
    assert!(
        problems.iter().any(|problem| problem
            .as_str()
            .is_some_and(|problem| problem.contains("\"forge\", not a process id"))),
        "{report}"
    );
}
