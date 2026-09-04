//! Native GPUI thread route screen (`/t/[workspace]/[thread]`).
//!
//! Direct translation of the legacy conversation surface into GPUI:
//!
//! - `routes/t/[workspace]/[thread]/+page.svelte` scopes the route by
//!   `workspace:thread` (`{#key ...}`) and mounts
//!   `routes/components/thread-route-gate.svelte`. Gate precedence
//!   (opened route > loading > failure retry) is reused from
//!   [`crate::thread_route_gate_policy::thread_route_gate_render`]; the
//!   loading mark reuses the shared [`FadeArc`] element, which is the same
//!   component the legacy gate renders (`size-6 text-muted-foreground`).
//! - `routes/components/thread-route.svelte` is controller plumbing around
//!   one headline visual fact: the document title renders
//!   `thread_display_title`. The header row here renders that same policy via
//!   [`crate::thread_title_policy::thread_display_title`].
//! - `routes/components/thread-workspace.svelte` is the screen frame this
//!   view follows in order: `main.relative.h-full.min-h-0.overflow-hidden`
//!   holding the transcript column
//!   (`div.prose-column.w-full.max-w-(--prose-width).px-6.pt-10` wrapping
//!   `div.flex.flex-col.gap-8` of turn sections) with the turn navigator and
//!   jump-to-latest controls, and the composer docked at the bottom. The
//!   transcript itself — the whole `conversation-*.svelte` item family,
//!   navigator rail, and jump-to-latest affordance — is the already-ported
//!   [`ConversationHost`]/[`ConversationSurface`] tree, which this screen
//!   mounts as its transcript column rather than re-implementing.
//! - `routes/components/thread-panel.svelte` (mounted by the shell as the
//!   inspector column for a thread surface) contributes the environment card
//!   (`thread-environment-card.svelte`), the terminals card
//!   (`thread-terminals-card.svelte` + `thread-terminals.svelte` rows), and
//!   the checklist card. Row content is projected through the already-ported
//!   [`crate::thread_environment_presentation`],
//!   [`crate::terminal_presentation`], and [`crate::thread_panel_policy`]
//!   policies.
//!
//! The composer is packet 2's [`NativeComposer`](crate::native_composer)
//! entity, consumed as-is and docked at the bottom of the frame. Anything
//! this screen cannot honestly render yet (project selector row, row icons,
//! remote chip, terminal tail viewer, `LipCard` overlay chrome) is listed in
//! the report as a gap, not faked.

#![forbid(unsafe_code)]

use std::rc::Rc;

use artisan_domain::ThreadId;
use artisan_ui::button::{Button, ButtonContent, ButtonSize, ButtonVariant, FocusVisibility};
use artisan_ui::card::{CardStyle, compact_card, compact_card_content};
use artisan_ui::fade_arc::FadeArc;
use artisan_ui::motion::MotionPolicy;
use artisan_ui::theme::{ArtisanTheme, ThemeMode};
use gpui::{
    App, AppContext as _, Context, Entity, FocusHandle, FontWeight, Hsla, IntoElement, Render,
    SharedString, Window, div,
    prelude::{InteractiveElement as _, ParentElement as _, Styled as _},
    px, rgb,
};

use crate::conversation_host::{ConversationHost, ConversationHostError};
use crate::native_composer::NativeComposer;
use crate::terminal_presentation::{
    TerminalSession, TerminalState, terminal_command_line, terminal_display_name,
};
use crate::thread_environment_presentation::{ThreadEnvironmentInput, present_thread_environment};
use crate::thread_panel_policy::{ChecklistEntry, ChecklistEntryState, present_checklist_entry};
use crate::thread_route_gate_policy::{ThreadRouteGateRender, thread_route_gate_render};
use crate::thread_title_policy::{ThreadTitleInput, ThreadTitleMode, thread_display_title};

/// Stable debug selector for the thread screen root.
pub const THREAD_SCREEN_SELECTOR: &str = "artisan-thread-screen";

/// Stable debug selector for the gate loading indicator.
pub const THREAD_SCREEN_LOADING_SELECTOR: &str = "artisan-thread-screen-loading";

/// Stable debug selector for the gate failure branch.
pub const THREAD_SCREEN_FAILURE_SELECTOR: &str = "artisan-thread-screen-failure";

/// Stable debug selector for the gate retry control.
pub const THREAD_SCREEN_RETRY_SELECTOR: &str = "artisan-thread-screen-retry";

/// Stable debug selector for the transcript column.
pub const THREAD_SCREEN_TRANSCRIPT_SELECTOR: &str = "artisan-thread-screen-transcript";

/// Stable debug selector for the empty-transcript state.
pub const THREAD_SCREEN_EMPTY_SELECTOR: &str = "artisan-thread-screen-empty";

/// Stable debug selector for the inspector column.
pub const THREAD_SCREEN_INSPECTOR_SELECTOR: &str = "artisan-thread-screen-inspector";

/// Stable debug selector for the composer dock.
pub const THREAD_SCREEN_COMPOSER_SELECTOR: &str = "artisan-thread-screen-composer-dock";

/// `--prose-width: 48rem` (`lib/styles/theme.css:157`); `1rem` is `16px`.
const PROSE_WIDTH_PX: f32 = 768.0;

/// `px-6` on the transcript column and composer dock.
const COLUMN_PAD_X_PX: f32 = 24.0;

/// `pt-10` on the transcript column.
const TRANSCRIPT_PAD_TOP_PX: f32 = 40.0;

/// `pb-4` under the composer dock (`sm:pb-6` is a viewport refinement GPUI
/// does not express; the base value is kept and noted).
const COMPOSER_PAD_BOTTOM_PX: f32 = 16.0;

/// `max-w-md` on the gate failure column.
const FAILURE_MAX_WIDTH_PX: f32 = 448.0;

/// `p-1` on the inspector column root.
const INSPECTOR_PAD_PX: f32 = 4.0;

/// `gap-4` between inspector cards (`thread-panel.svelte` hover-pill group).
const INSPECTOR_GAP_PX: f32 = 16.0;

/// Inspector column width. Legacy reserves
/// `w-[calc(clamp(16rem,25vw,350px)+1rem)]` in the shell row; GPUI has no
/// container-relative clamp, so the midpoint is pinned and named here.
const INSPECTOR_WIDTH_PX: f32 = 320.0;

/// `px-2 py-2` on inspector rows and card headings.
const ROW_PAD_PX: f32 = 8.0;

/// `gap-2` inside inspector rows.
const ROW_GAP_PX: f32 = 8.0;

/// `max-w-36` on the environment row values.
const ENV_VALUE_MAX_WIDTH_PX: f32 = 144.0;

/// `text-emerald-400` on the added-lines count
/// (`thread-environment-card.svelte` Changes row); no theme token exists.
const ADDED_LINES_GREEN: u32 = 0x0034_D399;

/// `text-red-400` on the deleted-lines count; no theme token exists.
const DELETED_LINES_RED: u32 = 0x00F8_7171;

/// Legacy `−` (U+2212) minus on the deleted-lines count.
const MINUS_SIGN: char = '\u{2212}';

/// Which legacy gate branch the screen renders.
///
/// These are the `thread-route-gate.svelte` branches. Render precedence
/// itself stays in [`thread_route_gate_render`]; [`ThreadScreenGate::presence`]
/// projects the presence triple that function consumes, so the policy keeps
/// sole ownership of the branch order.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ThreadScreenGate {
    /// Cold load in flight; renders the centered `FadeArc` mark.
    ///
    /// A fresh mount has no thread-open snapshot yet, so the gate opens on
    /// its cold-load branch exactly like `thread-route-gate.svelte` with
    /// `thread_open === undefined`.
    #[default]
    Loading,
    /// Thread-open snapshot arrived; renders the route frame.
    Open,
    /// Load failed; renders the message with the retry control.
    Failed {
        /// Exact reader-facing failure message.
        message: String,
    },
}

impl ThreadScreenGate {
    /// Projects the `(has_thread_open, loading, has_failure)` presence triple
    /// consumed by [`thread_route_gate_render`].
    #[must_use]
    pub const fn presence(&self) -> (bool, bool, bool) {
        match self {
            Self::Loading => (false, true, false),
            Self::Open => (true, false, false),
            Self::Failed { .. } => (false, false, true),
        }
    }

    /// Returns the failure message for the retry branch, if this is it.
    #[must_use]
    pub fn failure_message(&self) -> Option<&str> {
        match self {
            Self::Failed { message } => Some(message),
            Self::Loading | Self::Open => None,
        }
    }
}
///
/// Activation callback for the gate failure retry control.
///
/// The transport-owned retry itself lives outside this view; the orchestrator
/// installs a callback that starts it. No callback means retry is
/// unavailable, and the control renders disabled rather than faking a retry.
pub type ThreadScreenRetry = Rc<dyn Fn(&mut Window, &mut App)>;

/// Owned thread-title facts for the header row.
///
/// Borrowed [`ThreadTitleInput`] values are built per render so this view
/// never retains a borrow across frames.
#[derive(Clone, Debug)]
pub struct ThreadScreenTitle {
    /// Harness-generated summary title, when the projection supplied one.
    pub summary_title: Option<String>,
    /// Stored title; the legacy document title falls back to `"Thread"`.
    pub title: String,
    /// Whether a manual rename has locked the stored title.
    pub title_locked: bool,
    /// Reader's title preference.
    pub mode: ThreadTitleMode,
}

impl Default for ThreadScreenTitle {
    fn default() -> Self {
        Self {
            summary_title: None,
            title: String::from("Thread"),
            title_locked: false,
            mode: ThreadTitleMode::Summary,
        }
    }
}

/// One owned checklist entry for the inspector checklist card.
///
/// [`ChecklistEntry`] borrows, so entries are stored owned and projected per
/// render through [`crate::thread_panel_policy::present_checklist_entry`].
#[derive(Clone, Debug, PartialEq)]
pub struct ThreadChecklistEntry {
    /// Stable entry identity, retained exactly for the list key.
    pub id: String,
    /// Protocol state used for tone projection.
    pub state: ChecklistEntryState,
    /// Reader-facing entry text, retained exactly.
    pub text: String,
}

/// The native thread screen: header, transcript column, inspector cards, and
/// composer dock.
///
/// State arrives through the small setters below; every render projects the
/// retained facts through the existing policies, so this view owns no
/// presentation logic of its own beyond element structure.
pub struct ThreadScreen {
    host: Entity<ConversationHost>,
    composer: Entity<NativeComposer>,
    retry_focus: FocusHandle,
    theme_mode: ThemeMode,
    gate: ThreadScreenGate,
    on_retry: Option<ThreadScreenRetry>,
    title: ThreadScreenTitle,
    environment: ThreadEnvironmentInput,
    terminals: Vec<TerminalSession>,
    terminals_loading: bool,
    checklist: Vec<ThreadChecklistEntry>,
    composer_disabled: bool,
}

impl ThreadScreen {
    /// Builds the screen around an already-mounted conversation host and
    /// composer. Both children stay live for the screen lifetime; the host
    /// carries the real controller-owned transcript.
    ///
    /// crate-internal because [`NativeComposer`](crate::native_composer) is a
    /// packet-2 surface: the crate root [`ThreadScreen::mount`] is the public
    /// factory, and in-crate hosts may also assemble the screen directly.
    pub(crate) fn new(
        host: Entity<ConversationHost>,
        composer: Entity<NativeComposer>,
        theme_mode: ThemeMode,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            host,
            composer,
            retry_focus: cx.focus_handle(),
            theme_mode,
            gate: ThreadScreenGate::default(),
            on_retry: None,
            title: ThreadScreenTitle::default(),
            environment: ThreadEnvironmentInput::default(),
            terminals: Vec::new(),
            terminals_loading: false,
            checklist: Vec::new(),
            // Legacy disables the composer until session and work authority
            // arrive (`disabled={!session_ready || !work_ready || ...}`).
            composer_disabled: true,
        }
    }

    /// Mounts host, composer, and screen in one application-context step.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationHostError::SceneProjection`] when the fresh
    /// controller cannot produce its empty initial scene.
    pub fn mount(
        thread_id: ThreadId,
        theme_mode: ThemeMode,
        cx: &mut App,
    ) -> Result<Entity<Self>, ConversationHostError> {
        let host = ConversationHost::mount(thread_id, theme_mode, cx)?;
        let composer = cx.new(NativeComposer::new);
        Ok(cx.new(|screen_cx| Self::new(host, composer, theme_mode, screen_cx)))
    }

    /// Returns the mounted conversation host entity.
    #[must_use]
    pub fn host(&self) -> &Entity<ConversationHost> {
        &self.host
    }

    /// Returns the mounted composer entity (packet 2 surface).
    ///
    /// crate-internal for the same reason as [`ThreadScreen::new`]. Reserved
    /// for the route integrator's composer forwarding; not yet called.
    #[allow(dead_code)]
    pub(crate) fn composer(&self) -> &Entity<NativeComposer> {
        &self.composer
    }

    /// Publishes which legacy gate branch the screen renders.
    pub fn set_gate(&mut self, gate: ThreadScreenGate) {
        self.gate = gate;
    }

    /// Installs the gate retry callback (or clears it when `None`).
    pub fn set_retry_handler(&mut self, on_retry: Option<ThreadScreenRetry>) {
        self.on_retry = on_retry;
    }

    /// Replaces the owned header-title facts.
    pub fn set_title(&mut self, title: ThreadScreenTitle) {
        self.title = title;
    }

    /// Replaces the owned environment-card input.
    pub fn set_environment(&mut self, environment: ThreadEnvironmentInput) {
        self.environment = environment;
    }

    /// Replaces the owned terminal sessions.
    pub fn set_terminals(&mut self, terminals: Vec<TerminalSession>) {
        self.terminals = terminals;
    }

    /// Publishes whether the terminal list itself is still loading.
    pub fn set_terminals_loading(&mut self, terminals_loading: bool) {
        self.terminals_loading = terminals_loading;
    }

    /// Replaces the owned checklist entries.
    pub fn set_checklist(&mut self, checklist: Vec<ThreadChecklistEntry>) {
        self.checklist = checklist;
    }

    /// Forwards the composer disabled flag into the packet-2 surface.
    pub fn set_composer_disabled(&mut self, disabled: bool, cx: &mut App) {
        self.composer_disabled = disabled;
        self.composer.update(cx, |composer, composer_cx| {
            composer.set_disabled(disabled, composer_cx);
        });
    }

    /// Forwards a theme-mode change into the transcript surface.
    pub fn set_theme_mode(&mut self, theme_mode: ThemeMode, cx: &mut App) {
        self.theme_mode = theme_mode;
        self.host.update(cx, |host, host_cx| {
            host.surface().update(host_cx, |surface, surface_cx| {
                surface.set_theme_mode(theme_mode, surface_cx);
            });
        });
    }

    /// Projects the gate render branch from the retained gate state.
    ///
    /// This is the same ordered projection the legacy gate uses
    /// (`thread-route-gate.svelte` branches), reused rather than restated.
    fn gate_branch(&self) -> ThreadRouteGateRender {
        let (has_thread_open, loading, has_failure) = self.gate.presence();
        thread_route_gate_render(has_thread_open, loading, has_failure)
    }

    /// Selects the header title through the shared display policy.
    fn display_title(&self) -> &str {
        thread_display_title(
            ThreadTitleInput::new(
                self.title.summary_title.as_deref(),
                self.title.title.as_str(),
                self.title.title_locked,
            ),
            &self.title.mode,
        )
    }

    /// Renders the header row carrying the policy-selected thread title.
    ///
    /// Legacy frame: the shell's workspace header line names the thread via
    /// `thread_display_title`; this row keeps that title on the thread
    /// screen itself as a `text-sm font-medium` line with `gap-2`/`py-3`
    /// rhythm.
    fn render_title_header(&self, theme: &ArtisanTheme) -> impl IntoElement {
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(ROW_GAP_PX))
            .px(px(COLUMN_PAD_X_PX))
            .py(px(12.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(theme.typography.control_text)
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.colors.foreground.to_paint())
                    .child(self.display_title().to_owned()),
            )
    }

    /// Renders the transcript column: prose-width wrapper around the live
    /// conversation host, plus the honest empty state for a scene with no
    /// turns yet.
    ///
    /// Legacy frame: `main.relative.h-full.min-h-0.overflow-hidden` holding
    /// `div.prose-column.w-full.max-w-(--prose-width).px-6.pt-10` around the
    /// turn sections. The host's own surface paints the scroll area, turn
    /// navigator rail, and jump-to-latest control.
    fn render_transcript_column(
        &self,
        theme: &ArtisanTheme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let empty = self.host.read(cx).controller_view().turn_views.is_empty();
        let mut column = div()
            .relative()
            .min_h_0()
            .flex_1()
            .overflow_hidden()
            .debug_selector(|| THREAD_SCREEN_TRANSCRIPT_SELECTOR.to_owned())
            .child(
                div()
                    .w_full()
                    .h_full()
                    .max_w(px(PROSE_WIDTH_PX))
                    .px(px(COLUMN_PAD_X_PX))
                    .pt(px(TRANSCRIPT_PAD_TOP_PX))
                    .child(self.host.clone()),
            );
        if empty {
            column = column.child(
                div()
                    .absolute()
                    .top(px(TRANSCRIPT_PAD_TOP_PX))
                    .left(px(0.0))
                    .right(px(0.0))
                    .flex()
                    .justify_center()
                    .debug_selector(|| THREAD_SCREEN_EMPTY_SELECTOR.to_owned())
                    .child(
                        div()
                            .text_size(theme.typography.control_text)
                            .text_color(theme.colors.muted_foreground.to_paint())
                            .child("No messages yet."),
                    ),
            );
        }
        column
    }

    /// Renders one environment-card row: flexible label plus truncating value.
    ///
    /// Legacy frame: `div.flex.min-w-0.items-center.gap-2.rounded-lg.px-2.py-2`
    /// with a `flex-1` label and a `max-w-36 truncate` value. Row icons are a
    /// gap (Tabler glyphs have no asset-glyph plumbing yet); rows keep their
    /// label/value structure without them.
    fn render_environment_row(
        label: &str,
        value: String,
        theme: &ArtisanTheme,
    ) -> impl IntoElement {
        div()
            .flex()
            .min_w_0()
            .items_center()
            .gap(px(ROW_GAP_PX))
            .px(px(ROW_PAD_PX))
            .py(px(ROW_PAD_PX))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(theme.typography.control_text)
                    .text_color(theme.colors.foreground.to_paint())
                    .child(label.to_owned()),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .max_w(px(ENV_VALUE_MAX_WIDTH_PX))
                    .truncate()
                    .text_size(theme.typography.control_text)
                    .text_color(theme.colors.foreground.to_paint())
                    .child(value),
            )
    }

    /// Renders the environment card (`thread-environment-card.svelte`).
    ///
    /// Legacy frame: `section[aria-label="Thread context"]` around the card
    /// with Machine, Changes, Branch, and Worktree rows. The project selector
    /// row belongs to packet 2's project picker and is a gap; the remote chip
    /// is icon-only in legacy and is a gap until an asset glyph exists.
    fn render_environment_card(&self, theme: &ArtisanTheme) -> impl IntoElement {
        let projection = present_thread_environment(&self.environment);
        let style = CardStyle::resolve(*theme);
        let mut rows = div()
            .flex()
            .min_w_0()
            .flex_col()
            .child(Self::render_environment_row(
                "Machine",
                projection.machine_label,
                theme,
            ));
        if let Some(summary) = projection.change_summary {
            rows = rows.child(
                div()
                    .flex()
                    .min_w_0()
                    .items_center()
                    .gap(px(ROW_GAP_PX))
                    .px(px(ROW_PAD_PX))
                    .py(px(ROW_PAD_PX))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(theme.typography.control_text)
                            .text_color(theme.colors.foreground.to_paint())
                            .child("Changes"),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(theme.typography.control_text)
                            .text_color(Hsla::from(rgb(ADDED_LINES_GREEN)))
                            .child(format!("+{}", summary.lines_added)),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(theme.typography.control_text)
                            .text_color(Hsla::from(rgb(DELETED_LINES_RED)))
                            .child(format!("{MINUS_SIGN}{}", summary.lines_deleted)),
                    ),
            );
        }
        if let Some(branch_label) = projection.current_branch_label {
            rows = rows.child(Self::render_environment_row("Branch", branch_label, theme));
        }
        if let Some(worktree_label) = projection.current_worktree_label {
            rows = rows.child(Self::render_environment_row(
                "Worktree",
                worktree_label,
                theme,
            ));
        }
        div().w_full().min_w_0().child(
            compact_card(style).w_full().child(
                compact_card_content(style).child(
                    div()
                        .min_w_0()
                        .text_size(theme.typography.control_text)
                        .child(rows),
                ),
            ),
        )
    }

    /// Renders the terminals card (`thread-terminals-card.svelte`).
    ///
    /// Legacy branches: a skeleton shimmer while loading, the card only when
    /// at least one live terminal exists, and nothing otherwise. Liveness is
    /// `opening | active` (`lib/terminal/presentation.ts`
    /// `is_live_terminal`); exited terminals disappear like finished agents.
    /// Rows follow `thread-terminals.svelte`: display name plus mono command
    /// line. Click-to-inspect and the tail-viewer dialog need transport
    /// wiring and are gaps, so rows render without a fake affordance.
    fn render_terminals_card(&self, theme: &ArtisanTheme) -> Option<impl IntoElement> {
        let style = CardStyle::resolve(*theme);
        if self.terminals_loading {
            let bar = |width: f32| {
                div()
                    .h(px(16.0))
                    .w(px(width))
                    .rounded(px(4.0))
                    .bg(theme.colors.muted.to_paint())
            };
            return Some(
                compact_card(style).w_full().child(
                    compact_card_content(style).child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(ROW_GAP_PX))
                            .p(px(12.0))
                            .debug_selector(|| {
                                String::from("artisan-thread-screen-terminals-loading")
                            })
                            .child(bar(180.0))
                            .child(bar(120.0)),
                    ),
                ),
            );
        }
        let live: Vec<&TerminalSession> = self
            .terminals
            .iter()
            .filter(|session| is_live_terminal(session))
            .collect();
        if live.is_empty() {
            return None;
        }
        let mut list = div().flex().min_w_0().flex_col();
        for session in live {
            list = list.child(
                div()
                    .flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .justify_between()
                    .gap(px(INSPECTOR_GAP_PX))
                    .px(px(ROW_PAD_PX))
                    .py(px(ROW_PAD_PX))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(theme.typography.control_text)
                            .text_color(theme.colors.foreground.to_paint())
                            .child(terminal_display_name(session)),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .max_w(px(160.0))
                            .truncate()
                            .text_size(theme.typography.label_text)
                            .text_color(theme.colors.muted_foreground.to_paint())
                            .child(terminal_command_line(session)),
                    ),
            );
        }
        Some(
            compact_card(style).w_full().child(
                compact_card_content(style)
                    .child(
                        div().flex().min_w_0().flex_col().child(
                            div()
                                .px(px(ROW_PAD_PX))
                                .pt(px(ROW_PAD_PX))
                                .pb(px(4.0))
                                .text_size(theme.typography.control_text)
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.colors.foreground.to_paint())
                                .child("Terminals"),
                        ),
                    )
                    .child(list),
            ),
        )
    }

    /// Renders the checklist card (`thread-panel.svelte` plan section).
    ///
    /// Legacy frame: `section[aria-labelledby]` with an
    /// `h2.px-2.pt-2.pb-1.text-sm.font-medium` "Checklist" heading and a
    /// `ul.list-disc` of `li.rounded-lg.px-2.py-2.text-sm` rows. Tone maps
    /// the exact legacy classes to theme colors: active
    /// (`font-medium text-foreground`) keeps weight and foreground;
    /// completed/pending/skipped (`text-muted-foreground`, with
    /// `line-through` on completed/skipped) use the muted token with
    /// strikethrough where legacy crosses out. The screen-reader
    /// `"{state}: "` prefix has no GPUI equivalent on plain text and is a
    /// gap.
    fn render_checklist_card(&self, theme: &ArtisanTheme) -> Option<impl IntoElement> {
        if self.checklist.is_empty() {
            return None;
        }
        let style = CardStyle::resolve(*theme);
        let mut list = div().flex().min_w_0().flex_col();
        for entry in &self.checklist {
            let presented = present_checklist_entry(ChecklistEntry::new(
                entry.id.as_str(),
                entry.state,
                entry.text.as_str(),
            ));
            let mut row = div()
                .px(px(ROW_PAD_PX))
                .py(px(ROW_PAD_PX))
                .text_size(theme.typography.control_text)
                .debug_selector(|| {
                    format!("artisan-thread-screen-checklist-entry-{}", presented.id)
                });
            row = match presented.state {
                ChecklistEntryState::Active => row
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.colors.foreground.to_paint()),
                ChecklistEntryState::Completed => row
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .line_through(),
                ChecklistEntryState::Pending => {
                    row.text_color(theme.colors.muted_foreground.to_paint())
                }
                ChecklistEntryState::Skipped => row
                    .text_color(theme.colors.muted_foreground.with_alpha(0.7).to_paint())
                    .line_through(),
            };
            list = list.child(row.child(presented.text.to_owned()));
        }
        Some(
            compact_card(style).w_full().child(
                compact_card_content(style)
                    .child(
                        div().flex().min_w_0().flex_col().child(
                            div()
                                .px(px(ROW_PAD_PX))
                                .pt(px(ROW_PAD_PX))
                                .pb(px(4.0))
                                .text_size(theme.typography.control_text)
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.colors.foreground.to_paint())
                                .child("Checklist"),
                        ),
                    )
                    .child(list),
            ),
        )
    }

    /// Renders the inspector column (`thread-panel.svelte` root).
    ///
    /// Legacy frame: `div.relative.flex.h-full.min-h-0.flex-col.p-1` around
    /// the `flex.min-h-0.flex-1.flex-col.gap-4` card group.
    fn render_inspector(&self, theme: &ArtisanTheme) -> impl IntoElement {
        let mut cards = div()
            .flex()
            .min_h_0()
            .flex_1()
            .flex_col()
            .gap(px(INSPECTOR_GAP_PX))
            .child(self.render_environment_card(theme));
        if let Some(terminals) = self.render_terminals_card(theme) {
            cards = cards.child(terminals);
        }
        if let Some(checklist) = self.render_checklist_card(theme) {
            cards = cards.child(checklist);
        }
        div()
            .relative()
            .flex_shrink_0()
            .w(px(INSPECTOR_WIDTH_PX))
            .min_h_0()
            .flex()
            .flex_col()
            .p(px(INSPECTOR_PAD_PX))
            .debug_selector(|| THREAD_SCREEN_INSPECTOR_SELECTOR.to_owned())
            .child(cards)
    }

    /// Renders the composer dock around the packet-2 composer surface.
    ///
    /// Legacy geometry (`thread-composer.svelte` frame): a bottom-centered
    /// `prose-column w-full max-w-(--prose-width)` wrapper with `pb-4`. The
    /// legacy composer is an absolute overlay whose `LipCard` chrome owns the
    /// overlap contract with the transcript end space; that chrome belongs to
    /// packet 2, so this screen docks the composer statically instead of
    /// floating it over transcript content it cannot reserve space for.
    fn render_composer_dock(&self, _theme: &ArtisanTheme) -> impl IntoElement {
        div()
            .flex()
            .flex_shrink_0()
            .justify_center()
            .w_full()
            .px(px(COLUMN_PAD_X_PX))
            .pb(px(COMPOSER_PAD_BOTTOM_PX))
            .debug_selector(|| THREAD_SCREEN_COMPOSER_SELECTOR.to_owned())
            .child(
                div()
                    .w_full()
                    .max_w(px(PROSE_WIDTH_PX))
                    .child(self.composer.clone()),
            )
    }

    /// Renders the gate loading branch (`thread-route-gate.svelte`).
    ///
    /// Legacy frame: `div.flex.h-full.min-h-0.items-center.justify-center`
    /// with `role="status"` and `aria-label="Loading thread"` holding the
    /// `size-6 text-muted-foreground` `FadeArc`. GPUI divs carry no DOM roles;
    /// the stable selector keeps the branch addressable instead.
    fn render_loading(theme: &ArtisanTheme) -> impl IntoElement {
        div()
            .flex()
            .h_full()
            .min_h_0()
            .items_center()
            .justify_center()
            .bg(theme.colors.background.to_paint())
            .debug_selector(|| THREAD_SCREEN_LOADING_SELECTOR.to_owned())
            .child(
                FadeArc::new(SharedString::from(THREAD_SCREEN_LOADING_SELECTOR), *theme)
                    .size(px(24.0))
                    .debug_selector(THREAD_SCREEN_LOADING_SELECTOR),
            )
    }

    /// Renders the gate failure branch (`thread-route-gate.svelte`).
    ///
    /// Legacy frame: `div.flex.h-full.min-h-0.items-center.justify-center.px-6.text-center`
    /// around `div.flex.max-w-md.flex-col.items-center.gap-3` with the
    /// `text-sm text-destructive` message (`role="alert"`) and the bordered
    /// Retry button. The button is disabled while no retry handler is
    /// installed rather than faking a retry.
    fn render_failure(&self, theme: &ArtisanTheme, message: &str) -> impl IntoElement {
        let retry_ready = self.on_retry.is_some();
        let retry_focus = self.retry_focus.clone();
        let on_retry = self.on_retry.clone();
        let mut retry = Button::new(
            THREAD_SCREEN_RETRY_SELECTOR,
            retry_focus,
            *theme,
            MotionPolicy::Reduced,
            ButtonVariant::Outline,
            ButtonSize::Small,
            ButtonContent::text("Retry"),
        )
        .expect("static thread-screen retry button configuration is valid")
        .focus_visibility(FocusVisibility::Visible)
        .debug_selector(THREAD_SCREEN_RETRY_SELECTOR)
        .disabled(!retry_ready);
        if let Some(on_retry) = on_retry {
            retry = retry.on_activate(move |_, window, app| {
                on_retry(window, app);
            });
        }
        div()
            .flex()
            .h_full()
            .min_h_0()
            .items_center()
            .justify_center()
            .px(px(COLUMN_PAD_X_PX))
            .bg(theme.colors.background.to_paint())
            .debug_selector(|| THREAD_SCREEN_FAILURE_SELECTOR.to_owned())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(12.0))
                    .max_w(px(FAILURE_MAX_WIDTH_PX))
                    .child(
                        div()
                            .text_size(theme.typography.control_text)
                            .text_color(theme.colors.destructive.to_paint())
                            .child(message.to_owned()),
                    )
                    .child(retry),
            )
    }

    /// Renders the opened-route branch (`thread-workspace.svelte` frame).
    fn render_open(&self, theme: &ArtisanTheme, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .flex()
            .flex_col()
            .h_full()
            .min_h_0()
            .bg(theme.colors.background.to_paint())
            .debug_selector(|| THREAD_SCREEN_SELECTOR.to_owned())
            .child(self.render_title_header(theme))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .min_h_0()
                    .flex_1()
                    .child(self.render_transcript_column(theme, cx))
                    .child(self.render_inspector(theme)),
            )
            .child(self.render_composer_dock(theme))
    }
}

/// Returns whether a terminal session stays visible on the terminals card.
///
/// This is the legacy `is_live_terminal` boundary
/// (`lib/terminal/presentation.ts`): `opening | active` sessions show;
/// exited terminals disappear like finished agents.
fn is_live_terminal(session: &TerminalSession) -> bool {
    matches!(
        session.state,
        TerminalState::Opening | TerminalState::Active
    )
}

impl Render for ThreadScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = ArtisanTheme::for_mode(self.theme_mode);
        match self.gate_branch() {
            ThreadRouteGateRender::OpenedRoute => self.render_open(&theme, cx).into_any_element(),
            ThreadRouteGateRender::LoadingIndicator => {
                Self::render_loading(&theme).into_any_element()
            }
            ThreadRouteGateRender::FailureRetry => {
                let message = self.gate.failure_message().unwrap_or_default();
                self.render_failure(&theme, message).into_any_element()
            }
            ThreadRouteGateRender::EmptyFallback => div()
                .h_full()
                .min_h_0()
                .bg(theme.colors.background.to_paint())
                .debug_selector(|| THREAD_SCREEN_SELECTOR.to_owned())
                .into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_title() -> ThreadScreenTitle {
        ThreadScreenTitle::default()
    }

    #[test]
    fn default_title_falls_back_to_thread() {
        let title = empty_title();
        let display = thread_display_title(
            ThreadTitleInput::new(
                title.summary_title.as_deref(),
                &title.title,
                title.title_locked,
            ),
            &title.mode,
        );
        assert_eq!(display, "Thread");
    }

    #[test]
    fn summary_mode_selects_present_summary_for_unlocked_title() {
        let title = ThreadScreenTitle {
            summary_title: Some(String::from("Ship the port")),
            title: String::from("old title"),
            title_locked: false,
            mode: ThreadTitleMode::Summary,
        };
        let display = thread_display_title(
            ThreadTitleInput::new(
                title.summary_title.as_deref(),
                &title.title,
                title.title_locked,
            ),
            &title.mode,
        );
        assert_eq!(display, "Ship the port");
    }

    #[test]
    fn live_terminal_filter_keeps_opening_and_active_only() {
        let sessions = [
            TerminalSession::new("a", "pwsh", Vec::<String>::new(), TerminalState::Opening),
            TerminalSession::new("b", "pwsh", Vec::<String>::new(), TerminalState::Active),
            TerminalSession::new("c", "pwsh", Vec::<String>::new(), TerminalState::Closed),
            TerminalSession::new("d", "pwsh", Vec::<String>::new(), TerminalState::Failed),
        ];
        let live: Vec<&str> = sessions
            .iter()
            .filter(|session| is_live_terminal(session))
            .map(|session| session.terminal_id.as_str())
            .collect();
        assert_eq!(live, vec!["a", "b"]);
    }

    #[test]
    fn gate_presence_maps_each_branch_through_policy_order() {
        let loading = ThreadScreenGate::Loading;
        let (has_open, loading_flag, has_failure) = loading.presence();
        assert_eq!(
            thread_route_gate_render(has_open, loading_flag, has_failure),
            ThreadRouteGateRender::LoadingIndicator
        );
        let open = ThreadScreenGate::Open;
        let (has_open, loading_flag, has_failure) = open.presence();
        assert_eq!(
            thread_route_gate_render(has_open, loading_flag, has_failure),
            ThreadRouteGateRender::OpenedRoute
        );
        let failed = ThreadScreenGate::Failed {
            message: String::from("Forge is unreachable"),
        };
        assert_eq!(failed.failure_message(), Some("Forge is unreachable"));
        let (has_open, loading_flag, has_failure) = failed.presence();
        assert_eq!(
            thread_route_gate_render(has_open, loading_flag, has_failure),
            ThreadRouteGateRender::FailureRetry
        );
        assert_eq!(ThreadScreenGate::default(), ThreadScreenGate::Loading);
    }

    #[test]
    fn checklist_entry_projection_preserves_identity_and_tone() {
        let entry = ThreadChecklistEntry {
            id: String::from("entry-1"),
            state: ChecklistEntryState::Active,
            text: String::from("Port the transcript"),
        };
        let presented = present_checklist_entry(ChecklistEntry::new(
            entry.id.as_str(),
            entry.state,
            entry.text.as_str(),
        ));
        assert_eq!(presented.id, "entry-1");
        assert_eq!(presented.state, ChecklistEntryState::Active);
        assert_eq!(presented.text, "Port the transcript");
    }
}
