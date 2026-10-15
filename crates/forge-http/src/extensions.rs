//! Skills and the global prompt use the same typed HTTP failure mapping as
//! the ordinary Forge routes.

use crate::routes::{App, Result};
use artisan_api::{CatalogSkill, GlobalPrompt, ResolvePrompt, SavePrompt, Skill};
use axum::Json;
use axum::extract::{Path, State};

/// The skills a `$name` mention can name: Artisan's own and the user's,
/// less the ones turned off.
pub(crate) async fn skills(State(app): State<App>) -> Result<Json<Vec<Skill>>> {
    Ok(Json(app.hub.enabled_skills().await?))
}

pub(crate) async fn catalog(State(app): State<App>) -> Result<Json<Vec<CatalogSkill>>> {
    Ok(Json(app.hub.skill_catalog().await?))
}

/// `enable`, `disable` or `adopt`.
pub(crate) async fn skill_action(
    State(app): State<App>,
    Path((id, action)): Path<(String, artisan_api::SkillAction)>,
) -> Result<Json<Vec<CatalogSkill>>> {
    let catalog = match action {
        artisan_api::SkillAction::Enable => app.hub.set_skill_enabled(&id, true).await?,
        artisan_api::SkillAction::Disable => app.hub.set_skill_enabled(&id, false).await?,
        artisan_api::SkillAction::Adopt => app.hub.adopt_skill(&id).await?,
    };
    Ok(Json(catalog))
}

pub(crate) async fn prompt(State(app): State<App>) -> Result<Json<GlobalPrompt>> {
    Ok(Json(app.hub.global_prompt().await?))
}

pub(crate) async fn save_prompt(
    State(app): State<App>,
    Json(save): Json<SavePrompt>,
) -> Result<Json<GlobalPrompt>> {
    Ok(Json(app.hub.save_global_prompt(save).await?))
}

pub(crate) async fn resolve_prompt(
    State(app): State<App>,
    Json(resolution): Json<ResolvePrompt>,
) -> Result<Json<GlobalPrompt>> {
    Ok(Json(app.hub.resolve_global_prompt(resolution).await?))
}
