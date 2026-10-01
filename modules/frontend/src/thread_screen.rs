//! Native GPUI thread route screen (`/t/[workspace]/[thread]`).
//!
//! Direct translation of the legacy conversation surface into GPUI:
//!
//! - `routes/t/[workspace]/[thread]/+page.svelte` scopes the route by
//!   `workspace:thread` (`{#key ...}`) and mounts
//!   `routes/components/thread-route-gate.svelte`. Gate precedence
//!   (opened route > loading > failure retry) is reused from
//!   [`crate::thread_route_gate_policy::thread_route_gate_render`]; the
//!   loading mark is the shared shimmering wordmark ([`WordmarkLoader`]),
//!   the legacy application loader, in place of the legacy gate's arc.
//! - `routes/components/thread-route.svelte` is controller plumbing around
//!   one headline visual fact: the document title renders the thread's
//!   display title, which the Forge resolves and the listing carries. It
//!   feeds the desktop titlebar's centred header subject only; this screen
//!   carries no title row of its own (the reference desktop shell renders
//!   the workspace header once, in the window chrome).
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
//!   inspector column for a thread surface) contributes the row content of
//!   the environment card (`thread-environment-card.svelte`), the checklist,
//!   and the terminals card (`thread-terminals-card.svelte` +
//!   `thread-terminals.svelte` rows). Natively the column follows the ruled
//!   desktop shell instead of the legacy floating glass cards: a flat
//!   true-black column exactly as wide as the expanded left sidebar
//!   ([`THREAD_INSPECTOR_WIDTH_PX`]), with a one-device-pixel left rule and
//!   the unlabelled Context rows followed by the Agents, Checklist and
//!   Terminals sections under muted labels set like the sidebar's thread-age
//!   groups. The shell paints the junction crosshair
//!   where that rule meets the titlebar. Row content is projected through the
//!   already-ported [`crate::thread_environment_presentation`],
//!   [`crate::terminal_presentation`], and [`crate::thread_panel_policy`]
//!   policies.
//!
//! The composer is packet 2's [`NativeComposer`](crate::native_composer)
//! entity, consumed as-is and docked at the bottom of the frame. Anything
//! this screen cannot honestly render yet (project picker, remote chip,
//! terminal tail viewer, `LipCard` overlay chrome) is listed in the report as
//! a gap, not faked; inspector rows stay non-interactive.

#![forbid(unsafe_code)]

use std::rc::Rc;

use artisan_assets::AssetId;
use artisan_domain::{PlanEntry, PlanEntryStatus, ThreadId};
use artisan_ui::button::{Button, ButtonContent, ButtonSize, ButtonVariant, FocusVisibility};
use artisan_ui::fade_arc::FadeArc;
use artisan_ui::icon::{IconSize, IconStyle, IconTint, icon};
use artisan_ui::motion::MotionPolicy;
use artisan_ui::theme::{
    ArtisanTheme, DesktopTheme, ProseTypography, RadiusStep, RadiusTokens, ThemeMode,
};
use artisan_ui::wordmark_loader::WordmarkLoader;
use gpui::{
    App, AppContext as _, Context, Div, Entity, FocusHandle, Hsla, IntoElement, Pixels, Render,
    SharedString, Subscription, Window, div,
    prelude::{InteractiveElement as _, ParentElement as _, Styled as _},
    px, rgb, rgb_to_hsla,
};

use crate::conversation_host::{ConversationHost, ConversationHostError};
use crate::desktop_shell::{
    DESKTOP_COLUMN_INSET_PX, DESKTOP_SIDEBAR_WIDTH_PX, DesktopShellStyle, desktop_section_label,
};
use crate::native_composer::NativeComposer;
use crate::shell_layout::{ProseWidth, desktop_inspector_pixels};
use crate::terminal_presentation::{
    TerminalSession, TerminalState, terminal_command_line, terminal_display_name,
};
use crate::thread_agents::{ThreadAgentEntry, ThreadAgentState};
use crate::thread_environment_presentation::{ThreadEnvironmentInput, present_thread_environment};
use crate::thread_panel_policy::{ChecklistEntry, ChecklistEntryState, present_checklist_entry};
use crate::thread_route_gate_policy::{ThreadRouteGateRender, thread_route_gate_render};

// Module split submodules (see thread_screen/).

#[path = "thread_screen/state.rs"]
mod state;

#[path = "thread_screen/render.rs"]
mod render;

pub use state::*;

#[cfg(test)]
use render::is_live_terminal;

/// Stable debug selector for the thread screen root.
pub const THREAD_SCREEN_SELECTOR: &str = "artisan-thread-screen";

/// Stable debug selector for the gate loading indicator.
pub const THREAD_SCREEN_LOADING_SELECTOR: &str = "artisan-thread-screen-loading";

/// Stable debug selector for the wordmark inside the gate loading branch.
pub const THREAD_SCREEN_LOADING_MARK_SELECTOR: &str = "artisan-thread-screen-loading-mark";

/// How long the loading gate stays blank before its mark fades in, so a
/// thread that presents quickly never flashes it.
pub const THREAD_SCREEN_LOADING_MARK_DELAY: std::time::Duration = std::time::Duration::from_secs(1);

/// Stable debug selector for the gate failure branch.
pub const THREAD_SCREEN_FAILURE_SELECTOR: &str = "artisan-thread-screen-failure";

/// Stable debug selector for the gate retry control.
pub const THREAD_SCREEN_RETRY_SELECTOR: &str = "artisan-thread-screen-retry";

/// Stable debug selector for the transcript column.
pub const THREAD_SCREEN_TRANSCRIPT_SELECTOR: &str = "artisan-thread-screen-transcript";

/// Stable debug selector for the inspector column.
pub const THREAD_SCREEN_INSPECTOR_SELECTOR: &str = "artisan-thread-screen-inspector";

/// Stable debug selector for the inspector column's one-device-pixel left
/// rule.
pub const THREAD_SCREEN_INSPECTOR_RULE_SELECTOR: &str = "artisan-thread-screen-inspector-rule";

/// Stable debug selector for the composer dock.
pub const THREAD_SCREEN_COMPOSER_SELECTOR: &str = "artisan-thread-screen-composer-dock";

/// Stable debug selector for the centered composer card wrapper inside the
/// overlay frame (the native `prose-column w-full max-w-(--prose-width)`).
pub const THREAD_SCREEN_COMPOSER_CARD_SELECTOR: &str = "artisan-thread-screen-composer-card";

/// Stable debug selector for the inspector's Context section.
pub const THREAD_SCREEN_CONTEXT_SELECTOR: &str = "artisan-thread-screen-context";

/// Stable debug selector for the inspector's Agents section.
pub const THREAD_SCREEN_AGENTS_SELECTOR: &str = "artisan-thread-screen-agents";

/// Stable debug selector for the Agents section's muted label.
pub const THREAD_SCREEN_AGENTS_LABEL_SELECTOR: &str = "artisan-thread-screen-agents-label";

/// Stable debug-selector prefix for one agent row; the agent's identity is
/// appended after a `-` separator.
pub const THREAD_SCREEN_AGENT_ROW_PREFIX: &str = "artisan-thread-screen-agent";

/// Stable debug selector for the inspector's Checklist section.
pub const THREAD_SCREEN_CHECKLIST_SELECTOR: &str = "artisan-thread-screen-checklist";

/// Stable debug selector for the Checklist section's muted label.
pub const THREAD_SCREEN_CHECKLIST_LABEL_SELECTOR: &str = "artisan-thread-screen-checklist-label";

/// Stable debug selector for the inspector's Terminals section.
pub const THREAD_SCREEN_TERMINALS_SELECTOR: &str = "artisan-thread-screen-terminals";

/// Stable debug selector for the Terminals section's muted label.
pub const THREAD_SCREEN_TERMINALS_LABEL_SELECTOR: &str = "artisan-thread-screen-terminals-label";

/// Stable debug selector for the Context section's Project row.
pub const THREAD_SCREEN_PROJECT_ROW_SELECTOR: &str = "artisan-thread-screen-project-row";

/// Stable debug selector for one environment row of the Context section
/// (Machine, Changes, Branch, Worktree).
pub const THREAD_SCREEN_ENV_ROW_SELECTOR: &str = "artisan-thread-screen-env-row";

/// `--prose-width: 48rem` (`lib/styles/theme.css:157`); `1rem` is `16px`.
const PROSE_WIDTH_PX: f32 = 768.0;

/// `px-6` on the transcript column and composer dock.
const COLUMN_PAD_X_PX: f32 = 24.0;

/// `pb-4` under the composer frame below the `sm` breakpoint.
const COMPOSER_PAD_BOTTOM_PX: f32 = 16.0;

/// Viewport width where the composer frame steps to its desktop inset.
const COMPOSER_WIDE_BREAKPOINT_PX: f32 = 640.0;

/// `sm:pb-6` under the composer frame at/above the breakpoint — 24 px. The
/// native window sets no minimum width, so the frame follows live bounds.
const COMPOSER_PAD_BOTTOM_WIDE_PX: f32 = 24.0;

/// Resolves the composer frame bottom inset for a window viewport width:
/// 24 px at/above 640 px, 16 px below (`pb-4 sm:pb-6`,
/// `thread-composer.svelte:526`).
pub(crate) const fn composer_pad_bottom(viewport_width_px: f32) -> f32 {
    if viewport_width_px >= COMPOSER_WIDE_BREAKPOINT_PX {
        COMPOSER_PAD_BOTTOM_WIDE_PX
    } else {
        COMPOSER_PAD_BOTTOM_PX
    }
}

/// `max-w-md` on the gate failure column.
const FAILURE_MAX_WIDTH_PX: f32 = 448.0;

/// `gap-4` (16 px) between a terminal row's name cluster and its muted
/// command line.
const TERMINAL_ROW_GAP_PX: f32 = 16.0;

/// Full desktop inspector column width: exactly the expanded left sidebar
/// ([`DESKTOP_SIDEBAR_WIDTH_PX`], 327 px), so the two ruled columns frame the
/// conversation as a pair. On narrower windows it shrinks with the sidebar
/// (see [`thread_inspector_width`]).
pub(crate) const THREAD_INSPECTOR_WIDTH_PX: f32 = DESKTOP_SIDEBAR_WIDTH_PX;

/// True-black shell paint (`#000000`).
///
/// The explicit black-shell request overrides the Electron dark chrome
/// (`--surface-950` background, `--surface-900/925` card gradient) for shell
/// surfaces only: title header, transcript column, the flat inspector column,
/// composer dock, and gate branches. Message bubbles, controls, and overlays
/// keep their themed fills so intentional contrast survives.
const SHELL_BLACK_HEX: u32 = 0x0000_0000;

/// Resolves the true-black shell paint.
pub(crate) fn shell_black() -> Hsla {
    rgb_to_hsla(rgb(SHELL_BLACK_HEX))
}

/// Resolves the inspector column width for a content width, or `None` while
/// it does not fit.
///
/// The `shell-layout.ts` desktop policy at the native balanced prose width,
/// measured from the width left after the desktop sidebar (window minus
/// [`DesktopShellStyle::sidebar_width`], both in logical pixels) — never from
/// a physical-pixel screenshot reading. The chat keeps its 864 px minimum
/// (768 px prose plus the 48 px rail clearance each side); the column takes
/// what remains up to [`THREAD_INSPECTOR_WIDTH_PX`] and hides below 240 px.
/// Hidden, the column takes no space. Non-positive and non-finite widths
/// never fit.
pub(crate) fn thread_inspector_width(content_width_px: f32) -> Option<f32> {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the resolved width is bounded by the f32 column constant"
    )]
    desktop_inspector_pixels(
        f64::from(content_width_px),
        ProseWidth::Balanced,
        f64::from(THREAD_INSPECTOR_WIDTH_PX),
    )
    .map(|width| width as f32)
}

/// Inspector loading-shimmer bar height: `h-4`
/// (`thread-terminals-card.svelte:195-196`) — 16 px.
const LOADING_BAR_PX: f32 = 16.0;

/// Slot holding an agent row's state mark: the row glyph's 16 px, so the
/// working arc and the settled dot share one center.
const AGENT_STATE_SLOT_PX: f32 = 16.0;

/// Settled agent state dot: `state-dot size-1.5` (`thread-agents.svelte`).
const AGENT_STATE_DOT_PX: f32 = 6.0;

/// Which inspector row a glyph belongs to.
///
/// Identities transcribe the `@tabler/icons-svelte` imports in
/// `thread-environment-card.svelte:2-6` (`device-laptop`, `file-diff`,
/// `git-branch`, `folder-code`) and `thread-terminals.svelte:2`
/// (`terminal-2`); the Project row takes the plain `folder` glyph the sidebar
/// project picker already uses. Host-mark brand icons and dropdown chevrons
/// have no exact catalog glyph behind an honest affordance and stay gaps.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum InspectorRowIcon {
    /// `folder` on the Project row.
    Project,
    /// `device-laptop` on the Machine row.
    Machine,
    /// `file-diff` on the Changes row.
    Changes,
    /// `git-branch` on the Branch row.
    Branch,
    /// `folder-code` on the Worktree row.
    Worktree,
    /// `terminal-2` on terminal rows.
    Terminal,
    /// `bot-id` on agent rows (`thread-agents.svelte`).
    Agent,
}

/// Returns the exact catalog glyph for an inspector row kind.
#[must_use]
pub(crate) const fn inspector_row_icon(kind: InspectorRowIcon) -> AssetId {
    match kind {
        InspectorRowIcon::Project => AssetId::TABLER_FOLDER,
        InspectorRowIcon::Machine => AssetId::TABLER_DEVICE_LAPTOP,
        InspectorRowIcon::Changes => AssetId::TABLER_FILE_DIFF,
        InspectorRowIcon::Branch => AssetId::TABLER_GIT_BRANCH,
        InspectorRowIcon::Worktree => AssetId::TABLER_FOLDER_CODE,
        InspectorRowIcon::Terminal => AssetId::TABLER_TERMINAL_2,
        InspectorRowIcon::Agent => AssetId::TABLER_BOT_ID,
    }
}

/// `px-2 py-2` on inspector rows: the 8 px horizontal inset matches the
/// shared section label ([`crate::desktop_shell::DESKTOP_COLUMN_ROW_INSET_PX`])
/// and the left sidebar's rows, so labels and row text share one edge.
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

#[cfg(test)]
mod tests {
    #![expect(
        clippy::float_cmp,
        reason = "test assertions compare the exact pixel arithmetic the UI performs; an epsilon would weaken the regression coverage"
    )]
    use super::*;

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

    fn plan_entry(id: &str, status: PlanEntryStatus, text: &str) -> PlanEntry {
        PlanEntry::new(
            artisan_domain::ObservationId::parse(id).expect("plan entry id"),
            status,
            text.to_owned(),
        )
        .expect("plan entry")
    }

    /// Each protocol plan status lands on its checklist policy state, with
    /// identity and text copied exactly; the borrowed comparison agrees with
    /// the copy and catches a changed entry.
    #[test]
    fn plan_entries_map_onto_checklist_policy_states() {
        for (status, state) in [
            (PlanEntryStatus::Pending, ChecklistEntryState::Pending),
            (PlanEntryStatus::InProgress, ChecklistEntryState::Active),
            (PlanEntryStatus::Completed, ChecklistEntryState::Completed),
        ] {
            let source = plan_entry("step-1", status, "Wire the inspector");
            let entry = ThreadChecklistEntry::from_plan_entry(&source);
            assert_eq!(
                entry,
                ThreadChecklistEntry {
                    id: String::from("step-1"),
                    state,
                    text: String::from("Wire the inspector"),
                }
            );
            assert!(entry.presents(&source));
        }
        let entry = ThreadChecklistEntry::from_plan_entry(&plan_entry(
            "step-1",
            PlanEntryStatus::InProgress,
            "Wire the inspector",
        ));
        assert!(!entry.presents(&plan_entry(
            "step-1",
            PlanEntryStatus::Completed,
            "Wire the inspector",
        )));
        assert!(!entry.presents(&plan_entry(
            "step-1",
            PlanEntryStatus::InProgress,
            "Wire the panel",
        )));
    }

    /// The environment and checklist setters report a change only when the
    /// value actually differs, so the per-render route sync never notifies
    /// for an unchanged frame; the borrowed plan check follows suit.
    #[gpui::test]
    fn inspector_setters_report_only_real_changes(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| {
            mount_proof_screen("shell-proof-setters", EXPANDED_WIDE_CONTENT, cx)
        });
        cx.update(|_, app| {
            view.update(app, |probe, probe_cx| {
                probe.screen.update(probe_cx, |screen, _| {
                    let environment = ThreadEnvironmentInput {
                        identity: Some(
                            crate::thread_environment_presentation::HostIdentitySnapshot::new(
                                "forge-host",
                            ),
                        ),
                        ..ThreadEnvironmentInput::default()
                    };
                    assert!(screen.set_environment(environment.clone()));
                    assert!(!screen.set_environment(environment.clone()));
                    assert_eq!(screen.environment(), &environment);

                    let plan = [
                        plan_entry("step-1", PlanEntryStatus::Completed, "Read"),
                        plan_entry("step-2", PlanEntryStatus::InProgress, "Wire"),
                    ];
                    assert!(screen.checklist_presents(&[]));
                    assert!(!screen.checklist_presents(&plan));
                    let entries: Vec<ThreadChecklistEntry> = plan
                        .iter()
                        .map(ThreadChecklistEntry::from_plan_entry)
                        .collect();
                    assert!(screen.set_checklist(entries.clone()));
                    assert!(!screen.set_checklist(entries));
                    assert!(screen.checklist_presents(&plan));
                    assert!(!screen.checklist_presents(&plan[..1]));
                    assert!(screen.set_checklist(Vec::new()));
                    assert!(screen.checklist().is_empty());
                });
            });
        });
    }

    /// Mounted-total equivalents in logical pixels (window minus the live
    /// [`DesktopShellStyle`](crate::desktop_shell::DesktopShellStyle) rail,
    /// never a scaled screenshot reading): 1280 px expanded leaves 953 px,
    /// 1400 px expanded leaves 1073 px, 1920 px expanded leaves 1593 px, and
    /// 1400 px collapsed leaves 1342 px.
    const EXPANDED_1280_CONTENT: f32 = 953.0;
    const EXPANDED_1400_CONTENT: f32 = 1073.0;
    const EXPANDED_WIDE_CONTENT: f32 = 1593.0;
    const COLLAPSED_1400_CONTENT: f32 = 1342.0;

    #[test]
    fn inspector_hides_at_mounted_1280_and_1400_with_expanded_rail() {
        assert!(thread_inspector_width(EXPANDED_1280_CONTENT).is_none());
        assert!(thread_inspector_width(EXPANDED_1400_CONTENT).is_none());
    }

    #[test]
    fn inspector_returns_when_wide_with_room_to_spare() {
        assert!(thread_inspector_width(EXPANDED_WIDE_CONTENT).is_some());
    }

    /// Inclusive boundary of the fit: 1104 px of content is the 864 px
    /// minimum chat (768 px prose plus 48 px rail clearance each side) plus
    /// the 240 px column minimum; one pixel less hides it. The column grows
    /// with the content up to the full 327 px. Crossing back and forth
    /// toggles in both directions.
    #[test]
    fn inspector_visibility_flips_at_the_column_minimum() {
        let threshold = 768.0 + 2.0 * 48.0 + 240.0;
        assert_eq!(threshold, 1104.0);
        assert_eq!(thread_inspector_width(threshold), Some(240.0));
        assert_eq!(thread_inspector_width(threshold - 1.0), None);
        assert_eq!(thread_inspector_width(threshold - 0.5), None);
        assert_eq!(thread_inspector_width(threshold + 40.0), Some(280.0));
        assert_eq!(
            thread_inspector_width(768.0 + 96.0 + THREAD_INSPECTOR_WIDTH_PX),
            Some(THREAD_INSPECTOR_WIDTH_PX)
        );
        assert_eq!(
            thread_inspector_width(EXPANDED_WIDE_CONTENT),
            Some(THREAD_INSPECTOR_WIDTH_PX)
        );
    }

    #[test]
    fn collapsed_rail_seats_the_inspector_at_1400_total() {
        assert!(thread_inspector_width(COLLAPSED_1400_CONTENT).is_some());
    }

    #[test]
    fn inspector_visibility_toggles_in_both_resize_directions() {
        assert!(thread_inspector_width(EXPANDED_WIDE_CONTENT).is_some());
        assert!(thread_inspector_width(EXPANDED_1280_CONTENT).is_none());
        assert!(thread_inspector_width(EXPANDED_WIDE_CONTENT).is_some());
        assert!(thread_inspector_width(EXPANDED_1400_CONTENT).is_none());
    }

    #[test]
    fn inspector_never_fits_malformed_content_widths() {
        assert!(thread_inspector_width(0.0).is_none());
        assert!(thread_inspector_width(-1280.0).is_none());
        assert!(thread_inspector_width(f32::NAN).is_none());
    }

    #[test]
    fn inspector_width_is_the_expanded_sidebar_width() {
        assert_eq!(THREAD_INSPECTOR_WIDTH_PX, DESKTOP_SIDEBAR_WIDTH_PX);
        assert_eq!(THREAD_INSPECTOR_WIDTH_PX, 327.0);
        // Rows and section labels share the sidebar's 8 px text inset.
        assert_eq!(
            ROW_PAD_PX,
            crate::desktop_shell::DESKTOP_COLUMN_ROW_INSET_PX
        );
    }

    #[test]
    fn composer_frame_inset_follows_the_640px_viewport_rule() {
        assert_eq!(composer_pad_bottom(0.0), COMPOSER_PAD_BOTTOM_PX);
        assert_eq!(composer_pad_bottom(639.0), COMPOSER_PAD_BOTTOM_PX);
        assert_eq!(composer_pad_bottom(640.0), COMPOSER_PAD_BOTTOM_WIDE_PX);
        assert_eq!(composer_pad_bottom(1920.0), COMPOSER_PAD_BOTTOM_WIDE_PX);
        assert_eq!(COMPOSER_PAD_BOTTOM_WIDE_PX, 24.0);
    }

    /// Minimal host mounting one proof screen so the paint tree can be
    /// inspected: the screen is the real entity (mounted through
    /// [`ThreadScreen::mount_proof`]), not a stub.
    struct ShellProofProbe {
        screen: Entity<ThreadScreen>,
    }

    impl Render for ShellProofProbe {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut Context<Self>,
        ) -> impl gpui::IntoElement {
            self.screen.clone()
        }
    }

    fn mount_proof_screen(
        thread: &str,
        content_width_px: f32,
        cx: &mut Context<ShellProofProbe>,
    ) -> ShellProofProbe {
        let screen = ThreadScreen::mount_proof(
            ThreadId::parse(thread).expect("thread id parses"),
            content_width_px,
            cx,
        )
        .expect("proof screen mounts");
        ShellProofProbe { screen }
    }

    /// At mounted-1280 content the inspector and its reserved space are gone
    /// while the composer stays inside the transcript column.
    #[gpui::test]
    fn narrow_content_omits_inspector_and_contains_composer(cx: &mut gpui::TestAppContext) {
        let (_view, cx) = cx.add_window_view(|_, cx| {
            mount_proof_screen("shell-proof-narrow", EXPANDED_1280_CONTENT, cx)
        });
        cx.run_until_parked();
        assert!(
            cx.debug_bounds(THREAD_SCREEN_INSPECTOR_SELECTOR).is_none(),
            "1280-total content must omit the inspector and its space"
        );
        let transcript = cx
            .debug_bounds(THREAD_SCREEN_TRANSCRIPT_SELECTOR)
            .expect("transcript column lays out");
        let composer = cx
            .debug_bounds(THREAD_SCREEN_COMPOSER_SELECTOR)
            .expect("composer dock lays out");
        let transcript_left = f32::from(transcript.origin.x);
        let transcript_right = transcript_left + f32::from(transcript.size.width);
        let composer_left = f32::from(composer.origin.x);
        let composer_right = composer_left + f32::from(composer.size.width);
        assert!(
            composer_left >= transcript_left - 1.0 && composer_right <= transcript_right + 1.0,
            "composer [{composer_left}, {composer_right}] must stay inside the transcript column [{transcript_left}, {transcript_right}]"
        );
    }

    /// At wide content the inspector returns as a distinct 327 px column that
    /// neither overlaps the transcript nor the composer.
    #[gpui::test]
    fn wide_content_seats_a_disjoint_inspector_column(cx: &mut gpui::TestAppContext) {
        let (_view, cx) = cx.add_window_view(|_, cx| {
            mount_proof_screen("shell-proof-wide", EXPANDED_WIDE_CONTENT, cx)
        });
        cx.run_until_parked();
        let inspector = cx
            .debug_bounds(THREAD_SCREEN_INSPECTOR_SELECTOR)
            .expect("wide content seats the inspector");
        let transcript = cx
            .debug_bounds(THREAD_SCREEN_TRANSCRIPT_SELECTOR)
            .expect("transcript column lays out");
        let composer = cx
            .debug_bounds(THREAD_SCREEN_COMPOSER_SELECTOR)
            .expect("composer dock lays out");
        let inspector_width = f32::from(inspector.size.width);
        assert_eq!(
            inspector_width, THREAD_INSPECTOR_WIDTH_PX,
            "the inspector is exactly as wide as the expanded sidebar"
        );
        let inspector_left = f32::from(inspector.origin.x);
        let transcript_right = f32::from(transcript.origin.x) + f32::from(transcript.size.width);
        let composer_right = f32::from(composer.origin.x) + f32::from(composer.size.width);
        assert!(
            transcript_right <= inspector_left + 1.0,
            "transcript right {transcript_right}px must not cross the inspector at {inspector_left}px"
        );
        assert!(
            composer_right <= inspector_left + 1.0,
            "composer right {composer_right}px must not cross the inspector at {inspector_left}px"
        );
    }

    /// Resize is truly live: narrowing a mounted wide screen drops the
    /// inspector (change-guarded notify defeats GPUI child caching), and
    /// widening it again seats the 327 px column back.
    #[gpui::test]
    fn resize_toggles_the_mounted_inspector_column(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| {
            mount_proof_screen("shell-proof-resize", EXPANDED_WIDE_CONTENT, cx)
        });
        cx.run_until_parked();
        assert!(
            cx.debug_bounds(THREAD_SCREEN_INSPECTOR_SELECTOR).is_some(),
            "wide mount seats the inspector"
        );
        cx.update(|_, app| {
            view.update(app, |probe, probe_cx| {
                probe.screen.update(probe_cx, |screen, screen_cx| {
                    if screen.set_content_width(EXPANDED_1280_CONTENT) {
                        screen_cx.notify();
                    }
                });
            });
        });
        cx.run_until_parked();
        assert!(
            cx.debug_bounds(THREAD_SCREEN_INSPECTOR_SELECTOR).is_none(),
            "narrowed content must drop the inspector and its space"
        );
        cx.update(|_, app| {
            view.update(app, |probe, probe_cx| {
                probe.screen.update(probe_cx, |screen, screen_cx| {
                    if screen.set_content_width(EXPANDED_WIDE_CONTENT) {
                        screen_cx.notify();
                    }
                });
            });
        });
        cx.run_until_parked();
        let inspector = cx
            .debug_bounds(THREAD_SCREEN_INSPECTOR_SELECTOR)
            .expect("widened content seats the inspector again");
        assert_eq!(
            f32::from(inspector.size.width),
            THREAD_INSPECTOR_WIDTH_PX,
            "returned inspector keeps the fixed sidebar width"
        );
    }

    #[test]
    fn workspace_body_type_derives_from_the_shared_helper() {
        // Workspace surfaces inherit 410 / −0.04 em (`docs-responsive-surfaces`);
        // 14 px control text resolves tracking through the shared helper.
        assert_eq!(ProseTypography::BODY_WEIGHT.0, 410.0);
        assert!(
            (ProseTypography::body_tracking_px(14.0) - -0.56).abs() < 1e-6,
            "14px workspace tracking must resolve −0.04 em"
        );
        assert_eq!(ProseTypography::BODY_TRACKING_PX, -0.64);
    }

    #[test]
    fn inspector_row_icons_map_to_the_exact_reference_glyphs() {
        assert_eq!(
            inspector_row_icon(InspectorRowIcon::Project),
            AssetId::TABLER_FOLDER
        );
        assert_eq!(
            inspector_row_icon(InspectorRowIcon::Machine),
            AssetId::TABLER_DEVICE_LAPTOP
        );
        assert_eq!(
            inspector_row_icon(InspectorRowIcon::Changes),
            AssetId::TABLER_FILE_DIFF
        );
        assert_eq!(
            inspector_row_icon(InspectorRowIcon::Branch),
            AssetId::TABLER_GIT_BRANCH
        );
        assert_eq!(
            inspector_row_icon(InspectorRowIcon::Worktree),
            AssetId::TABLER_FOLDER_CODE
        );
        assert_eq!(
            inspector_row_icon(InspectorRowIcon::Terminal),
            AssetId::TABLER_TERMINAL_2
        );
    }

    #[test]
    fn inspector_rows_keep_the_reference_row_geometry() {
        // Rows keep `px-2 py-2` and `rounded-lg` from shared tokens, never
        // local magic; the column pads like the left sidebar.
        assert_eq!(ROW_PAD_PX, 8.0);
        assert_eq!(ROW_GAP_PX, 8.0);
        assert_eq!(DESKTOP_COLUMN_INSET_PX, 10.0);
        assert_eq!(
            RadiusTokens::value(RadiusStep::Lg),
            px(10.0),
            "inspector rows keep the reference rounded-lg"
        );
    }

    /// The host viewport spans the full card: the surface root coincides
    /// with the transcript column (no prose inset), so the navigator rail
    /// anchors to the card edge at wide and narrow widths alike. Reading
    /// rhythm lives one layer down, inside the surface's own turn roots.
    #[gpui::test]
    fn host_viewport_spans_the_full_card_width(cx: &mut gpui::TestAppContext) {
        for (thread, content) in [
            ("shell-proof-bleed-wide", EXPANDED_WIDE_CONTENT),
            ("shell-proof-bleed-narrow", EXPANDED_1280_CONTENT),
        ] {
            let (_view, cx) = cx.add_window_view(|_, cx| mount_proof_screen(thread, content, cx));
            cx.run_until_parked();
            let transcript = cx
                .debug_bounds(THREAD_SCREEN_TRANSCRIPT_SELECTOR)
                .expect("transcript column lays out");
            let surface = cx
                .debug_bounds("artisan-conversation-surface")
                .expect("host surface lays out");
            assert!(
                (f32::from(surface.origin.x) - f32::from(transcript.origin.x)).abs() < 1.0
                    && (f32::from(surface.size.width) - f32::from(transcript.size.width)).abs()
                        < 1.0,
                "surface root must span the full transcript column, not a prose inset"
            );
        }
    }

    /// Transcript and composer share one centered reading column: the
    /// composer card stays centered in the transcript column at wide and
    /// narrow content widths alike, while the card keeps its max-width rule.
    /// Turn-level prose centering lives in the surface suite, where turns
    /// exist to measure.
    #[gpui::test]
    fn composer_card_stays_centered_in_the_column(cx: &mut gpui::TestAppContext) {
        for (thread, content) in [
            ("shell-proof-center-wide", EXPANDED_WIDE_CONTENT),
            ("shell-proof-center-narrow", EXPANDED_1280_CONTENT),
        ] {
            let (_view, cx) = cx.add_window_view(|_, cx| mount_proof_screen(thread, content, cx));
            cx.run_until_parked();
            let transcript = cx
                .debug_bounds(THREAD_SCREEN_TRANSCRIPT_SELECTOR)
                .expect("transcript column lays out");
            let composer = cx
                .debug_bounds(THREAD_SCREEN_COMPOSER_CARD_SELECTOR)
                .expect("composer card lays out");
            let transcript_center =
                f32::from(transcript.origin.x) + f32::from(transcript.size.width) / 2.0;
            let composer_center =
                f32::from(composer.origin.x) + f32::from(composer.size.width) / 2.0;
            assert!(
                (transcript_center - composer_center).abs() < 1.0,
                "transcript center {transcript_center}px must match composer center {composer_center}px"
            );
            assert!(
                f32::from(composer.size.width) <= PROSE_WIDTH_PX + 1.0,
                "composer card keeps its max-width rule"
            );
        }
    }

    /// The composer frame is a bottom overlay, not an in-flow dock: the inner
    /// card wrapper anchors above the route body bottom by exactly the
    /// viewport-rule inset (read from real window bounds, 24 wide / 16
    /// narrow), while strictly overlapping the full-height transcript, so
    /// editor growth never steals transcript height. Tail clearance below the
    /// card is the surface endspace (pending counterpart), asserted nowhere
    /// here.
    #[gpui::test]
    fn composer_frame_floats_over_the_full_height_transcript(cx: &mut gpui::TestAppContext) {
        for (thread, width, height) in [
            ("shell-proof-overlay-wide", 900.0, 600.0),
            ("shell-proof-overlay-narrow", 500.0, 600.0),
        ] {
            let (_view, cx) =
                cx.add_window_view(|_, cx| mount_proof_screen(thread, EXPANDED_WIDE_CONTENT, cx));
            cx.simulate_resize(gpui::size(px(width), px(height)));
            cx.run_until_parked();
            let viewport_width = cx.update(|window, _| f32::from(window.bounds().size.width));
            let expected_pad = composer_pad_bottom(viewport_width);
            let root = cx
                .debug_bounds(THREAD_SCREEN_SELECTOR)
                .expect("screen root lays out");
            let transcript = cx
                .debug_bounds(THREAD_SCREEN_TRANSCRIPT_SELECTOR)
                .expect("transcript column lays out");
            let card = cx
                .debug_bounds(THREAD_SCREEN_COMPOSER_CARD_SELECTOR)
                .expect("composer card wrapper lays out");
            let root_bottom = f32::from(root.origin.y) + f32::from(root.size.height);
            let card_bottom = f32::from(card.origin.y) + f32::from(card.size.height);
            assert!(
                (root_bottom - card_bottom - expected_pad).abs() < 1.0,
                "card must anchor {expected_pad}px above the body bottom for a {viewport_width}px viewport"
            );
            let overlay = cx
                .debug_bounds(THREAD_SCREEN_COMPOSER_SELECTOR)
                .expect("composer overlay lays out");
            let overlay_top = f32::from(overlay.origin.y);
            let transcript_bottom =
                f32::from(transcript.origin.y) + f32::from(transcript.size.height);
            assert!(
                overlay_top + 1.0 < transcript_bottom,
                "overlay top {overlay_top}px must strictly overlap the transcript ending at {transcript_bottom}px (an in-flow dock would sit exactly below it)"
            );
        }
    }

    #[gpui::test]
    fn composer_clearance_keeps_the_last_message_reachable_after_resize(
        cx: &mut gpui::TestAppContext,
    ) {
        let (view, cx) = cx.add_window_view(|_, cx| {
            mount_proof_screen("clearance-proof", EXPANDED_WIDE_CONTENT, cx)
        });
        let surface = cx.update(|_, app| {
            let screen = view.read(app).screen.read(app);
            screen.host.read(app).surface().clone()
        });
        cx.update(|_, app| {
            surface.update(app, |surface, cx| {
                surface.show_queued_text("last line\n".repeat(40), cx);
            });
        });
        for (width, height) in [(500.0, 400.0), (900.0, 700.0), (500.0, 450.0)] {
            cx.simulate_resize(gpui::size(px(width), px(height)));
            cx.run_until_parked();
            cx.update(|_, app| {
                surface.update(app, |surface, cx| {
                    let handle = surface.scroll_handle().clone();
                    assert!(handle.max_offset().y > px(0.0));
                    handle.set_offset(gpui::point(px(0.0), -handle.max_offset().y));
                    cx.notify();
                })
            });
            cx.run_until_parked();
            let spacer = cx
                .debug_bounds("artisan-conversation-surface-end-space")
                .expect("tail clearance");
            let composer = cx
                .debug_bounds(THREAD_SCREEN_COMPOSER_CARD_SELECTOR)
                .expect("composer");
            assert!(
                spacer.top() + px(1.0) < composer.top(),
                "last content must scroll above the card"
            );
            assert!(spacer.size.height >= composer.size.height + px(24.0));
        }
    }

    /// Clicks landing on the composer dock never route into the
    /// transcript: the dock paints and hit-tests in the mounted screen
    /// without queueing any surface scroll intent.
    #[gpui::test]
    fn composer_dock_click_never_reaches_the_transcript(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| {
            mount_proof_screen("shell-proof-composer-click", EXPANDED_WIDE_CONTENT, cx)
        });
        cx.simulate_resize(gpui::size(gpui::px(900.0), gpui::px(600.0)));
        cx.run_until_parked();
        let dock = cx
            .debug_bounds(THREAD_SCREEN_COMPOSER_SELECTOR)
            .expect("composer dock lays out");
        cx.simulate_click(dock.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, app| {
            view.update(app, |probe, probe_cx| {
                let intents = probe
                    .screen
                    .read(probe_cx)
                    .host()
                    .read(probe_cx)
                    .surface()
                    .read(probe_cx)
                    .pending_actions()
                    .iter()
                    .filter(|action| {
                        matches!(
                            action,
                            crate::conversation_surface::ConversationSurfaceAction::ScrollIntent { .. }
                        )
                    })
                    .count();
                assert_eq!(intents, 0, "composer clicks must not scroll the transcript");
            });
        });
    }

    /// Mounted growth evidence: a 12-line draft grows the overlay card while
    /// the transcript keeps its exact full height — the overlay never squeezes
    /// the column it floats above.
    #[gpui::test]
    fn composer_growth_never_steals_transcript_height(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| {
            mount_proof_screen("shell-proof-growth", EXPANDED_WIDE_CONTENT, cx)
        });
        cx.simulate_resize(gpui::size(px(900.0), px(600.0)));
        cx.run_until_parked();
        let transcript_before = cx
            .debug_bounds(THREAD_SCREEN_TRANSCRIPT_SELECTOR)
            .expect("transcript column lays out");
        let card_before = cx
            .debug_bounds(THREAD_SCREEN_COMPOSER_CARD_SELECTOR)
            .expect("composer card wrapper lays out");
        let draft = (1..=12)
            .map(|line| format!("Line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        cx.update(|_, app| {
            view.update(app, |probe, probe_cx| {
                probe.screen.update(probe_cx, |screen, screen_cx| {
                    screen.composer.update(screen_cx, |composer, composer_cx| {
                        composer.set_draft(draft.as_str());
                        composer_cx.notify();
                    });
                });
            });
        });
        cx.run_until_parked();
        let transcript_after = cx
            .debug_bounds(THREAD_SCREEN_TRANSCRIPT_SELECTOR)
            .expect("transcript column lays out after growth");
        let card_after = cx
            .debug_bounds(THREAD_SCREEN_COMPOSER_CARD_SELECTOR)
            .expect("composer card wrapper lays out after growth");
        assert!(
            f32::from(card_after.size.height) > f32::from(card_before.size.height) + 48.0,
            "12-line draft must grow the overlay card, before={card_before:?} after={card_after:?}"
        );
        assert!(
            (f32::from(transcript_after.size.height) - f32::from(transcript_before.size.height))
                .abs()
                < 1.0,
            "transcript must keep its full height while the overlay grows"
        );
    }

    /// Publishes one screen update and lets the window repaint.
    fn update_screen(
        view: &Entity<ShellProofProbe>,
        cx: &mut gpui::VisualTestContext,
        update: impl FnOnce(&mut ThreadScreen) -> bool,
    ) {
        cx.update(|_, app| {
            view.update(app, |probe, probe_cx| {
                probe.screen.update(probe_cx, |screen, screen_cx| {
                    if update(screen) {
                        screen_cx.notify();
                    }
                });
            });
        });
        cx.run_until_parked();
    }

    /// The Context section is flat and unlabelled: its single Machine row
    /// heads the column directly (no label, card inset, frame, or fill),
    /// starting at the sidebar-matched column inset, and the section is
    /// exactly that row.
    #[gpui::test]
    fn context_section_sits_flat_on_the_column(cx: &mut gpui::TestAppContext) {
        let (_view, cx) = cx.add_window_view(|_, cx| {
            mount_proof_screen("shell-proof-density", EXPANDED_WIDE_CONTENT, cx)
        });
        cx.run_until_parked();
        let inspector = cx
            .debug_bounds(THREAD_SCREEN_INSPECTOR_SELECTOR)
            .expect("inspector lays out");
        let section = cx
            .debug_bounds(THREAD_SCREEN_CONTEXT_SELECTOR)
            .expect("context section lays out");
        let row = cx
            .debug_bounds(THREAD_SCREEN_ENV_ROW_SELECTOR)
            .expect("machine row lays out");
        let inset = inspector.origin.x + px(DESKTOP_COLUMN_INSET_PX);
        assert_eq!(row.origin.x, inset, "row starts at the column inset");
        assert_eq!(
            row.origin.y,
            inspector.origin.y + px(DESKTOP_COLUMN_INSET_PX),
            "the first row heads the column"
        );
        assert_eq!(section, row, "the section is exactly its one row");
    }

    /// The panel's left rule is one device pixel wide at the column's left
    /// edge and runs the column's full height (the whole body below the
    /// titlebar); it leaves with the column when the content narrows.
    #[gpui::test]
    fn inspector_left_rule_follows_the_column(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| {
            mount_proof_screen("shell-proof-rule", EXPANDED_WIDE_CONTENT, cx)
        });
        cx.run_until_parked();
        let root = cx
            .debug_bounds(THREAD_SCREEN_SELECTOR)
            .expect("screen root lays out");
        let inspector = cx
            .debug_bounds(THREAD_SCREEN_INSPECTOR_SELECTOR)
            .expect("inspector lays out");
        let rule = cx
            .debug_bounds(THREAD_SCREEN_INSPECTOR_RULE_SELECTOR)
            .expect("visible inspector paints its left rule");
        let one_device_pixel = cx.update(|window, _| {
            DesktopShellStyle::resolve(false, window.scale_factor()).one_device_pixel
        });
        assert_eq!(rule.origin.x, inspector.origin.x);
        assert_eq!(rule.size.width, one_device_pixel);
        assert_eq!(rule.origin.y, root.origin.y);
        assert_eq!(rule.size.height, root.size.height);
        let visible_width =
            cx.update(|_, app| view.read(app).screen.read(app).visible_inspector_width());
        assert_eq!(visible_width, Some(px(THREAD_INSPECTOR_WIDTH_PX)));

        update_screen(&view, cx, |screen| {
            screen.set_content_width(EXPANDED_1280_CONTENT)
        });
        assert!(
            cx.debug_bounds(THREAD_SCREEN_INSPECTOR_RULE_SELECTOR)
                .is_none()
        );
        let hidden_width =
            cx.update(|_, app| view.read(app).screen.read(app).visible_inspector_width());
        assert_eq!(hidden_width, None, "a hidden column publishes no junction");
    }

    /// The mounted column crosses the 1104 px threshold in both directions:
    /// exactly at it the 240 px minimum column seats, one pixel below it is
    /// gone with no reserved space, and it returns on widening.
    #[gpui::test]
    fn mounted_inspector_toggles_at_the_threshold(cx: &mut gpui::TestAppContext) {
        let threshold = 1104.0;
        let (view, cx) =
            cx.add_window_view(|_, cx| mount_proof_screen("shell-proof-threshold", threshold, cx));
        cx.simulate_resize(gpui::size(px(threshold), px(800.0)));
        cx.run_until_parked();
        for (content, visible) in [
            (threshold, true),
            (threshold - 1.0, false),
            (threshold, true),
        ] {
            update_screen(&view, cx, |screen| screen.set_content_width(content));
            let inspector = cx.debug_bounds(THREAD_SCREEN_INSPECTOR_SELECTOR);
            assert_eq!(inspector.is_some(), visible, "content={content}");
            if let Some(inspector) = inspector {
                assert_eq!(f32::from(inspector.size.width), 240.0);
            } else {
                let transcript = cx
                    .debug_bounds(THREAD_SCREEN_TRANSCRIPT_SELECTOR)
                    .expect("transcript lays out");
                let root = cx
                    .debug_bounds(THREAD_SCREEN_SELECTOR)
                    .expect("screen root lays out");
                assert_eq!(
                    transcript.size.width, root.size.width,
                    "a hidden inspector reserves no space"
                );
            }
        }
    }

    /// With the rail collapsed the same fit seats the column at 1400 px total,
    /// still exactly 327 px wide rather than growing with the freed room.
    #[gpui::test]
    fn collapsed_rail_keeps_the_fixed_inspector_width(cx: &mut gpui::TestAppContext) {
        let (_view, cx) = cx.add_window_view(|_, cx| {
            mount_proof_screen("shell-proof-collapsed", COLLAPSED_1400_CONTENT, cx)
        });
        cx.run_until_parked();
        let inspector = cx
            .debug_bounds(THREAD_SCREEN_INSPECTOR_SELECTOR)
            .expect("collapsed rail seats the inspector");
        assert_eq!(f32::from(inspector.size.width), THREAD_INSPECTOR_WIDTH_PX);
    }

    /// Sections read Context, Agents, Checklist, Terminals top to bottom.
    /// Context is unlabelled; the others open with their muted label and
    /// render only with content.
    #[gpui::test]
    fn inspector_sections_follow_their_content(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| {
            mount_proof_screen("shell-proof-sections", EXPANDED_WIDE_CONTENT, cx)
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds(THREAD_SCREEN_CONTEXT_SELECTOR).is_some());
        assert!(cx.debug_bounds(THREAD_SCREEN_AGENTS_SELECTOR).is_none());
        assert!(cx.debug_bounds(THREAD_SCREEN_CHECKLIST_SELECTOR).is_none());
        assert!(cx.debug_bounds(THREAD_SCREEN_TERMINALS_SELECTOR).is_none());

        update_screen(&view, cx, |screen| {
            screen.set_agents(vec![ThreadAgentEntry {
                id: String::from("agent-1"),
                name: String::from("Wire the inspector feed"),
                state: ThreadAgentState::Working,
            }]);
            screen.set_checklist(vec![ThreadChecklistEntry {
                id: String::from("entry-1"),
                state: ChecklistEntryState::Active,
                text: String::from("Port the inspector"),
            }]);
            screen.set_terminals(vec![TerminalSession::new(
                "t1",
                "pwsh",
                Vec::<String>::new(),
                TerminalState::Active,
            )]);
            true
        });
        let tops = [
            (
                THREAD_SCREEN_CONTEXT_SELECTOR,
                THREAD_SCREEN_ENV_ROW_SELECTOR,
            ),
            (
                THREAD_SCREEN_AGENTS_SELECTOR,
                THREAD_SCREEN_AGENTS_LABEL_SELECTOR,
            ),
            (
                THREAD_SCREEN_CHECKLIST_SELECTOR,
                THREAD_SCREEN_CHECKLIST_LABEL_SELECTOR,
            ),
            (
                THREAD_SCREEN_TERMINALS_SELECTOR,
                THREAD_SCREEN_TERMINALS_LABEL_SELECTOR,
            ),
        ]
        .map(|(section, head_selector)| {
            let head = cx
                .debug_bounds(head_selector)
                .unwrap_or_else(|| panic!("{section} head lays out"));
            let bounds = cx
                .debug_bounds(section)
                .unwrap_or_else(|| panic!("{section} lays out"));
            assert_eq!(
                head.origin, bounds.origin,
                "{section} opens with its label, or its first row when unlabelled"
            );
            bounds.origin.y
        });
        assert!(
            tops.windows(2).all(|pair| pair[0] < pair[1]),
            "sections must read Context, Agents, Checklist, Terminals: {tops:?}"
        );

        update_screen(&view, cx, |screen| {
            screen.set_terminals(Vec::new());
            screen.set_terminals_loading(true);
            true
        });
        assert!(
            cx.debug_bounds("artisan-thread-screen-terminals-loading")
                .is_some(),
            "loading keeps the Terminals section with its bars"
        );
        update_screen(&view, cx, |screen| {
            screen.set_terminals_loading(false);
            screen.set_checklist(Vec::new());
            screen.set_agents(Vec::new());
            true
        });
        assert!(cx.debug_bounds(THREAD_SCREEN_AGENTS_SELECTOR).is_none());
        assert!(cx.debug_bounds(THREAD_SCREEN_CHECKLIST_SELECTOR).is_none());
        assert!(cx.debug_bounds(THREAD_SCREEN_TERMINALS_SELECTOR).is_none());
    }

    /// The Project row appears only once a project label is published, and
    /// then leads the Context section above Machine.
    #[gpui::test]
    fn project_row_renders_only_with_a_label(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| {
            mount_proof_screen("shell-proof-project", EXPANDED_WIDE_CONTENT, cx)
        });
        cx.run_until_parked();
        assert!(
            cx.debug_bounds(THREAD_SCREEN_PROJECT_ROW_SELECTOR)
                .is_none()
        );

        update_screen(&view, cx, |screen| {
            screen.set_project_label(Some(String::from("Artisan Editor")))
        });
        let project = cx
            .debug_bounds(THREAD_SCREEN_PROJECT_ROW_SELECTOR)
            .expect("labelled project renders its row");
        let machine = cx
            .debug_bounds(THREAD_SCREEN_ENV_ROW_SELECTOR)
            .expect("machine row lays out");
        let section = cx
            .debug_bounds(THREAD_SCREEN_CONTEXT_SELECTOR)
            .expect("context section lays out");
        assert_eq!(project.origin, section.origin, "Project heads the section");
        assert!(project.bottom() <= machine.top());
        assert_eq!(project.size.height, machine.size.height);

        update_screen(&view, cx, |screen| screen.set_project_label(None));
        assert!(
            cx.debug_bounds(THREAD_SCREEN_PROJECT_ROW_SELECTOR)
                .is_none()
        );
    }
    #[gpui::test]
    fn jump_circle_stays_centered_above_composer_after_resize(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|_, cx| {
            mount_proof_screen("jump-circle-proof", EXPANDED_WIDE_CONTENT, cx)
        });
        let surface = cx.update(|_, app| {
            view.read(app)
                .screen
                .read(app)
                .host
                .read(app)
                .surface()
                .clone()
        });
        cx.update(|_, app| {
            surface.update(app, |surface, cx| {
                surface.set_jump_to_latest_visible(true, cx)
            })
        });
        for (width, height) in [(500.0, 400.0), (900.0, 700.0)] {
            cx.simulate_resize(gpui::size(px(width), px(height)));
            cx.run_until_parked();
            let card = cx
                .debug_bounds(THREAD_SCREEN_COMPOSER_CARD_SELECTOR)
                .expect("composer");
            let button = cx
                .debug_bounds(crate::conversation_surface::JUMP_TO_LATEST_SELECTOR)
                .expect("jump button");
            assert_eq!(button.size, gpui::size(px(32.0), px(32.0)));
            assert!((button.center().x - card.center().x).abs() < px(1.0));
            assert!(
                (card.top() - button.bottom() - px(8.0)).abs() < px(1.0),
                "button {button:?} must clear composer {card:?} by 8px"
            );
        }
    }
}
