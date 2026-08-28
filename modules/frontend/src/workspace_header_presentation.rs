//! Pure workspace-header segment and visibility presentation.
//!
//! This is the native counterpart of
//! `modules/frontend/src/routes/components/workspace-header.svelte`. The
//! surrounding adapter owns project/repository inspection, URL parsing, VCS
//! label helpers, host-mark selection, controllers, and rendering. This leaf
//! only selects the visible semantic segments and keeps them in source order.

#![allow(clippy::module_name_repetitions)]
#![forbid(unsafe_code)]

/// The exact text inserted between the repository/workspace and branch name.
pub const WORKSPACE_HEADER_ON: &str = "on";

/// The exact text inserted before a distinct linked checkout name.
pub const WORKSPACE_HEADER_IN: &str = "in";

/// The exact separator inserted before an optional thread title.
pub const WORKSPACE_HEADER_THREAD_SEPARATOR: &str = "/";

/// The exact label used for a detached repository `HEAD`.
pub const WORKSPACE_HEADER_DETACHED_HEAD: &str = "detached HEAD";

/// The branch state needed by the header.
///
/// The protocol's attached and unborn branch states both carry a named
/// branch. Adapters may map either of those states to [`Self::Named`]. A
/// detached state carries no branch name and is rendered as `detached HEAD`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WorkspaceHeaderBranch<'a> {
    /// An attached or unborn branch with its exact protocol name.
    Named(&'a str),
    /// A detached `HEAD`.
    DetachedHead,
}

impl<'a> WorkspaceHeaderBranch<'a> {
    /// Returns the exact branch text visible in the header.
    #[must_use]
    pub const fn label(self) -> &'a str {
        match self {
            Self::Named(name) => name,
            Self::DetachedHead => WORKSPACE_HEADER_DETACHED_HEAD,
        }
    }

    /// Returns whether the branch is detached.
    #[must_use]
    pub const fn is_detached(self) -> bool {
        matches!(self, Self::DetachedHead)
    }
}

/// The fixed text roles emitted by the workspace-header policy.
///
/// Keeping these roles typed leaves their exact strings visible to a renderer
/// without making the policy know how text is painted.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum WorkspaceHeaderText {
    /// The repository-to-branch connector.
    On,
    /// The linked-checkout connector.
    In,
    /// The workspace-to-thread connector.
    ThreadSeparator,
}

impl WorkspaceHeaderText {
    /// Returns the exact visible string for this fixed text role.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::On => WORKSPACE_HEADER_ON,
            Self::In => WORKSPACE_HEADER_IN,
            Self::ThreadSeparator => WORKSPACE_HEADER_THREAD_SEPARATOR,
        }
    }
}

/// Already-derived link and VCS presentation facts for one web remote.
///
/// `web_url` is the exact link target supplied by the adapter. `link_label`
/// and `qualified_label` are the corresponding results of the existing VCS
/// label helpers, and `host_mark` is the result of host-mark selection. This
/// type deliberately does not parse or validate the URL and does not know the
/// mark's asset or rendering type.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct WorkspaceHeaderRemoteLink<'a, M = &'a str> {
    /// The exact web URL used as the link target.
    pub web_url: &'a str,
    /// The already-derived repository-only label used for checkout comparison.
    pub link_label: &'a str,
    /// The already-derived qualified label shown for the remote link.
    pub qualified_label: &'a str,
    /// The already-selected host mark, opaque to this policy.
    pub host_mark: M,
}

impl<'a, M> WorkspaceHeaderRemoteLink<'a, M> {
    /// Builds a link from already-derived adapter values without parsing or
    /// normalizing any of them.
    #[must_use]
    pub const fn new(
        web_url: &'a str,
        link_label: &'a str,
        qualified_label: &'a str,
        host_mark: M,
    ) -> Self {
        Self {
            web_url,
            link_label,
            qualified_label,
            host_mark,
        }
    }

    /// Returns the exact URL target under the renderer-facing `href` name.
    #[must_use]
    pub const fn href(&self) -> &str {
        self.web_url
    }

    /// Returns the repository-only label used by the checkout comparison.
    #[must_use]
    pub const fn repository_label(&self) -> &str {
        self.link_label
    }

    /// Returns the exact qualified label shown for the remote link.
    #[must_use]
    pub const fn qualified_label(&self) -> &str {
        self.qualified_label
    }

    /// Returns the opaque host mark selected by the adapter.
    #[must_use]
    pub const fn host_mark(&self) -> &M {
        &self.host_mark
    }
}

/// One configured remote in source order.
///
/// A `None` link represents a remote whose inspection did not produce a web
/// URL. The remote remains present so exact first-match behavior is preserved.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct WorkspaceHeaderRemote<'a, M = &'a str> {
    /// The exact configured remote name.
    pub name: &'a str,
    /// The already-derived web-link presentation, when a web URL exists.
    pub link: Option<WorkspaceHeaderRemoteLink<'a, M>>,
}

impl<'a, M> WorkspaceHeaderRemote<'a, M> {
    /// Builds one remote without changing its name or optional link.
    #[must_use]
    pub const fn new(name: &'a str, link: Option<WorkspaceHeaderRemoteLink<'a, M>>) -> Self {
        Self { name, link }
    }

    /// Builds a remote that has no browser-openable web link.
    #[must_use]
    pub const fn without_link(name: &'a str) -> Self {
        Self { name, link: None }
    }
}

/// The repository observation consumed by the header.
///
/// `default_remote` is compared to remote names exactly. The adapter supplies
/// the remotes in protocol order, including duplicates, and this leaf does not
/// sort, deduplicate, or select a fallback remote.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct WorkspaceHeaderRepository<'a, M = &'a str> {
    /// The exact current branch state.
    pub branch: WorkspaceHeaderBranch<'a>,
    /// The exact configured default-remote name, when one exists.
    pub default_remote: Option<&'a str>,
    /// Configured remotes in their original protocol order.
    pub remotes: &'a [WorkspaceHeaderRemote<'a, M>],
}

impl<'a, M> WorkspaceHeaderRepository<'a, M> {
    /// Builds a repository observation from already-decoded protocol fields.
    #[must_use]
    pub const fn new(
        branch: WorkspaceHeaderBranch<'a>,
        default_remote: Option<&'a str>,
        remotes: &'a [WorkspaceHeaderRemote<'a, M>],
    ) -> Self {
        Self {
            branch,
            default_remote,
            remotes,
        }
    }
}

/// The result of project-root repository inspection.
///
/// Inspection may be absent at the outer input boundary, or explicitly report
/// [`Self::NotRepository`]. Both cases use the folder presentation. A
/// repository observation additionally contributes the branch and `on`
/// segments.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum WorkspaceHeaderRepositoryInspection<'a, M = &'a str> {
    /// The project root is not a repository.
    NotRepository,
    /// The project root is a repository.
    Repository(WorkspaceHeaderRepository<'a, M>),
}

impl<'a, M> WorkspaceHeaderRepositoryInspection<'a, M> {
    /// Builds the explicit non-repository inspection result.
    #[must_use]
    pub const fn not_repository() -> Self {
        Self::NotRepository
    }

    /// Builds a repository inspection result.
    #[must_use]
    pub const fn repository(repository: WorkspaceHeaderRepository<'a, M>) -> Self {
        Self::Repository(repository)
    }
}

/// The already-decoded facts consumed by the workspace-header policy.
///
/// `project_display_name == None` means there is no project in the open route;
/// in that case the entire header is absent. `thread_title` uses `Option` for
/// presence, so `Some("")` still emits the slash and an empty title segment.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct WorkspaceHeaderInput<'a, M = &'a str> {
    /// The project folder/checkout label, when a project is present.
    pub project_display_name: Option<&'a str>,
    /// The retained repository inspection, when one is available.
    pub repository: Option<WorkspaceHeaderRepositoryInspection<'a, M>>,
    /// The open thread title, when the route supplies one.
    pub thread_title: Option<&'a str>,
}

impl<'a, M> WorkspaceHeaderInput<'a, M> {
    /// Builds a header input without copying any string or adapter mark.
    #[must_use]
    pub const fn new(
        project_display_name: Option<&'a str>,
        repository: Option<WorkspaceHeaderRepositoryInspection<'a, M>>,
        thread_title: Option<&'a str>,
    ) -> Self {
        Self {
            project_display_name,
            repository,
            thread_title,
        }
    }
}

/// A semantic segment in the exact order used by the legacy header.
///
/// A renderer maps `Folder`, `RemoteLink`, `Branch`, and `Checkout` to its
/// icons and controls. The policy carries only visible labels and the opaque
/// remote link/mark facts; it never selects or loads an icon.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum WorkspaceHeaderSegment<'a, M = &'a str> {
    /// The project folder label, rendered with a plain folder icon.
    Folder {
        /// Exact project display name.
        label: &'a str,
    },
    /// The selected remote's link, qualified label, and host mark.
    RemoteLink {
        /// Borrowed adapter facts for the selected first matching remote.
        link: &'a WorkspaceHeaderRemoteLink<'a, M>,
    },
    /// One of the fixed `on`, `in`, or `/` text segments.
    Text(WorkspaceHeaderText),
    /// The repository branch marker and its exact visible label.
    Branch {
        /// Attached/unborn named branch or detached `HEAD`.
        branch: WorkspaceHeaderBranch<'a>,
    },
    /// The linked checkout's project display name, rendered with a folder-code
    /// icon after the preceding `in` text segment.
    Checkout {
        /// Exact project display name.
        label: &'a str,
    },
    /// The optional thread title, including an explicitly empty title.
    ThreadTitle {
        /// Exact thread title with no trimming or normalization.
        title: &'a str,
    },
}

impl<'a, M> WorkspaceHeaderSegment<'a, M> {
    /// Returns the exact visible text represented by this segment.
    ///
    /// For icon-bearing segments this is the text alongside the icon. The
    /// returned value always borrows the adapter input and never allocates.
    #[must_use]
    pub const fn visible_text(&self) -> &'a str {
        match self {
            Self::Folder { label } | Self::Checkout { label } => label,
            Self::RemoteLink { link } => link.qualified_label,
            Self::Text(text) => text.as_str(),
            Self::Branch { branch } => branch.label(),
            Self::ThreadTitle { title } => title,
        }
    }
}

/// The visible workspace-header projection.
///
/// The vector owns only the ordered segment list. Every label and remote link
/// fact inside it borrows from [`WorkspaceHeaderInput`], making this suitable
/// for a renderer that consumes the projection while its adapter snapshot is
/// retained.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct WorkspaceHeaderPresentation<'a, M = &'a str> {
    /// Semantic segments in exact source/render order.
    pub segments: Vec<WorkspaceHeaderSegment<'a, M>>,
}

impl<'a, M> WorkspaceHeaderPresentation<'a, M> {
    /// Returns the ordered segment slice without transferring ownership.
    #[must_use]
    pub fn segments(&self) -> &[WorkspaceHeaderSegment<'a, M>] {
        &self.segments
    }

    /// Transfers the ordered segment list to the renderer adapter.
    #[must_use]
    pub fn into_segments(self) -> Vec<WorkspaceHeaderSegment<'a, M>> {
        self.segments
    }

    /// A returned presentation is always visible; absence is represented by
    /// `None` from [`present_workspace_header`].
    #[must_use]
    pub const fn is_visible(&self) -> bool {
        !self.segments.is_empty()
    }
}

/// Compares strings with the same Unicode case-folding direction as the
/// legacy JavaScript `.toLowerCase()` expressions.
fn same_lowercase(left: &str, right: &str) -> bool {
    left.to_lowercase() == right.to_lowercase()
}

/// Projects the exact workspace-header segments from already-decoded facts.
///
/// The policy follows the Svelte source in order:
///
/// 1. no project returns no header;
/// 2. an absent/non-repository inspection uses the folder label;
/// 3. repository inspection selects the first remote whose name exactly
///    equals `default_remote`;
/// 4. a selected remote with a web link contributes its qualified link/mark,
///    otherwise the folder label is used;
/// 5. repository state always contributes `on` and the branch label;
/// 6. a distinct linked checkout contributes `in` and its folder-code label;
/// 7. a present thread title contributes `/` and the exact title.
///
/// No URL parsing, controller/stream work, icon selection, or rendering is
/// performed here. The returned presentation borrows all adapter values.
#[must_use]
pub fn present_workspace_header<M>(
    input: WorkspaceHeaderInput<'_, M>,
) -> Option<WorkspaceHeaderPresentation<'_, M>> {
    let WorkspaceHeaderInput {
        project_display_name,
        repository,
        thread_title,
    } = input;
    let project_display_name = project_display_name?;
    let mut segments = Vec::with_capacity(7);

    if let Some(WorkspaceHeaderRepositoryInspection::Repository(repository)) = repository {
        let selected_remote = repository.default_remote.and_then(|default_remote| {
            repository
                .remotes
                .iter()
                .find(|remote| remote.name == default_remote)
        });

        if let Some(link) = selected_remote.and_then(|remote| remote.link.as_ref()) {
            segments.push(WorkspaceHeaderSegment::RemoteLink { link });

            segments.push(WorkspaceHeaderSegment::Text(WorkspaceHeaderText::On));
            segments.push(WorkspaceHeaderSegment::Branch {
                branch: repository.branch,
            });

            if !same_lowercase(project_display_name, link.link_label)
                && !same_lowercase(project_display_name, "default")
            {
                segments.push(WorkspaceHeaderSegment::Text(WorkspaceHeaderText::In));
                segments.push(WorkspaceHeaderSegment::Checkout {
                    label: project_display_name,
                });
            }
        } else {
            segments.push(WorkspaceHeaderSegment::Folder {
                label: project_display_name,
            });
            segments.push(WorkspaceHeaderSegment::Text(WorkspaceHeaderText::On));
            segments.push(WorkspaceHeaderSegment::Branch {
                branch: repository.branch,
            });
        }
    } else {
        segments.push(WorkspaceHeaderSegment::Folder {
            label: project_display_name,
        });
    }

    if let Some(title) = thread_title {
        segments.push(WorkspaceHeaderSegment::Text(
            WorkspaceHeaderText::ThreadSeparator,
        ));
        segments.push(WorkspaceHeaderSegment::ThreadTitle { title });
    }

    Some(WorkspaceHeaderPresentation { segments })
}

/// Alias naming the returned value after the component's presentation role.
#[must_use]
pub fn workspace_header_presentation<M>(
    input: WorkspaceHeaderInput<'_, M>,
) -> Option<WorkspaceHeaderPresentation<'_, M>> {
    present_workspace_header(input)
}
