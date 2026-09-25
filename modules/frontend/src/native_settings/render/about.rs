//! About section for `SettingsScreen`: which build is running.
//!
//! Every value comes from the payload's build identity
//! (`artisan-build-info`), so the section answers "is this an old build?"
//! from the installed files themselves rather than from compile-time
//! constants that a stale binary would also carry.

use std::path::Path;

use artisan_build_info::BuildIdentity;

use super::chrome::{settings_card, settings_header, settings_row, settings_section_shell};
use super::*;

impl SettingsScreen {
    /// Renders the About section from the running process's identity.
    pub(super) fn render_about(theme: &artisan_ui::theme::ArtisanTheme) -> Div {
        let identity = BuildIdentity::current();
        let executable = std::env::current_exe().ok();
        let rows = about_rows(identity, executable.as_deref())
            .into_iter()
            .map(|(title, value)| settings_row(theme, title, &value, None))
            .collect();
        let intro = match identity {
            BuildIdentity::Installed(_) => None,
            BuildIdentity::Unstaged(_) => Some(
                "This binary is not part of an installed payload, so it has no recorded commit. Stage it with `cargo dev` to run it as an identified build.",
            ),
        };
        div()
            .flex()
            .flex_col()
            .child(settings_header(
                theme,
                SettingsSection::About.title().to_owned(),
                SettingsSection::About.description(),
            ))
            .child(settings_section_shell(
                theme,
                "build",
                "Build",
                intro,
                None,
                settings_card(theme, rows),
            ))
    }
}

/// Label and value pairs the About section paints, in order.
#[must_use]
pub(crate) fn about_rows(
    identity: &BuildIdentity,
    executable: Option<&Path>,
) -> Vec<(&'static str, String)> {
    let mut rows = vec![("Version", identity.version().to_owned())];
    match identity {
        BuildIdentity::Installed(info) => {
            rows.push(("Channel", info.channel.label().to_owned()));
            rows.push((
                "Commit",
                match &info.commit {
                    Some(commit) if info.dirty => format!("{commit} (with uncommitted changes)"),
                    Some(commit) => commit.clone(),
                    None => "Unknown".to_owned(),
                },
            ));
            rows.push(("Profile", info.profile.clone()));
            rows.push(("Target", info.target.clone()));
            if let Some(built_at) = &info.built_at {
                rows.push(("Built", built_at.clone()));
            }
        }
        BuildIdentity::Unstaged(build) => {
            rows.push(("Channel", "Unstaged".to_owned()));
            rows.push(("Profile", build.profile.to_owned()));
            rows.push(("Target", format!("{}-{}", build.os, build.arch)));
        }
    }
    if let Some(executable) = executable {
        rows.push(("Executable", executable.display().to_string()));
    }
    rows
}
