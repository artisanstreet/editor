//! About (`render/about.rs`): which build is running and from where.

use crate::chrome::{card, header, row, section_shell};
use crate::page;
use editor_core::SettingsPage;
use editor_ui::theme::ArtisanTheme;
use gpui::{Div, IntoElement, ParentElement as _, Styled as _, div};
use std::path::Path;

/// The running build, as the binary was compiled.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BuildInfo {
    pub version: String,
    /// Empty when the build had no git checkout to read.
    pub commit: String,
    /// `debug` or `release`.
    pub profile: String,
    pub target: String,
}

/// The label and value of each row, in order.
pub(crate) fn rows(build: &BuildInfo, executable: Option<&Path>) -> Vec<(&'static str, String)> {
    let mut rows = vec![("Version", build.version.clone())];
    rows.push((
        "Commit",
        if build.commit.is_empty() {
            "Unknown".to_owned()
        } else {
            build.commit.clone()
        },
    ));
    rows.push(("Profile", build.profile.clone()));
    rows.push(("Target", build.target.clone()));
    if let Some(executable) = executable {
        rows.push(("Executable", executable.display().to_string()));
    }
    rows
}

pub(crate) fn render(build: &BuildInfo, theme: &ArtisanTheme) -> Div {
    let executable = std::env::current_exe().ok();
    let rows = rows(build, executable.as_deref())
        .into_iter()
        .map(|(title, value)| row(theme, title, value, None).into_any_element())
        .collect();
    div()
        .flex()
        .flex_col()
        .child(header(
            theme,
            page::label(SettingsPage::About),
            page::description(SettingsPage::About),
        ))
        .child(section_shell(
            theme,
            "build",
            "Build",
            None,
            None,
            card(theme, rows),
        ))
}
