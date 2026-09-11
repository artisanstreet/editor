//! Pure titlebar workspace-header composition.
//!
//! The reference desktop shell names the open workspace and the conversation
//! inside it on one line at the leading end of the window strip:
//! `<vcs mark> owner/repository / <thread title>`. This is the native
//! projection of that line. The adapter owns repository inspection, host-mark
//! selection, and rendering; this leaf only selects the visible segments and
//! their exact order.
//!
//! The repository segment is emitted only from inspected facts. No production
//! repository source is wired yet, so adapters currently pass
//! `repository: None` and the project-folder fallback paints; the types here
//! exist so the forthcoming Git read query can fill the line without
//! reshaping the composition.

#![allow(clippy::module_name_repetitions)]
#![forbid(unsafe_code)]

use crate::repository_mark::RepositoryHost;
use crate::vcs_labels::repository_qualified_label;

/// The exact separator between the workspace context and the thread subject.
pub const TITLEBAR_HEADER_THREAD_SEPARATOR: &str = "/";

/// One project repository whose default remote names a browser page.
///
/// Construct this only from inspected repository facts: a repository state, a
/// selected default remote, and that remote's browser-facing `web_url`. The
/// titlebar never synthesizes a link; a repository with no web remote stays
/// out of this type entirely, and the project-folder fallback paints instead.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TitlebarRepository {
    /// Host mark identity selected from the remote URL.
    pub host: RepositoryHost,
    /// The remote's browser-facing web URL.
    pub web_url: String,
}

impl TitlebarRepository {
    /// Creates a repository fact without changing either supplied value.
    #[must_use]
    pub fn new(host: RepositoryHost, web_url: impl Into<String>) -> Self {
        Self {
            host,
            web_url: web_url.into(),
        }
    }

    /// Derives the qualified `owner/repository` label shown for the link.
    ///
    /// This is the existing [`repository_qualified_label`] projection, kept on
    /// the fact so composition never re-parses the URL.
    #[must_use]
    pub fn qualified_label(&self) -> String {
        repository_qualified_label(&self.web_url)
    }
}

/// The already-decoded facts consumed by the titlebar header policy.
///
/// `project_display_name == None` means the open route has no project (or the
/// catalog has not loaded); the thread subject still paints, without the
/// context prefix and separator. `thread_title == None` means the route names
/// no conversation at all, which is the bare-wordmark case.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TitlebarHeaderInput<'a> {
    /// The open project's display label, when a project is selected.
    pub project_display_name: Option<&'a str>,
    /// Inspected repository facts, when the project's default remote names a
    /// browser page.
    pub repository: Option<&'a TitlebarRepository>,
    /// The open conversation's title, when the route names one.
    pub thread_title: Option<&'a str>,
}

impl<'a> TitlebarHeaderInput<'a> {
    /// Builds a header input without copying any presented string.
    #[must_use]
    pub const fn new(
        project_display_name: Option<&'a str>,
        repository: Option<&'a TitlebarRepository>,
        thread_title: Option<&'a str>,
    ) -> Self {
        Self {
            project_display_name,
            repository,
            thread_title,
        }
    }
}

/// One semantic segment of the titlebar workspace header, in source order.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum TitlebarHeaderSegment<'a> {
    /// The repository host mark, selected from the remote URL.
    RepositoryMark {
        /// Mark identity resolved from the remote's host.
        host: RepositoryHost,
    },
    /// The repository link: qualified label plus its browser destination.
    RepositoryLink {
        /// The already-derived qualified `owner/repository` label.
        label: String,
        /// The remote's browser-facing web URL.
        web_url: &'a str,
    },
    /// The project-folder fallback when no browsable repository exists.
    ProjectFolder {
        /// Exact project display name.
        label: &'a str,
    },
    /// The separator before the thread subject.
    ThreadSeparator,
    /// The open conversation's title.
    ThreadTitle {
        /// Exact thread title with no trimming or normalization.
        title: &'a str,
    },
}

/// The visible titlebar workspace-header projection.
///
/// The vector owns only the ordered segment list; every label borrows from
/// [`TitlebarHeaderInput`] or its repository fact.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TitlebarHeaderPresentation<'a> {
    /// Semantic segments in exact source/render order.
    pub segments: Vec<TitlebarHeaderSegment<'a>>,
}

impl<'a> TitlebarHeaderPresentation<'a> {
    /// Returns the ordered segment slice without transferring ownership.
    #[must_use]
    pub fn segments(&self) -> &[TitlebarHeaderSegment<'a>] {
        &self.segments
    }

    /// Transfers the ordered segment list to the renderer adapter.
    #[must_use]
    pub fn into_segments(self) -> Vec<TitlebarHeaderSegment<'a>> {
        self.segments
    }

    /// A returned presentation is always visible; absence is represented by
    /// `None` from [`present_titlebar_header`].
    #[must_use]
    pub const fn is_visible(&self) -> bool {
        !self.segments.is_empty()
    }
}

/// Projects the titlebar workspace header from already-decoded facts.
///
/// The policy follows the reference strip in order:
///
/// 1. a route with no conversation returns no header (bare wordmark);
/// 2. a present repository contributes its host mark and qualified link;
/// 3. otherwise a present project contributes its folder label;
/// 4. a present project contributes the `/` separator before the subject;
/// 5. the conversation title always closes the line.
///
/// No URL parsing, repository inspection, host-mark asset selection, or
/// rendering is performed here.
#[must_use]
pub fn present_titlebar_header(
    input: TitlebarHeaderInput<'_>,
) -> Option<TitlebarHeaderPresentation<'_>> {
    let thread_title = input.thread_title?;
    let mut segments = Vec::with_capacity(4);

    if let Some(project_display_name) = input.project_display_name {
        if let Some(repository) = input.repository {
            segments.push(TitlebarHeaderSegment::RepositoryMark {
                host: repository.host,
            });
            segments.push(TitlebarHeaderSegment::RepositoryLink {
                label: repository.qualified_label(),
                web_url: repository.web_url.as_str(),
            });
        } else {
            segments.push(TitlebarHeaderSegment::ProjectFolder {
                label: project_display_name,
            });
        }
        segments.push(TitlebarHeaderSegment::ThreadSeparator);
    }

    segments.push(TitlebarHeaderSegment::ThreadTitle {
        title: thread_title,
    });

    Some(TitlebarHeaderPresentation { segments })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn github_repository() -> TitlebarRepository {
        TitlebarRepository::new(
            RepositoryHost::GitHub,
            "https://github.com/artisanstreet/editor",
        )
    }

    #[test]
    fn qualified_label_is_the_owner_and_repository_name() {
        assert_eq!(
            github_repository().qualified_label(),
            "artisanstreet/editor"
        );
    }

    #[test]
    fn repository_facts_compose_mark_link_separator_and_title() {
        let repository = github_repository();
        let presentation = present_titlebar_header(TitlebarHeaderInput::new(
            Some("editor"),
            Some(&repository),
            Some("Ship the port"),
        ))
        .expect("a named conversation has a header");

        assert_eq!(
            presentation.segments(),
            [
                TitlebarHeaderSegment::RepositoryMark {
                    host: RepositoryHost::GitHub,
                },
                TitlebarHeaderSegment::RepositoryLink {
                    label: "artisanstreet/editor".to_owned(),
                    web_url: "https://github.com/artisanstreet/editor",
                },
                TitlebarHeaderSegment::ThreadSeparator,
                TitlebarHeaderSegment::ThreadTitle {
                    title: "Ship the port",
                },
            ]
        );
    }

    #[test]
    fn missing_repository_uses_the_project_folder_fallback() {
        let presentation = present_titlebar_header(TitlebarHeaderInput::new(
            Some("editor"),
            None,
            Some("Ship the port"),
        ))
        .expect("a named conversation has a header");

        assert_eq!(
            presentation.segments(),
            [
                TitlebarHeaderSegment::ProjectFolder { label: "editor" },
                TitlebarHeaderSegment::ThreadSeparator,
                TitlebarHeaderSegment::ThreadTitle {
                    title: "Ship the port",
                },
            ]
        );
    }

    #[test]
    fn missing_project_keeps_the_subject_alone() {
        let presentation = present_titlebar_header(TitlebarHeaderInput::new(
            None,
            None,
            Some("Ship the port"),
        ))
        .expect("a named conversation has a header");

        assert_eq!(
            presentation.segments(),
            [TitlebarHeaderSegment::ThreadTitle {
                title: "Ship the port",
            }]
        );
    }

    #[test]
    fn subject_less_routes_have_no_header() {
        let repository = github_repository();
        assert!(
            present_titlebar_header(TitlebarHeaderInput::new(
                Some("editor"),
                Some(&repository),
                None,
            ))
            .is_none(),
            "the workspace context alone never replaces the bare wordmark"
        );
        assert!(
            present_titlebar_header(TitlebarHeaderInput::new(None, None, None)).is_none(),
            "a route with nothing to name paints nothing"
        );
    }

    #[test]
    fn presentation_is_visible_exactly_when_it_has_segments() {
        let presentation = present_titlebar_header(TitlebarHeaderInput::new(
            Some("editor"),
            None,
            Some(""),
        ))
        .expect("an explicitly empty title still closes the line");
        assert!(presentation.is_visible());
        assert_eq!(
            presentation.into_segments().last(),
            Some(&TitlebarHeaderSegment::ThreadTitle { title: "" })
        );
    }
}
