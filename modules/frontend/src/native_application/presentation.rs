//! Context-free presentation helpers for the native application: status
//! panels, engine-settings detail text, and titlebar fact mapping.
//!
//! Extracted verbatim from `native_application.rs` during the phase-1 module
//! split; visibility was widened to `pub(super)` for the parent module.

use artisan_assets::AssetId;
use artisan_ui::card::{CardStyle, compact_card, compact_card_content};
use artisan_ui::separator::{SeparatorAxis, separator};
use artisan_ui::theme::ArtisanTheme;
use gpui::prelude::{InteractiveElement as _, ParentElement as _, Styled as _};
use gpui::{Div, FontWeight, div};

use super::selectors::NATIVE_STATUS_SELECTOR;
use super::state::NativeViewState;
use crate::repository_mark::RepositoryLogo;
use crate::titlebar_header_presentation::{TitlebarRepository, titlebar_repository_from_remote};

#[cfg(test)]
use super::state::NativeMessageFailure;
#[cfg(test)]
use artisan_protocol::QueueMessageReceipt;

/// Returns the receipt/failure detail line consumed by the status panel's
/// tests; the live status panel renders its own copy.
#[cfg(test)]
pub(super) fn message_status_detail(
    receipt: Option<&QueueMessageReceipt>,
    failure: Option<NativeMessageFailure>,
) -> Option<String> {
    Some(if let Some(receipt) = receipt {
        let disposition = match receipt.disposition {
            artisan_domain::ReceiptDisposition::Accepted => "accepted",
            artisan_domain::ReceiptDisposition::Duplicate => "duplicate",
        };
        format!(
            "Message {disposition}; Forge message id {}.",
            receipt.message_id.as_str()
        )
    } else {
        let failure = failure?;
        format!(
            "Send failed: {} ({}).",
            failure.failure.stage, failure.failure.category
        )
    })
}

pub(super) fn status_panel(theme: &ArtisanTheme, state: &NativeViewState) -> Div {
    let (heading, detail): (&'static str, String) = match state {
        NativeViewState::Loading => (
            "Loading Artisan data",
            "Connecting to the owned local Forge.".to_owned(),
        ),
        NativeViewState::EmptyProjects => (
            "No attached projects",
            "Attach a project to begin a conversation.".to_owned(),
        ),
        NativeViewState::LoadingThreads => (
            "Loading project threads",
            "Reading the selected project from Forge.".to_owned(),
        ),
        NativeViewState::EmptyThreads => (
            "No threads in this project",
            "Choose another project or attach a new one.".to_owned(),
        ),
        NativeViewState::Ready => (
            "Conversation unavailable",
            "No conversation host is mounted.".to_owned(),
        ),
        NativeViewState::Failure(failure) => (
            "Native connection unavailable",
            format!("Service state: {failure}"),
        ),
    };
    status_panel_with_text(theme, heading, detail)
}

/// Projects one inspected repository observation onto titlebar facts.
///
/// Only the default remote's browser-facing URL earns the link; a repository
/// with no browsable remote, and a directory Git does not track, keep the
/// project-folder fallback instead of synthesizing a link.
pub(super) fn titlebar_repository_for_project(
    repository: &artisan_protocol::ProjectRepository,
) -> Option<TitlebarRepository> {
    let snapshot = repository.snapshot()?;
    let default_remote = snapshot.default_remote()?;
    let remote = snapshot
        .remotes()
        .iter()
        .find(|remote| remote.name() == default_remote)?;
    titlebar_repository_from_remote(remote.host().as_str(), remote.web_url())
}

/// The muted-foreground tone carried by the titlebar header's workspace
/// context: the project-folder fallback and the thread subject.
///
/// The reference strip paints both in `text-muted-foreground`; only the
/// repository link and the darker separator override the inherited tone.
pub(super) fn titlebar_context_tone(theme: &ArtisanTheme) -> gpui::Hsla {
    theme.colors.muted_foreground.to_paint()
}

/// Maps a repository mark identity to its cataloged native asset.
///
/// Every vendored host mark the reference table can select has a catalog row;
/// the plain Git mark is the fallback identity for local and unknown hosts.
pub(super) fn repository_logo_asset(logo: RepositoryLogo) -> AssetId {
    match logo {
        RepositoryLogo::Git => AssetId::SVGL_GIT,
        RepositoryLogo::GitHub => AssetId::SVGL_GITHUB,
        RepositoryLogo::GitLab => AssetId::SVGL_GITLAB,
        RepositoryLogo::MicrosoftAzure => AssetId::SVGL_MICROSOFT_AZURE,
        RepositoryLogo::Bitbucket => AssetId::SIMPLE_ICONS_BITBUCKET,
        RepositoryLogo::Codeberg => AssetId::SIMPLE_ICONS_CODEBERG,
        RepositoryLogo::Gitea => AssetId::SIMPLE_ICONS_GITEA,
        RepositoryLogo::Sourcehut => AssetId::SIMPLE_ICONS_SOURCEHUT,
    }
}

pub(super) fn status_panel_with_text(
    theme: &ArtisanTheme,
    heading: &'static str,
    detail: String,
) -> Div {
    let style = CardStyle::resolve(*theme);
    compact_card(style)
        .w_full()
        .debug_selector(|| NATIVE_STATUS_SELECTOR.to_string())
        .child(
            compact_card_content(style).child(
                div()
                    .text_size(theme.typography.dialog_title_text)
                    .font_weight(FontWeight::MEDIUM)
                    .child(heading),
            ),
        )
        .child(separator(
            theme.colors.border.to_paint(),
            SeparatorAxis::Horizontal,
        ))
        .child(
            compact_card_content(style).child(
                div()
                    .text_size(theme.typography.control_text)
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(detail),
            ),
        )
}

pub(super) fn profile_usage_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

/// Display a raw OS account or machine string with only its first letter
/// capitalized, so `sander` paints as `Sander`. The stored value is left
/// untouched so avatar seeds and identity matching stay stable.
pub(super) fn capitalize_label(value: &str) -> String {
    let mut characters = value.chars();
    match characters.next() {
        None => String::new(),
        Some(first) => {
            first.to_uppercase().collect::<String>() + &characters.as_str().to_lowercase()
        }
    }
}
