//! Native GPUI port of the `/e/[workspace]/[thread]` editor route surface.
//!
//! Legacy sources (read-only reference, never edited):
//!
//! - `routes/e/[workspace]/[thread]/editor-route-gate.svelte` — route params
//!   and gate logic. The gate policy already lives in
//!   [`crate::editor_route_gate_policy`]; this leaf keeps that logic untouched
//!   and ports only the visuals mounted after activation.
//! - `routes/components/editor-route.svelte` — the editor content column:
//!   no-file empty state (mark or recent-changes table), opening skeletons,
//!   per-path failure card, and the mounted surface.
//! - `routes/components/editor-file-panel.svelte` plus
//!   `routes/components/workspace-file-tree.svelte` — the inspector-column
//!   file tree: heading, indentation, disclosure, row states, loading and
//!   empty copies.
//! - `lib/components/editor/surface.svelte` plus `lib/editor/{theme,language}`
//!   (the `CodeMirror` surface). There is no native text-editing engine yet, so
//!   this leaf renders the surface frame (gutter metrics, active-line wash,
//!   theme colors) around read-only file content. A live editing buffer is an
//!   explicit non-goal and is recorded as a gap, not faked.
//! - `routes/+layout.svelte` — composition order: header row on top, file
//!   panel in the inspector column beside the primary surface.
//! - `routes/components/workspace-header.svelte` — header segments, consumed
//!   through [`crate::workspace_header_presentation`] without modification.
//!
//! Data arrives injected: no workspace file-listing controller exists in Rust
//! yet, so the orchestrator supplies the ordered tree entries, the open-file
//! content, and the already-formatted recent-change rows. An empty entry list
//! renders the honest empty copy, never invented files. Pointer activation
//! never navigates here; it records an [`EditorAction`] the orchestrator
//! consumes, mirroring the `NativeThreadPicker` pending-action seam.

#![allow(clippy::module_name_repetitions)]

use std::collections::HashSet;

use artisan_assets::AssetId;
use artisan_domain::{ProjectId, ThreadId};
use artisan_ui::asset_seam::asset_glyph;
use artisan_ui::icon::{IconSize, IconStyle, IconTint, icon};
use artisan_ui::motion::MotionPolicy;
use artisan_ui::skeleton::{SkeletonStyle, skeleton};
use artisan_ui::theme::{ArtisanTheme, Oklch};
use gpui::{
    AnyElement, ClickEvent, Context, Div, FontWeight, Hsla, InteractiveElement as _,
    ParentElement as _, Render, ScrollHandle, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::IntoElement, px, relative,
};

use crate::editor_diagnostic_mapping::{
    EditorDiagnostic, EditorDiagnosticSeverity, MappedMarker, map_editor_diagnostics,
};
use crate::editor_language::{EditorLanguageId, editor_language_for_path};
use crate::editor_workspace_identity::editor_route_path;
use crate::file_icon::{FileIcon, resolve_file_icon};
use crate::tab_derivations::{WorkspaceFileReference, derive_breadcrumbs};
use crate::vcs_labels::{repository_link_label, repository_qualified_label};
use crate::workspace_header_presentation::{
    WorkspaceHeaderBranch, WorkspaceHeaderInput, WorkspaceHeaderRemote, WorkspaceHeaderRemoteLink,
    WorkspaceHeaderRepository, WorkspaceHeaderRepositoryInspection, WorkspaceHeaderSegment,
    present_workspace_header,
};
use crate::workspace_tab_state::EditorViewState as RetainedEditorViewState;

/// Debug selector for the editor screen root.
pub const EDITOR_SCREEN_SELECTOR: &str = "route-editor-screen";
/// Debug selector for the workspace header row.
pub const EDITOR_HEADER_SELECTOR: &str = "editor-header";
/// Debug selector for the file-tree inspector column.
pub const EDITOR_FILE_PANEL_SELECTOR: &str = "editor-file-panel";
/// Debug selector for the editor surface column.
pub const EDITOR_SURFACE_SELECTOR: &str = "editor-surface";
/// Prefix for per-row debug selectors (`{prefix}-{visible index}`).
pub const EDITOR_TREE_ROW_SELECTOR_PREFIX: &str = "editor-file-tree-row";
/// Debug selector for the loading-skeleton column.
pub const EDITOR_LOADING_SELECTOR: &str = "editor-loading";
/// Debug selector for the per-path failure card.
pub const EDITOR_FAILURE_SELECTOR: &str = "editor-failure";
/// Debug selector for the no-file empty state.
pub const EDITOR_EMPTY_SELECTOR: &str = "editor-empty";
/// Debug selector for the recent-changes table.
pub const EDITOR_RECENTS_SELECTOR: &str = "editor-recent-files";

/// Inspector-column heading (`editor-file-panel.svelte`).
pub const FILE_PANEL_TITLE: &str = "Files";
/// Honest empty-tree copy (`editor-file-panel.svelte`).
pub const EMPTY_TREE_TEXT: &str = "This project has no files.";
/// Per-directory loading copy.
pub const TREE_LOADING_TEXT: &str = "Loading…";
/// Recent-changes heading (`editor-route.svelte`).
pub const RECENT_FILES_TITLE: &str = "Recently changed files";
/// Per-path failure title (`editor-route.svelte`).
pub const FILE_FAILURE_TITLE: &str = "This file can't be displayed";

/// Editor body text: `CodeMirror` `fontSize: "0.8125rem"` (`lib/editor/theme.ts`).
/// No typography token carries 13 px, so this stays a named leaf const.
const EDITOR_TEXT_PX: f32 = 13.0;
/// Editor line height: `.cm-scroller` `lineHeight: "1.6"` over 13 px text.
const EDITOR_LINE_HEIGHT_PX: f32 = 20.8;
/// Tab expansion width for read-only rendering.
const EDITOR_TAB_WIDTH: usize = 4;
/// Gutter number color: `--foreground` at 35% (`.cm-gutters`).
const GUTTER_TEXT_ALPHA: f32 = 0.35;
/// Active-line gutter number: `--foreground` at 70%.
const ACTIVE_GUTTER_TEXT_ALPHA: f32 = 0.70;
/// Active-line row wash: `--foreground` at 4% (`.cm-activeLine`).
const ACTIVE_LINE_ALPHA: f32 = 0.04;
/// Diagnostic row wash alpha (read-only stand-in for lint markers).
const DIAGNOSTIC_WASH_ALPHA: f32 = 0.12;
/// File-panel width: midpoint of `clamp(16rem, 25vw, 350px)`.
const FILE_PANEL_WIDTH_PX: f32 = 300.0;
/// Tree indentation: `padding-left: 0.25 + depth * 0.75 (+ 0.75 file) rem`.
const TREE_INDENT_BASE_PX: f32 = 4.0;
/// Per-depth tree indentation step: `0.75rem`.
const TREE_INDENT_DEPTH_PX: f32 = 12.0;
/// Extra file-row indent past the directory row.
const TREE_FILE_EXTRA_PX: f32 = 12.0;
/// Recent-changes cap: legacy keeps 8 rows.
const RECENT_FILES_MAX: usize = 8;
/// Loading skeleton widths: `w-2/5 w-4/5 w-3/4 w-5/6 w-1/2`; heights `h-4`.
const LOADING_SKELETON_FRACTIONS: [f32; 5] = [0.4, 0.8, 0.75, 5.0 / 6.0, 0.5];
/// Empty-state mark size: `<ArtisanLogo size={56} />`.
const EMPTY_MARK_PX: f32 = 56.0;
/// The kind of content one tree entry represents.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EditorTreeKind {
    /// A collapsible directory row.
    Directory,
    /// An openable file row.
    File,
}

/// One file-tree row in display order.
///
/// Parents precede their children and siblings keep listing order; `depth` is
/// the nesting level below the workspace root. The orchestrator owns fetching:
/// children of a collapsed directory are simply skipped at render time, and an
/// expanded directory with no listed children renders the loading copy until
/// the orchestrator refills the list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorTreeEntry {
    /// Workspace-relative path, the stable identity used for expansion,
    /// selection, and open actions.
    pub path: String,
    /// Basename shown in the row.
    pub name: String,
    /// Directory or file rendering.
    pub kind: EditorTreeKind,
    /// Nesting level below the workspace root.
    pub depth: usize,
}

impl EditorTreeEntry {
    /// Builds a file entry, deriving the row name from the path basename.
    #[must_use]
    pub fn file(path: impl Into<String>, depth: usize) -> Self {
        let path = path.into();
        let name = basename_of(&path);
        Self {
            path,
            name,
            kind: EditorTreeKind::File,
            depth,
        }
    }

    /// Builds a directory entry, deriving the row name from the path basename.
    #[must_use]
    pub fn directory(path: impl Into<String>, depth: usize) -> Self {
        let path = path.into();
        let name = basename_of(&path);
        Self {
            path,
            name,
            kind: EditorTreeKind::Directory,
            depth,
        }
    }

    /// Returns whether this entry renders as a collapsible directory.
    #[must_use]
    pub const fn is_directory(&self) -> bool {
        matches!(self.kind, EditorTreeKind::Directory)
    }
}

/// One recent-change row for the no-file empty state.
///
/// Both strings arrive caller-formatted: `changed` is the already-formatted
/// relative time, so this leaf performs no clock reads.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorRecentFile {
    /// Workspace-relative file path.
    pub path: String,
    /// Caller-formatted relative change time.
    pub changed: String,
}

impl EditorRecentFile {
    /// Builds a recent-file row from its path and formatted time.
    #[must_use]
    pub fn new(path: impl Into<String>, changed: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            changed: changed.into(),
        }
    }
}

/// Adapter-derived repository facts for the header row.
///
/// The VCS labels are derived from `web_url` with the existing
/// [`crate::vcs_labels`] helpers at render time; the host mark has no native
/// brand-asset mapping yet, so the remote link renders as text and the gap is
/// recorded.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorScreenRepository {
    /// Exact web URL of the selected remote, the link target.
    pub web_url: String,
    /// Configured name of the selected remote (for first-match display).
    pub remote_name: String,
    /// Configured default-remote name used for first-match selection.
    pub default_remote: Option<String>,
    /// Attached branch name, or `None` for a detached `HEAD`.
    pub branch: Option<String>,
}

impl EditorScreenRepository {
    /// Builds repository facts without deriving or normalizing any value.
    #[must_use]
    pub fn new(
        web_url: impl Into<String>,
        remote_name: impl Into<String>,
        default_remote: Option<String>,
        branch: Option<String>,
    ) -> Self {
        Self {
            web_url: web_url.into(),
            remote_name: remote_name.into(),
            default_remote,
            branch,
        }
    }
}

/// The workspace/thread identity owning this screen.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorScreenIdentity {
    /// Authoritative workspace identity (`/e/[workspace]`).
    pub workspace: ProjectId,
    /// Project display name shown in the header row.
    pub display_name: String,
    /// Authoritative thread identity (`/e/[workspace]/[thread]`).
    pub thread: ThreadId,
    /// Open thread title closing the header line, when the route supplies one.
    pub thread_title: Option<String>,
    /// Repository observation for the VCS segments, when inspected.
    pub repository: Option<EditorScreenRepository>,
}

impl EditorScreenIdentity {
    /// Builds the screen identity without deriving or normalizing any value.
    #[must_use]
    pub fn new(
        workspace: ProjectId,
        display_name: impl Into<String>,
        thread: ThreadId,
        thread_title: Option<String>,
        repository: Option<EditorScreenRepository>,
    ) -> Self {
        Self {
            workspace,
            display_name: display_name.into(),
            thread,
            thread_title,
            repository,
        }
    }
}

/// The editor content-column state, mirroring the `editor-route.svelte`
/// branch order: open file, opening skeleton, per-path failure, or the
/// no-file empty state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditorSurfaceState {
    /// No `?file=` query: the mark (no recents) or the recent-changes table.
    NoFile {
        /// Already-formatted recent rows, newest first; capped at
        /// [`RECENT_FILES_MAX`] at render time.
        recent: Vec<EditorRecentFile>,
    },
    /// A read is in flight for this path: skeleton lines.
    Loading {
        /// The path being opened.
        path: String,
    },
    /// The path failed to open: the failure card replaces the surface.
    Failed {
        /// The path that could not be displayed.
        path: String,
        /// Caller-supplied failure message, shown verbatim.
        message: String,
    },
    /// A readable file: read-only content with provider diagnostics.
    Open {
        /// The open workspace-relative path (`?file=`).
        path: String,
        /// Full file text rendered read-only.
        content: String,
        /// Provider diagnostics mapped to markers at render time.
        diagnostics: Vec<EditorDiagnostic>,
    },
}

/// One action emitted by pointer activation.
///
/// The screen never navigates or reads files; the orchestrator consumes the
/// pending action and owns the resulting route or transport work.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditorAction {
    /// Open the file at this workspace-relative path.
    OpenFile {
        /// The activated path.
        path: String,
    },
}
/// Returns the row basename, treating `/` and `\` as separators and ignoring
/// a trailing separator so `"src/"` names its row `"src"`.
fn basename_of(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(path)
        .to_owned()
}

/// Returns the visible entry indices in display order, skipping the children
/// of collapsed directories.
///
/// A directory hides every following entry deeper than itself until an entry
/// at its own depth or shallower reappears. Unknown paths in `expanded` are
/// ignored; entries are never reordered.
#[must_use]
pub fn visible_tree_entries<S: std::hash::BuildHasher>(
    entries: &[EditorTreeEntry],
    expanded: &HashSet<String, S>,
) -> Vec<usize> {
    let mut visible = Vec::with_capacity(entries.len());
    let mut hidden_below: Option<usize> = None;
    for (index, entry) in entries.iter().enumerate() {
        if let Some(depth) = hidden_below {
            if entry.depth > depth {
                continue;
            }
            hidden_below = None;
        }
        visible.push(index);
        if entry.is_directory() && !expanded.contains(&entry.path) {
            hidden_below = Some(entry.depth);
        }
    }
    visible
}

/// Returns whether the directory at `index` has at least one listed direct
/// child. An expanded directory without one renders the loading copy; a
/// collapsed one never asks.
#[must_use]
pub fn directory_has_children(entries: &[EditorTreeEntry], index: usize) -> bool {
    let Some(directory) = entries.get(index) else {
        return false;
    };
    if !directory.is_directory() {
        return false;
    }
    entries
        .get(index.saturating_add(1)..)
        .unwrap_or(&[])
        .iter()
        .take_while(|entry| entry.depth > directory.depth)
        .any(|entry| entry.depth == directory.depth.saturating_add(1))
}

/// Expands tab stops to spaces for read-only rendering.
///
/// `width` is the tab-stop interval; zero is treated as one. Column counting
/// is by Unicode scalar, so double-width glyphs do not align stops exactly;
/// that refinement belongs to a future text engine and is recorded as a gap.
#[must_use]
pub fn expand_editor_tabs(line: &str, width: usize) -> String {
    let width = width.max(1);
    let mut expanded = String::with_capacity(line.len());
    let mut column = 0_usize;
    for character in line.chars() {
        if character == '\t' {
            let spaces = width.saturating_sub(column % width);
            for _ in 0..spaces {
                expanded.push(' ');
            }
            column = column.saturating_add(spaces);
        } else {
            expanded.push(character);
            column = column.saturating_add(1);
        }
    }
    expanded
}

/// Returns the gutter width for a file with `line_count` lines.
///
/// `CodeMirror` gutters size to their content; this fixed rule reserves 12 px of
/// chrome plus 8 px per decimal digit at the 13 px editor size. It is a named
/// approximation, not a measured text width.
#[must_use]
pub fn gutter_width_px(line_count: usize) -> gpui::Pixels {
    let mut digits: u32 = 1;
    let mut remaining = line_count;
    while remaining >= 10 {
        remaining /= 10;
        digits = digits.saturating_add(1);
    }
    #[allow(clippy::cast_precision_loss)]
    let digits = digits as f32;
    px(12.0 + 8.0 * digits)
}

/// Splits content into line-start byte offsets for marker mapping.
fn line_start_offsets(content: &str) -> Vec<usize> {
    let mut starts = vec![0_usize];
    for (index, byte) in content.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(index.saturating_add(1));
        }
    }
    starts
}

/// Returns the index of the last line starting at or before `offset`.
fn line_index_for_offset(starts: &[usize], offset: usize) -> usize {
    starts
        .iter()
        .rposition(|start| *start <= offset)
        .unwrap_or(0)
}

/// Ranks severities so one line keeps its most severe diagnostic.
const fn severity_rank(severity: EditorDiagnosticSeverity) -> u8 {
    match severity {
        EditorDiagnosticSeverity::Error => 4,
        EditorDiagnosticSeverity::Warning => 3,
        EditorDiagnosticSeverity::Info => 2,
        EditorDiagnosticSeverity::Hint => 1,
    }
}

/// Maps already-mapped markers to one winning severity per content line.
///
/// Reversed ranges are already collapsed by [`map_editor_diagnostics`]; ranges
/// spanning lines wash every touched line. Lines without markers stay `None`.
#[must_use]
pub fn diagnostic_severity_per_line(
    content: &str,
    markers: &[MappedMarker],
) -> Vec<Option<EditorDiagnosticSeverity>> {
    let starts = line_start_offsets(content);
    let mut per_line: Vec<Option<EditorDiagnosticSeverity>> = vec![None; starts.len()];
    for marker in markers {
        let from_line = line_index_for_offset(&starts, marker.from);
        let to_line = line_index_for_offset(&starts, marker.to.max(marker.from));
        for line in from_line..=to_line {
            let Some(slot) = per_line.get_mut(line) else {
                continue;
            };
            let incumbent = slot.map_or(0, severity_rank);
            if severity_rank(marker.severity) > incumbent {
                *slot = Some(marker.severity);
            }
        }
    }
    per_line
}

/// Splits a recent path into its row name and parent display.
///
/// Mirrors the legacy `file_name`/`file_parent` helpers: the parent keeps `/`
/// separators (the native display-format preference is not wired here) and an
/// empty parent renders nothing beside the name.
#[must_use]
pub fn split_recent_display(path: &str) -> (String, String) {
    let segments = derive_breadcrumbs(&WorkspaceFileReference {
        path: path.to_owned(),
    });
    let Some((name, parents)) = segments.split_last() else {
        return (path.to_owned(), String::new());
    };
    (name.clone(), parents.join("/"))
}

/// Maps a resolved [`FileIcon`] to its sealed asset-catalog identifier.
#[must_use]
pub const fn asset_for_file_icon(icon: FileIcon) -> AssetId {
    match icon {
        FileIcon::Text => AssetId::JETBRAINS_TEXT,
        FileIcon::TypeScriptTest => AssetId::JETBRAINS_TS_TEST,
        FileIcon::TypeScript => AssetId::JETBRAINS_TYPESCRIPT,
        FileIcon::Svelte => AssetId::JETBRAINS_SVELTE,
    }
}

/// Applies an alpha to a theme color while keeping its hue and lightness.
fn alpha_paint(base: Oklch, alpha: f32) -> Hsla {
    let mut paint = base.to_paint();
    paint.alpha = alpha;
    paint
}

/// Resolves the row-wash color for one diagnostic severity.
fn severity_wash(theme: &ArtisanTheme, severity: EditorDiagnosticSeverity) -> Hsla {
    let base = match severity {
        EditorDiagnosticSeverity::Error => theme.colors.destructive,
        EditorDiagnosticSeverity::Warning => theme.colors.banner_warning,
        EditorDiagnosticSeverity::Info => theme.colors.banner_info,
        EditorDiagnosticSeverity::Hint => theme.colors.muted_foreground,
    };
    alpha_paint(base, DIAGNOSTIC_WASH_ALPHA)
}
/// The native `/e/[workspace]/[thread]` screen: header row, file-tree
/// inspector column, and editor surface column.
///
/// The constructor takes the workspace/thread identity, the injected file
/// list, the surface state, and the retained editor view; rendering follows
/// `routes/+layout.svelte` composition with `editor-route.svelte` content
/// branches. Mount with `cx.new(EditorScreen::new(...))` from the
/// `NativeRoute::Editor` branch once the route is wired.
pub struct EditorScreen {
    identity: EditorScreenIdentity,
    files: Vec<EditorTreeEntry>,
    expanded: HashSet<String>,
    surface: EditorSurfaceState,
    view: RetainedEditorViewState,
    theme: ArtisanTheme,
    surface_scroll: ScrollHandle,
    pending_action: Option<EditorAction>,
}

impl EditorScreen {
    /// Builds the screen over injected route state.
    ///
    /// Directories start collapsed, matching the legacy tree. The retained
    /// view supplies the active-line highlight; its scroll offset is retained
    /// for the orchestrator but GPUI scroll restoration is deferred (gap).
    #[must_use]
    pub fn new(
        identity: EditorScreenIdentity,
        files: Vec<EditorTreeEntry>,
        surface: EditorSurfaceState,
        view: RetainedEditorViewState,
        theme: ArtisanTheme,
    ) -> Self {
        Self {
            identity,
            files,
            expanded: HashSet::new(),
            surface,
            view,
            theme,
            surface_scroll: ScrollHandle::new(),
            pending_action: None,
        }
    }

    /// Returns the workspace/thread identity owning this screen.
    #[must_use]
    pub const fn identity(&self) -> &EditorScreenIdentity {
        &self.identity
    }

    /// Returns the injected file list in display order.
    #[must_use]
    pub fn files(&self) -> &[EditorTreeEntry] {
        &self.files
    }

    /// Returns the current surface state.
    #[must_use]
    pub const fn surface(&self) -> &EditorSurfaceState {
        &self.surface
    }

    /// Returns the retained editor view (active line and scroll offset).
    #[must_use]
    pub const fn retained_view(&self) -> &RetainedEditorViewState {
        &self.view
    }

    /// Returns the pending activation without consuming it.
    #[must_use]
    pub const fn pending_action(&self) -> Option<&EditorAction> {
        self.pending_action.as_ref()
    }

    /// Returns and clears the one action waiting for orchestrator observation.
    pub fn take_pending_action(&mut self) -> Option<EditorAction> {
        self.pending_action.take()
    }

    /// Replaces the injected file list. Expansion for surviving paths is kept;
    /// unknown expanded paths are ignored at render time.
    pub fn set_files(&mut self, files: Vec<EditorTreeEntry>, cx: &mut Context<Self>) {
        self.files = files;
        cx.notify();
    }

    /// Replaces the surface state (open, loading, failure, or empty).
    pub fn set_surface(&mut self, surface: EditorSurfaceState, cx: &mut Context<Self>) {
        self.surface = surface;
        cx.notify();
    }

    /// Replaces the retained editor view.
    pub fn set_view(&mut self, view: RetainedEditorViewState, cx: &mut Context<Self>) {
        self.view = view;
        cx.notify();
    }

    /// Replaces the resolved theme (for mode changes).
    pub fn set_theme(&mut self, theme: ArtisanTheme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    /// Builds the canonical editor URL for `file`, reusing
    /// [`editor_route_path`] so deep links match the web route exactly.
    /// `None` builds the bare editor URL without a `?file=` query.
    #[must_use]
    pub fn href_for_file(&self, file: Option<&str>) -> String {
        editor_route_path(
            self.identity.workspace.as_str(),
            self.identity.thread.as_str(),
            file,
        )
    }

    /// Returns the open file's path for tree-row selection, if any file state
    /// names one (open, loading, or failed).
    #[must_use]
    pub fn open_path(&self) -> Option<&str> {
        match &self.surface {
            EditorSurfaceState::NoFile { .. } => None,
            EditorSurfaceState::Loading { path }
            | EditorSurfaceState::Failed { path, .. }
            | EditorSurfaceState::Open { path, .. } => Some(path),
        }
    }

    /// Resolves the open file's editor language for future highlighting.
    /// Files without an open path resolve to plaintext; the read-only text
    /// itself is currently unhighlighted (recorded gap).
    #[must_use]
    pub fn open_language(&self) -> EditorLanguageId {
        match &self.surface {
            EditorSurfaceState::Open { path, .. } => editor_language_for_path(path, None),
            EditorSurfaceState::NoFile { .. }
            | EditorSurfaceState::Loading { .. }
            | EditorSurfaceState::Failed { .. } => EditorLanguageId::Plaintext,
        }
    }

    /// Toggles one directory's expansion. Directories without listed children
    /// expand into the loading copy until the orchestrator refills the list.
    fn toggle_directory(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.expanded.remove(path) {
            cx.notify();
            return;
        }
        self.expanded.insert(path.to_owned());
        cx.notify();
    }

    /// Records an open-file activation for orchestrator consumption.
    fn request_open(&mut self, path: String, cx: &mut Context<Self>) {
        self.pending_action = Some(EditorAction::OpenFile { path });
        cx.notify();
    }

    /// Renders one workspace-header segment from the shell packet's public
    /// presentation API. Icons inherit the row's muted color (`currentColor`
    /// in legacy); the remote link has no native brand-mark mapping yet, so
    /// it renders as banner-info text and the shortfall is recorded as a gap.
    fn render_header_segment(&self, segment: &WorkspaceHeaderSegment<'_, ()>) -> AnyElement {
        let theme = self.theme;
        match segment {
            WorkspaceHeaderSegment::Folder { label } => div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(icon(IconStyle::resolve(
                    theme,
                    AssetId::TABLER_FOLDER,
                    IconSize::Compact,
                    IconTint::Inherit,
                )))
                .child(SharedString::from(label.to_string()))
                .into_any_element(),
            WorkspaceHeaderSegment::RemoteLink { link } => div()
                .text_color(theme.colors.banner_info.to_paint())
                .child(SharedString::from(link.qualified_label.to_string()))
                .into_any_element(),
            WorkspaceHeaderSegment::Text(text) => div()
                .child(SharedString::from(text.as_str().to_owned()))
                .into_any_element(),
            WorkspaceHeaderSegment::Branch { branch } => div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(icon(IconStyle::resolve(
                    theme,
                    AssetId::TABLER_GIT_BRANCH,
                    IconSize::Compact,
                    IconTint::Inherit,
                )))
                .child(SharedString::from(branch.label().to_owned()))
                .into_any_element(),
            WorkspaceHeaderSegment::Checkout { label } => div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .child(icon(IconStyle::resolve(
                    theme,
                    AssetId::TABLER_FOLDER_CODE,
                    IconSize::Compact,
                    IconTint::Inherit,
                )))
                .child(SharedString::from(label.to_string()))
                .into_any_element(),
            WorkspaceHeaderSegment::ThreadTitle { title } => div()
                .flex_1()
                .min_w(px(0.0))
                .truncate()
                .text_color(theme.colors.foreground.to_paint())
                .child(SharedString::from(title.to_string()))
                .into_any_element(),
        }
    }

    /// Renders the header row by projecting owned facts through the shell
    /// packet's [`present_workspace_header`] policy. Absent repository
    /// inspection yields the folder presentation, exactly as the policy
    /// specifies for the pre-inspection native state.
    fn render_header(&self) -> Div {
        let theme = self.theme;
        let link_labels: Option<(String, String)> =
            self.identity.repository.as_ref().map(|repository| {
                (
                    repository_link_label(&repository.web_url),
                    repository_qualified_label(&repository.web_url),
                )
            });
        let remote_link: Option<WorkspaceHeaderRemoteLink<'_, ()>> =
            match (self.identity.repository.as_ref(), link_labels.as_ref()) {
                (Some(repository), Some((link_label, qualified_label))) => {
                    Some(WorkspaceHeaderRemoteLink::new(
                        &repository.web_url,
                        link_label,
                        qualified_label,
                        (),
                    ))
                }
                _ => None,
            };
        let remote: Option<WorkspaceHeaderRemote<'_, ()>> =
            match (self.identity.repository.as_ref(), remote_link.as_ref()) {
                (Some(repository), Some(link)) => Some(WorkspaceHeaderRemote::new(
                    &repository.remote_name,
                    Some(link.clone()),
                )),
                (Some(repository), None) => {
                    Some(WorkspaceHeaderRemote::without_link(&repository.remote_name))
                }
                (None, _) => None,
            };
        let remotes: &[WorkspaceHeaderRemote<'_, ()>] = remote.as_slice();
        let branch = self.identity.repository.as_ref().map(|repository| {
            repository.branch.as_deref().map_or(
                WorkspaceHeaderBranch::DetachedHead,
                WorkspaceHeaderBranch::Named,
            )
        });
        let inspection: Option<WorkspaceHeaderRepositoryInspection<'_, ()>> =
            match (self.identity.repository.as_ref(), branch) {
                (Some(repository), Some(branch)) => {
                    Some(WorkspaceHeaderRepositoryInspection::repository(
                        WorkspaceHeaderRepository::new(
                            branch,
                            repository.default_remote.as_deref(),
                            remotes,
                        ),
                    ))
                }
                _ => None,
            };
        let presentation = present_workspace_header(WorkspaceHeaderInput::new(
            Some(self.identity.display_name.as_str()),
            inspection,
            self.identity.thread_title.as_deref(),
        ));

        let mut row = div()
            .flex()
            .items_center()
            .gap(px(6.0))
            .text_size(theme.typography.control_text)
            .text_color(theme.colors.muted_foreground.to_paint())
            .debug_selector(|| EDITOR_HEADER_SELECTOR.to_owned());
        if let Some(presentation) = presentation {
            for segment in presentation.segments() {
                row = row.child(self.render_header_segment(segment));
            }
        }
        row
    }
    /// Renders one visible tree row, dispatching to the directory or file row.
    fn render_tree_row(
        &self,
        visible_index: usize,
        entry_index: usize,
        cx: &Context<Self>,
    ) -> AnyElement {
        let Some(entry) = self.files.get(entry_index) else {
            return div().into_any_element();
        };
        let row_id =
            SharedString::from(format!("{EDITOR_TREE_ROW_SELECTOR_PREFIX}-{visible_index}"));
        if entry.is_directory() {
            return self.render_directory_row(entry, entry_index, row_id, cx);
        }
        self.render_file_row(entry, row_id, cx)
    }

    /// Renders one directory row plus its expanded-children loading copy.
    fn render_directory_row(
        &self,
        entry: &EditorTreeEntry,
        entry_index: usize,
        row_id: SharedString,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let selector = row_id.to_string();
        #[allow(clippy::cast_precision_loss)]
        let indent_px = TREE_INDENT_BASE_PX + entry.depth as f32 * TREE_INDENT_DEPTH_PX;
        let muted = theme.colors.muted_foreground.to_paint();
        {
            let expanded = self.expanded.contains(&entry.path);
            // Legacy rotates `chevron-right` 90° on expansion; GPUI has no
            // rotate primitive on glyphs, so the catalog's `chevron-down`
            // variant carries the expanded state instead.
            let chevron = if expanded {
                AssetId::TABLER_CHEVRON_DOWN
            } else {
                AssetId::TABLER_CHEVRON_RIGHT
            };
            let folder = if expanded {
                AssetId::TABLER_FOLDER_OPEN
            } else {
                AssetId::TABLER_FOLDER
            };
            let path = entry.path.clone();
            let row = div()
                .id(row_id)
                .debug_selector(move || selector.clone())
                .flex()
                .items_center()
                .gap(px(6.0))
                .w_full()
                .min_w(px(0.0))
                .rounded(px(8.0))
                .py(px(4.0))
                .pr(px(8.0))
                .pl(px(indent_px))
                .text_size(theme.typography.control_text)
                .text_color(muted)
                .on_click(cx.listener(move |screen, _: &ClickEvent, _, cx| {
                    screen.toggle_directory(&path, cx);
                }))
                .child(icon(IconStyle::resolve(
                    theme,
                    chevron,
                    IconSize::Compact,
                    IconTint::Inherit,
                )))
                .child(icon(IconStyle::resolve(
                    theme,
                    folder,
                    IconSize::Compact,
                    IconTint::Inherit,
                )))
                .child(
                    div()
                        .min_w(px(0.0))
                        .truncate()
                        .child(SharedString::from(entry.name.clone())),
                )
                .into_any_element();
            if expanded && !directory_has_children(&self.files, entry_index) {
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .min_w(px(0.0))
                    .child(row)
                    .child(
                        div()
                            .py(px(4.0))
                            .pl(px(indent_px + TREE_FILE_EXTRA_PX + TREE_INDENT_DEPTH_PX))
                            .text_size(theme.typography.label_text)
                            .text_color(muted)
                            .child(TREE_LOADING_TEXT),
                    )
                    .into_any_element()
            } else {
                row
            }
        }
    }

    /// Renders one file row, highlighted when it names the open path.
    fn render_file_row(
        &self,
        entry: &EditorTreeEntry,
        row_id: SharedString,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = self.theme;
        let selector = row_id.to_string();
        #[allow(clippy::cast_precision_loss)]
        let indent_px = TREE_INDENT_BASE_PX + entry.depth as f32 * TREE_INDENT_DEPTH_PX;
        let muted = theme.colors.muted_foreground.to_paint();
        let active = self.open_path() == Some(entry.path.as_str());
        let path = entry.path.clone();
        div()
            .id(row_id)
            .debug_selector(move || selector.clone())
            .flex()
            .items_center()
            .gap(px(6.0))
            .w_full()
            .min_w(px(0.0))
            .rounded(px(8.0))
            .py(px(4.0))
            .pr(px(8.0))
            .pl(px(indent_px + TREE_FILE_EXTRA_PX))
            .text_size(theme.typography.control_text)
            .text_color(if active {
                theme.colors.foreground.to_paint()
            } else {
                muted
            })
            .on_click(cx.listener(move |screen, _: &ClickEvent, _, cx| {
                screen.request_open(path.clone(), cx);
            }))
            .child(icon(IconStyle::resolve(
                theme,
                asset_for_file_icon(resolve_file_icon(&entry.path)),
                IconSize::Default,
                IconTint::Inherit,
            )))
            .child(
                div()
                    .min_w(px(0.0))
                    .truncate()
                    .child(SharedString::from(entry.name.clone())),
            )
            .into_any_element()
    }

    /// Renders the inspector-column file panel.
    fn render_file_panel(&self, cx: &Context<Self>) -> Div {
        let theme = self.theme;
        let muted = theme.colors.muted_foreground.to_paint();
        let mut list = div()
            .id("editor-file-panel-list")
            .flex_1()
            .min_h(px(0.0))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .min_w(px(0.0));
        if self.files.is_empty() {
            list = list.child(
                div()
                    .px(px(4.0))
                    .text_size(theme.typography.label_text)
                    .text_color(muted)
                    .child(EMPTY_TREE_TEXT),
            );
        } else {
            for (visible_index, entry_index) in visible_tree_entries(&self.files, &self.expanded)
                .iter()
                .enumerate()
            {
                list = list.child(self.render_tree_row(visible_index, *entry_index, cx));
            }
        }
        div()
            .w(px(FILE_PANEL_WIDTH_PX))
            .flex_shrink_0()
            .h_full()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .p(px(12.0))
            .debug_selector(|| EDITOR_FILE_PANEL_SELECTOR.to_owned())
            .child(
                div()
                    .px(px(4.0))
                    .text_size(theme.typography.label_text)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(muted)
                    .child(FILE_PANEL_TITLE),
            )
            .child(list)
    }

    /// Renders the no-file empty state: the product mark when the thread
    /// touched nothing yet, otherwise the recent-changes table capped at
    /// [`RECENT_FILES_MAX`] rows.
    fn render_empty(&self, recent: &[EditorRecentFile], cx: &Context<Self>) -> Div {
        let theme = self.theme;
        let muted = theme.colors.muted_foreground.to_paint();
        if recent.is_empty() {
            return div()
                .flex_1()
                .min_h(px(0.0))
                .flex()
                .items_center()
                .justify_center()
                .p(px(24.0))
                .debug_selector(|| EDITOR_EMPTY_SELECTOR.to_owned())
                .child(
                    asset_glyph(AssetId::ARTISAN_LOGO_GRADIENT)
                        .size(px(EMPTY_MARK_PX))
                        .opacity(0.6),
                );
        }
        let mut rows = div()
            .flex()
            .flex_col()
            .w_full()
            .min_w(px(0.0))
            .debug_selector(|| EDITOR_RECENTS_SELECTOR.to_owned());
        for (index, file) in recent.iter().take(RECENT_FILES_MAX).enumerate() {
            let (name, parent) = split_recent_display(&file.path);
            let path = file.path.clone();
            let row_id = SharedString::from(format!("{EDITOR_EMPTY_SELECTOR}-recent-{index}"));
            let selector = row_id.to_string();
            rows = rows.child(
                div()
                    .id(row_id)
                    .debug_selector(move || selector.clone())
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .w_full()
                    .min_w(px(0.0))
                    .py(px(12.0))
                    .on_click(cx.listener(move |screen, _: &ClickEvent, _, cx| {
                        screen.request_open(path.clone(), cx);
                    }))
                    .child(icon(IconStyle::resolve(
                        theme,
                        asset_for_file_icon(resolve_file_icon(&file.path)),
                        IconSize::Default,
                        IconTint::Inherit,
                    )))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_size(theme.typography.control_text)
                            .text_color(theme.colors.foreground.to_paint())
                            .child(SharedString::from(name)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .truncate()
                            .text_size(theme.typography.label_text)
                            .text_color(muted)
                            .child(SharedString::from(parent)),
                    )
                    .child(
                        div()
                            .w(px(112.0))
                            .flex_shrink_0()
                            .text_size(theme.typography.label_text)
                            .text_color(muted)
                            .child(SharedString::from(file.changed.clone())),
                    ),
            );
        }
        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .items_center()
            .justify_center()
            .p(px(24.0))
            .debug_selector(|| EDITOR_EMPTY_SELECTOR.to_owned())
            .child(
                div()
                    .w_full()
                    .min_w(px(0.0))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .mb(px(8.0))
                            .text_size(theme.typography.label_text)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(muted)
                            .child(RECENT_FILES_TITLE),
                    )
                    .child(rows),
            )
    }
    /// Renders the opening skeleton column: five muted lines at the legacy
    /// fractional widths.
    fn render_loading(&self) -> Div {
        let style = SkeletonStyle::resolve(self.theme);
        let mut column = div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .flex_col()
            .gap(px(12.0))
            .p(px(24.0))
            .debug_selector(|| EDITOR_LOADING_SELECTOR.to_owned());
        for fraction in LOADING_SKELETON_FRACTIONS {
            column = column.child(
                skeleton(style, MotionPolicy::Reduced)
                    .h(px(16.0))
                    .w(relative(fraction)),
            );
        }
        column
    }

    /// Renders the per-path failure card. The surface stays unmounted so no
    /// stale document shows beneath the error, matching legacy. The legacy
    /// `FileOff` renders at `size-6` (24 px); the shared icon recipe caps at
    /// 16 px, so the recipe maximum is used and the shortfall is recorded.
    fn render_failure(&self, message: &str, path: &str) -> Div {
        let theme = self.theme;
        div()
            .flex_1()
            .min_h(px(0.0))
            .flex()
            .items_center()
            .justify_center()
            .p(px(24.0))
            .debug_selector(|| EDITOR_FAILURE_SELECTOR.to_owned())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(8.0))
                    .min_w(px(0.0))
                    .child(icon(IconStyle::resolve(
                        theme,
                        AssetId::TABLER_FILE_OFF,
                        IconSize::Default,
                        IconTint::Inherit,
                    )))
                    .child(
                        div()
                            .text_size(theme.typography.control_text)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.colors.foreground.to_paint())
                            .child(FILE_FAILURE_TITLE),
                    )
                    .child(
                        div()
                            .text_size(theme.typography.label_text)
                            .text_color(theme.colors.muted_foreground.to_paint())
                            .child(SharedString::from(message.to_owned())),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(theme.typography.label_text)
                            .text_color(alpha_paint(
                                theme.colors.muted_foreground,
                                GUTTER_TEXT_ALPHA,
                            ))
                            .child(SharedString::from(path.to_owned())),
                    ),
            )
    }

    /// Renders read-only file content as gutter-plus-text rows.
    ///
    /// Tabs are pre-expanded, empty lines keep their height with a no-break
    /// space, and the retained view's cursor line carries the active-line wash
    /// with its brighter gutter number. Diagnostic ranges wash their lines in
    /// the severity color. Horizontal scrolling mirrors `CodeMirror`'s unwrapped
    /// lines; syntax highlighting and editing are recorded gaps.
    fn render_open(&self, content: &str, diagnostics: &[EditorDiagnostic]) -> Stateful<Div> {
        let theme = self.theme;
        let foreground = theme.colors.foreground.to_paint();
        let markers = map_editor_diagnostics(content, diagnostics);
        let severities = diagnostic_severity_per_line(content, &markers);
        let lines: Vec<&str> = content.split('\n').collect();
        let gutter = gutter_width_px(lines.len());
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let active_line = self.view.cursor_line.max(1).saturating_sub(1) as usize;

        let mut column = div().flex().flex_col().w_full().min_w(px(0.0)).py(px(8.0));
        for (index, line) in lines.iter().enumerate() {
            let expanded = expand_editor_tabs(line, EDITOR_TAB_WIDTH);
            let display = if expanded.is_empty() {
                "\u{a0}".to_owned()
            } else {
                expanded
            };
            let gutter_color = if index == active_line {
                alpha_paint(theme.colors.foreground, ACTIVE_GUTTER_TEXT_ALPHA)
            } else {
                alpha_paint(theme.colors.foreground, GUTTER_TEXT_ALPHA)
            };
            let mut row = div().flex().flex_row().w_full().min_w(px(0.0));
            if index == active_line {
                row = row.bg(alpha_paint(theme.colors.foreground, ACTIVE_LINE_ALPHA));
            } else if let Some(Some(severity)) = severities.get(index) {
                row = row.bg(severity_wash(&theme, *severity));
            }
            row = row
                .child(
                    div()
                        .w(gutter)
                        .flex_shrink_0()
                        .flex()
                        .justify_end()
                        .pr(px(12.0))
                        .text_size(px(EDITOR_TEXT_PX))
                        .line_height(px(EDITOR_LINE_HEIGHT_PX))
                        .text_color(gutter_color)
                        .child(SharedString::from(format!("{}", index.saturating_add(1)))),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .whitespace_nowrap()
                        .text_size(px(EDITOR_TEXT_PX))
                        .line_height(px(EDITOR_LINE_HEIGHT_PX))
                        .text_color(foreground)
                        .child(SharedString::from(display)),
                );
            column = column.child(row);
        }
        div()
            .id("editor-surface-scroll")
            .flex_1()
            .min_h(px(0.0))
            .min_w(px(0.0))
            .overflow_y_scroll()
            .overflow_x_scroll()
            .track_scroll(&self.surface_scroll)
            .debug_selector(|| EDITOR_SURFACE_SELECTOR.to_owned())
            .child(column)
    }

    /// Renders the surface column across the four content branches.
    fn render_surface_column(&self, cx: &Context<Self>) -> Div {
        let body: AnyElement = match &self.surface {
            EditorSurfaceState::NoFile { recent } => {
                self.render_empty(recent, cx).into_any_element()
            }
            EditorSurfaceState::Loading { .. } => self.render_loading().into_any_element(),
            EditorSurfaceState::Failed { path, message } => {
                self.render_failure(message, path).into_any_element()
            }
            EditorSurfaceState::Open {
                content,
                diagnostics,
                ..
            } => self.render_open(content, diagnostics).into_any_element(),
        };
        div()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .h_full()
            .flex()
            .flex_col()
            .child(body)
    }
}

impl Render for EditorScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("editor-screen-root")
            .flex()
            .flex_col()
            .size_full()
            .bg(self.theme.colors.background.to_paint())
            .debug_selector(|| EDITOR_SCREEN_SELECTOR.to_owned())
            .child(self.render_header())
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.0))
                    .min_w(px(0.0))
                    .w_full()
                    .flex()
                    .flex_row()
                    .child(self.render_file_panel(cx))
                    .child(self.render_surface_column(cx)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_identity() -> EditorScreenIdentity {
        EditorScreenIdentity::new(
            ProjectId::parse("workspace-1").expect("static test id is valid"),
            "workspace-1",
            ThreadId::parse("thread_abc").expect("static test id is valid"),
            None,
            None,
        )
    }

    fn test_theme() -> ArtisanTheme {
        ArtisanTheme::for_mode(artisan_ui::theme::ThemeMode::Dark)
    }

    fn test_screen(surface: EditorSurfaceState) -> EditorScreen {
        EditorScreen::new(
            test_identity(),
            Vec::new(),
            surface,
            RetainedEditorViewState::default(),
            test_theme(),
        )
    }

    #[test]
    fn tree_entry_names_come_from_basename() {
        assert_eq!(EditorTreeEntry::file("src/main.rs", 1).name, "main.rs");
        assert_eq!(
            EditorTreeEntry::directory("src/components/", 1).name,
            "components"
        );
        assert_eq!(EditorTreeEntry::file("lonely", 0).name, "lonely");
    }

    #[test]
    fn collapsed_directories_hide_their_children() {
        let entries = vec![
            EditorTreeEntry::directory("src", 0),
            EditorTreeEntry::file("src/main.rs", 1),
            EditorTreeEntry::directory("src/nested", 1),
            EditorTreeEntry::file("src/nested/deep.rs", 2),
            EditorTreeEntry::file("README.md", 0),
        ];
        let expanded = HashSet::new();
        assert_eq!(visible_tree_entries(&entries, &expanded), vec![0, 4]);

        let expanded: HashSet<String> = ["src".to_owned()].into();
        assert_eq!(visible_tree_entries(&entries, &expanded), vec![0, 1, 2, 4]);
    }

    #[test]
    fn directory_children_detection_ignores_non_direct_rows() {
        let entries = vec![
            EditorTreeEntry::directory("src", 0),
            EditorTreeEntry::file("src/main.rs", 1),
            EditorTreeEntry::file("top.rs", 0),
        ];
        assert!(directory_has_children(&entries, 0));
        assert!(!directory_has_children(&entries, 1));
        assert!(!directory_has_children(&entries, 2));
        assert!(!directory_has_children(&entries, 99));
    }

    #[test]
    fn tabs_expand_to_stop_intervals() {
        assert_eq!(expand_editor_tabs("a\tb", 4), "a   b");
        assert_eq!(expand_editor_tabs("\t", 4), "    ");
        assert_eq!(expand_editor_tabs("no tabs", 4), "no tabs");
        assert_eq!(expand_editor_tabs("a\tb", 0), "a b");
    }

    #[test]
    fn gutter_grows_with_digit_count() {
        assert_eq!(gutter_width_px(9), px(20.0));
        assert_eq!(gutter_width_px(10), px(28.0));
        assert_eq!(gutter_width_px(1000), px(44.0));
    }

    #[test]
    fn diagnostic_lines_keep_the_worst_severity() {
        let content = "one\ntwo\nthree\n";
        let markers = map_editor_diagnostics(
            content,
            &[
                EditorDiagnostic::new("hint", EditorDiagnosticSeverity::Hint, 2, 1, 3, 2),
                EditorDiagnostic::new("boom", EditorDiagnosticSeverity::Error, 2, 2, 2, 4),
            ],
        );
        let per_line = diagnostic_severity_per_line(content, &markers);
        assert_eq!(per_line.len(), 4);
        assert_eq!(per_line[0], None);
        assert_eq!(per_line[1], Some(EditorDiagnosticSeverity::Error));
        assert_eq!(per_line[2], Some(EditorDiagnosticSeverity::Hint));
    }

    #[test]
    fn recent_display_splits_name_and_parent() {
        assert_eq!(
            split_recent_display("src/main.rs"),
            ("main.rs".to_owned(), "src".to_owned())
        );
        assert_eq!(
            split_recent_display("top.rs"),
            ("top.rs".to_owned(), String::new())
        );
    }

    #[test]
    fn hrefs_match_the_canonical_editor_route() {
        let screen = test_screen(EditorSurfaceState::NoFile { recent: Vec::new() });
        assert_eq!(
            screen.href_for_file(Some("src/main.rs")),
            "/e/workspace-1/abc?file=src%2Fmain.rs"
        );
        assert_eq!(screen.href_for_file(None), "/e/workspace-1/abc");
        assert_eq!(screen.open_path(), None);
        assert_eq!(screen.open_language(), EditorLanguageId::Plaintext);
    }

    #[test]
    fn open_state_reports_path_and_language() {
        let screen = test_screen(EditorSurfaceState::Open {
            path: "main.rs".to_owned(),
            content: "fn main() {}".to_owned(),
            diagnostics: Vec::new(),
        });
        assert_eq!(screen.open_path(), Some("main.rs"));
        assert_eq!(screen.open_language(), EditorLanguageId::Rust);
    }

    #[test]
    fn file_icons_resolve_to_catalog_assets() {
        assert_eq!(
            asset_for_file_icon(resolve_file_icon("a.test.ts")),
            AssetId::JETBRAINS_TS_TEST
        );
        assert_eq!(
            asset_for_file_icon(resolve_file_icon("view.svelte")),
            AssetId::JETBRAINS_SVELTE
        );
        assert_eq!(
            asset_for_file_icon(resolve_file_icon("notes.md")),
            AssetId::JETBRAINS_TEXT
        );
    }
}
