//! Skills and the global prompt: see `artisan_api::extensions` for the
//! routes. A refusal (`forge_runs::Refused`) answers 409 with a message for
//! the user; anything else is an internal error.

use crate::routes::App;
use artisan_api::{ApiError, ErrorCode, ResolvePrompt, SavePrompt};
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

fn answer<T: Serialize>(result: anyhow::Result<T>, step: &str) -> Response {
    match result {
        Ok(value) => Json(value).into_response(),
        Err(error) => {
            let (status, code) = if error.downcast_ref::<forge_runs::Refused>().is_some() {
                (StatusCode::CONFLICT, ErrorCode::Conflict)
            } else {
                tracing::error!(step, "request failed: {error:#}");
                (StatusCode::INTERNAL_SERVER_ERROR, ErrorCode::Internal)
            };
            (
                status,
                Json(ApiError {
                    code,
                    message: format!("{error:#}"),
                }),
            )
                .into_response()
        }
    }
}

/// The skills a `$name` mention can name: Artisan's own and the user's,
/// less the ones turned off.
pub(crate) async fn skills(State(app): State<App>) -> Response {
    answer(app.hub.enabled_skills().await, "list the skills")
}

pub(crate) async fn catalog(State(app): State<App>) -> Response {
    answer(app.hub.skill_catalog().await, "list the skill catalog")
}

/// `enable`, `disable` or `adopt`.
pub(crate) async fn skill_action(
    State(app): State<App>,
    Path((id, action)): Path<(String, String)>,
) -> Response {
    let result = match action.as_str() {
        "enable" => app.hub.set_skill_enabled(&id, true).await,
        "disable" => app.hub.set_skill_enabled(&id, false).await,
        "adopt" => app.hub.adopt_skill(&id).await,
        other => {
            return (
                StatusCode::NOT_FOUND,
                Json(ApiError {
                    code: ErrorCode::NotFound,
                    message: format!("skill action {other:?} not found"),
                }),
            )
                .into_response();
        }
    };
    answer(result, "change a skill")
}

pub(crate) async fn prompt(State(app): State<App>) -> Response {
    answer(app.hub.global_prompt().await, "read the global prompt")
}

pub(crate) async fn save_prompt(State(app): State<App>, Json(save): Json<SavePrompt>) -> Response {
    answer(
        app.hub.save_global_prompt(save).await,
        "save the global prompt",
    )
}

pub(crate) async fn resolve_prompt(
    State(app): State<App>,
    Json(resolution): Json<ResolvePrompt>,
) -> Response {
    answer(
        app.hub.resolve_global_prompt(resolution).await,
        "resolve the global prompt",
    )
}
