use crate::routes::{App, Result};
use artisan_api::{CatalogSkill, GlobalPrompt, ResolvePrompt, SavePrompt, Skill};
use axum::Json;
use axum::extract::{Path, State};

pub(crate) async fn skills(State(app): State<App>) -> Result<Json<Vec<Skill>>> {
    Ok(Json(app.hub.extensions().enabled_skills().await?))
}

pub(crate) async fn catalog(State(app): State<App>) -> Result<Json<Vec<CatalogSkill>>> {
    Ok(Json(app.hub.extensions().skill_catalog().await?))
}

pub(crate) async fn skill_action(
    State(app): State<App>,
    Path((id, action)): Path<(String, artisan_api::SkillAction)>,
) -> Result<Json<Vec<CatalogSkill>>> {
    let catalog = match action {
        artisan_api::SkillAction::Enable => {
            app.hub.extensions().set_skill_enabled(&id, true).await?
        }
        artisan_api::SkillAction::Disable => {
            app.hub.extensions().set_skill_enabled(&id, false).await?
        }
        artisan_api::SkillAction::Adopt => app.hub.extensions().adopt_skill(&id).await?,
        artisan_api::SkillAction::Discard => app.hub.extensions().discard_skill(&id).await?,
    };
    Ok(Json(catalog))
}

#[derive(serde::Deserialize)]
pub(crate) struct MarketplaceQuery {
    q: String,
}

pub(crate) async fn marketplace_search(
    State(app): State<App>,
    axum::extract::Query(query): axum::extract::Query<MarketplaceQuery>,
) -> Result<Json<Vec<artisan_api::Listing>>> {
    Ok(Json(
        app.hub.extensions().marketplace_search(&query.q).await?,
    ))
}

pub(crate) async fn marketplace_install(
    State(app): State<App>,
    Json(install): Json<artisan_api::InstallListing>,
) -> Result<Json<Vec<CatalogSkill>>> {
    Ok(Json(
        app.hub
            .extensions()
            .marketplace_install(&install.id)
            .await?,
    ))
}

pub(crate) async fn mcp_sign_in_start(
    State(app): State<App>,
    Json(start): Json<artisan_api::McpSignInStart>,
) -> Result<Json<artisan_api::McpSignInStarted>> {
    Ok(Json(
        app.hub
            .mcp()
            .sign_in_start(&start.server, &start.redirect_uri)
            .await?,
    ))
}

pub(crate) async fn mcp_sign_in_finish(
    State(app): State<App>,
    Json(finish): Json<artisan_api::McpSignInFinish>,
) -> Result<Json<String>> {
    Ok(Json(
        app.hub
            .mcp()
            .sign_in_finish(&finish.state, &finish.code)
            .await?,
    ))
}

#[derive(serde::Deserialize)]
pub(crate) struct CancelSignIn {
    state: String,
}

pub(crate) async fn mcp_sign_in_cancel(
    State(app): State<App>,
    Json(cancel): Json<CancelSignIn>,
) -> axum::http::StatusCode {
    app.hub.mcp().sign_in_cancel(&cancel.state);
    axum::http::StatusCode::NO_CONTENT
}

pub(crate) async fn mcp_sign_out(
    State(app): State<App>,
    Json(out): Json<artisan_api::McpSignOut>,
) -> Result<axum::http::StatusCode> {
    app.hub.mcp().sign_out(&out.server).await?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}

pub(crate) async fn prompt(State(app): State<App>) -> Result<Json<GlobalPrompt>> {
    Ok(Json(app.hub.extensions().global_prompt().await?))
}

pub(crate) async fn save_prompt(
    State(app): State<App>,
    Json(save): Json<SavePrompt>,
) -> Result<Json<GlobalPrompt>> {
    Ok(Json(app.hub.extensions().save_global_prompt(save).await?))
}

pub(crate) async fn resolve_prompt(
    State(app): State<App>,
    Json(resolution): Json<ResolvePrompt>,
) -> Result<Json<GlobalPrompt>> {
    Ok(Json(
        app.hub
            .extensions()
            .resolve_global_prompt(resolution)
            .await?,
    ))
}
