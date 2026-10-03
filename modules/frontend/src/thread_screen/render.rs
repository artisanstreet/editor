//! Render tree composition for [`ThreadScreen`].
//!
//! Extracted verbatim from `thread_screen.rs` during the module split;
//! `is_live_terminal` was widened to `pub(super)` for the parent test suite.

#[allow(clippy::wildcard_imports)]
use super::*;

/// The muted label heading one inspector section, with its debug selector.
#[derive(Clone, Copy)]
struct InspectorSectionLabel {
    selector: &'static str,
    text: &'static str,
}

/// Builds one flat inspector section: its rows directly on the black column
/// — no card frame, fill, blur, or shadow — under the shared desktop section
/// label (the left sidebar's thread-age group treatment) when it has one.
/// The Context rows name themselves, so that section carries no label.
fn inspector_section(
    theme: &ArtisanTheme,
    selector: &'static str,
    label: Option<InspectorSectionLabel>,
    rows: impl IntoElement,
) -> Div {
    div()
        .flex()
        .w_full()
        .min_w_0()
        .flex_col()
        .debug_selector(move || selector.to_owned())
        .children(label.map(|label| {
            desktop_section_label(theme, label.text)
                .debug_selector(move || label.selector.to_owned())
        }))
        .child(rows)
}

/// Builds one 16 px muted inspector row glyph (`size-4 text-muted-foreground`).
fn inspector_row_glyph(theme: &ArtisanTheme, kind: InspectorRowIcon) -> impl IntoElement {
    icon(IconStyle::resolve(
        *theme,
        inspector_row_icon(kind),
        IconSize::Default,
        IconTint::Muted,
    ))
    .flex_shrink_0()
}

/// Workspace body tracking at the control size: −0.04 em resolved through
/// the shared helper, so 14 px workspace text takes −0.56 px
/// (`docs-responsive-surfaces`, `ProseTypography::body_tracking_px`).
fn workspace_body_tracking(theme: &ArtisanTheme) -> f32 {
    ProseTypography::body_tracking_px(f32::from(theme.typography.control_text))
}

impl ThreadScreen {
    /// Renders the transcript column: the live conversation host at full
    /// card width. A thread with no turns yet shows nothing here; the
    /// new-thread screen owns the empty prompt.
    ///
    /// The host owns the full column so the surface root — the turn
    /// navigator rail's positioning context — spans the card, not the prose
    /// box: the rail anchors right-8 of the card and centers in it. Reading
    /// rhythm stays prose-bound one layer down, inside the surface itself:
    /// each turn root carries the shared max-width, auto margins, and
    /// gutters, so standalone surface fixtures keep the identical column
    /// without this frame. The 40 px top spacing lives inside the scroll
    /// content (owned by the surface).
    fn render_transcript_column(&self, cx: &App) -> impl IntoElement {
        div()
            .relative()
            .min_h_0()
            .flex_1()
            .overflow_hidden()
            .bg(shell_black())
            .debug_selector(|| THREAD_SCREEN_TRANSCRIPT_SELECTOR.to_owned())
            // Cached: the transcript renders when it changes, not when the
            // inspector, the composer, or the sidebar beside it does.
            .child(div().w_full().h_full().child(crate::view_boundary::cached_view(
                self.host.clone(),
                gpui::StyleRefinement::default().size_full(),
                cx,
            )))
    }

    /// Renders one Context row: glyph, flexible label, truncating value.
    ///
    /// Legacy frame: `div.flex.min-w-0.items-center.gap-2.rounded-lg.px-2.py-2`
    /// with the `size-4 text-muted-foreground` row glyph, a `flex-1` label,
    /// and a `max-w-36 truncate` value
    /// (`thread-environment-card.svelte:372-376`). Rows are read-only facts:
    /// no id, hover, or chevron implies a click.
    fn render_environment_row(
        label: &str,
        value: String,
        icon: InspectorRowIcon,
        selector: &'static str,
        theme: &ArtisanTheme,
    ) -> impl IntoElement {
        div()
            .flex()
            .min_w_0()
            .items_center()
            .gap(px(ROW_GAP_PX))
            .rounded(RadiusTokens::value(RadiusStep::Lg))
            .px(px(ROW_PAD_PX))
            .py(px(ROW_PAD_PX))
            .debug_selector(move || selector.to_owned())
            .child(inspector_row_glyph(theme, icon))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(theme.typography.control_text)
                    .font_weight(ProseTypography::BODY_WEIGHT)
                    .letter_spacing(px(workspace_body_tracking(theme)))
                    .text_color(theme.colors.foreground.to_paint())
                    .child(label.to_owned()),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .max_w(px(ENV_VALUE_MAX_WIDTH_PX))
                    .truncate()
                    .text_size(theme.typography.control_text)
                    .font_weight(ProseTypography::BODY_WEIGHT)
                    .letter_spacing(px(workspace_body_tracking(theme)))
                    .text_color(theme.colors.foreground.to_paint())
                    .child(value),
            )
    }

    /// Renders the Context section: the environment card's rows
    /// (`thread-environment-card.svelte`), unlabelled because each row names
    /// its own fact.
    ///
    /// Rows, in order: Project (the published project display name, only
    /// when one is set), Machine (always), then Changes, Branch, and Worktree
    /// only when the environment projection has them. The project row is a
    /// read-only label, not the legacy project picker; the remote chip is
    /// icon-only in legacy with a host-mark brand glyph that has no exact
    /// catalog entry, so it stays a gap rather than a fake chip.
    fn render_context_section(&self, theme: &ArtisanTheme) -> impl IntoElement {
        let projection = present_thread_environment(&self.environment);
        let mut rows = div()
            .flex()
            .min_w_0()
            .flex_col()
            .text_size(theme.typography.control_text);
        if let Some(project_label) = self.project_label.clone() {
            rows = rows.child(Self::render_environment_row(
                "Project",
                project_label,
                InspectorRowIcon::Project,
                THREAD_SCREEN_PROJECT_ROW_SELECTOR,
                theme,
            ));
        }
        rows = rows.child(Self::render_environment_row(
            "Machine",
            projection.machine_label,
            InspectorRowIcon::Machine,
            THREAD_SCREEN_ENV_ROW_SELECTOR,
            theme,
        ));
        if let Some(summary) = projection.change_summary {
            rows = rows.child(
                div()
                    .flex()
                    .min_w_0()
                    .items_center()
                    .gap(px(ROW_GAP_PX))
                    .rounded(RadiusTokens::value(RadiusStep::Lg))
                    .px(px(ROW_PAD_PX))
                    .py(px(ROW_PAD_PX))
                    .debug_selector(|| THREAD_SCREEN_ENV_ROW_SELECTOR.to_owned())
                    .child(inspector_row_glyph(theme, InspectorRowIcon::Changes))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(theme.typography.control_text)
                            .font_weight(ProseTypography::BODY_WEIGHT)
                            .letter_spacing(px(workspace_body_tracking(theme)))
                            .text_color(theme.colors.foreground.to_paint())
                            .child("Changes"),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(theme.typography.control_text)
                            // Mono numerals keep normal tracking
                            // (`utilities.css:717-719` code rule).
                            .letter_spacing(px(0.0))
                            .text_color(rgb_to_hsla(rgb(ADDED_LINES_GREEN)))
                            .child(format!("+{}", summary.lines_added)),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(theme.typography.control_text)
                            .letter_spacing(px(0.0))
                            .text_color(rgb_to_hsla(rgb(DELETED_LINES_RED)))
                            .child(format!("{MINUS_SIGN}{}", summary.lines_deleted)),
                    ),
            );
        }
        if let Some(branch_label) = projection.current_branch_label {
            rows = rows.child(Self::render_environment_row(
                "Branch",
                branch_label,
                InspectorRowIcon::Branch,
                THREAD_SCREEN_ENV_ROW_SELECTOR,
                theme,
            ));
        }
        if let Some(worktree_label) = projection.current_worktree_label {
            rows = rows.child(Self::render_environment_row(
                "Worktree",
                worktree_label,
                InspectorRowIcon::Worktree,
                THREAD_SCREEN_ENV_ROW_SELECTOR,
                theme,
            ));
        }
        inspector_section(theme, THREAD_SCREEN_CONTEXT_SELECTOR, None, rows)
    }

    /// Renders the Terminals section (`thread-terminals-card.svelte`).
    ///
    /// Legacy branches: a skeleton shimmer while loading (`flex flex-col
    /// gap-2` with `h-4` bars at 3/5 and 2/5 widths, here inset like the rows
    /// so the bars start on the row text edge), the rows only when at least
    /// one live terminal exists, and nothing otherwise — the section and its
    /// label then take no space. Liveness is `opening | active`
    /// (`lib/terminal/presentation.ts` `is_live_terminal`); exited terminals
    /// disappear like finished agents. Rows follow `thread-terminals.svelte`:
    /// the `terminal-2` glyph plus display name plus muted command line.
    /// Click-to-inspect and the tail-viewer dialog need transport wiring and
    /// are gaps, so rows render without a fake affordance.
    fn render_terminals_section(&self, theme: &ArtisanTheme) -> Option<Div> {
        if self.terminals_loading {
            let bar = |fraction: f32| {
                div()
                    .h(px(LOADING_BAR_PX))
                    .w(gpui::relative(fraction))
                    .rounded(px(4.0))
                    .bg(theme.colors.muted.to_paint())
            };
            return Some(inspector_section(
                theme,
                THREAD_SCREEN_TERMINALS_SELECTOR,
                Some(InspectorSectionLabel {
                    selector: THREAD_SCREEN_TERMINALS_LABEL_SELECTOR,
                    text: "Terminals",
                }),
                div()
                    .flex()
                    .flex_col()
                    .gap(px(ROW_GAP_PX))
                    .px(px(ROW_PAD_PX))
                    .py(px(ROW_PAD_PX))
                    .debug_selector(|| String::from("artisan-thread-screen-terminals-loading"))
                    .child(bar(0.6))
                    .child(bar(0.4)),
            ));
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
                    .gap(px(TERMINAL_ROW_GAP_PX))
                    .rounded(RadiusTokens::value(RadiusStep::Lg))
                    .px(px(ROW_PAD_PX))
                    .py(px(ROW_PAD_PX))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(px(ROW_GAP_PX))
                            .child(inspector_row_glyph(theme, InspectorRowIcon::Terminal))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(theme.typography.control_text)
                                    .font_weight(ProseTypography::BODY_WEIGHT)
                                    .letter_spacing(px(workspace_body_tracking(theme)))
                                    .text_color(theme.colors.foreground.to_paint())
                                    .child(terminal_display_name(session)),
                            ),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .max_w(px(160.0))
                            .truncate()
                            .text_size(theme.typography.label_text)
                            // Mono command keeps normal tracking
                            // (`utilities.css:717-719` code rule).
                            .letter_spacing(px(0.0))
                            .text_color(theme.colors.muted_foreground.to_paint())
                            .child(terminal_command_line(session)),
                    ),
            );
        }
        Some(inspector_section(
            theme,
            THREAD_SCREEN_TERMINALS_SELECTOR,
            Some(InspectorSectionLabel {
                selector: THREAD_SCREEN_TERMINALS_LABEL_SELECTOR,
                text: "Terminals",
            }),
            list,
        ))
    }

    /// Renders the Agents section (`thread-agents.svelte`): the subagents
    /// the thread's current run has started, in the order they started.
    ///
    /// Each row is the `bot-id` glyph, the subagent's task truncated to one
    /// line, and its state at the right edge: a spinning arc while it works,
    /// then the reference state dot — sky (`--unread`) once it completed,
    /// destructive when it failed. Click-to-inspect needs the agent
    /// transcript viewer and is a gap, so rows render without a fake
    /// affordance. The section is omitted while there are no agents.
    fn render_agents_section(&self, theme: &ArtisanTheme) -> Option<Div> {
        if self.agents.is_empty() {
            return None;
        }
        let mut list = div().flex().min_w_0().flex_col();
        for agent in &self.agents {
            let row_selector = format!("{THREAD_SCREEN_AGENT_ROW_PREFIX}-{}", agent.id);
            let state_selector = format!("{row_selector}-state");
            let state = div()
                .flex_shrink_0()
                .size(px(AGENT_STATE_SLOT_PX))
                .flex()
                .items_center()
                .justify_center()
                .debug_selector({
                    let selector = state_selector.clone();
                    move || selector.clone()
                });
            let state = match agent.state {
                ThreadAgentState::Working => state.child(
                    FadeArc::new(SharedString::from(state_selector), *theme)
                        .size(px(AGENT_STATE_SLOT_PX)),
                ),
                ThreadAgentState::Completed => state.child(
                    div()
                        .size(px(AGENT_STATE_DOT_PX))
                        .rounded_full()
                        .bg(theme.colors.unread.to_paint()),
                ),
                ThreadAgentState::Failed => state.child(
                    div()
                        .size(px(AGENT_STATE_DOT_PX))
                        .rounded_full()
                        .bg(theme.colors.destructive.to_paint()),
                ),
            };
            list = list.child(
                div()
                    .flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .gap(px(ROW_GAP_PX))
                    .rounded(RadiusTokens::value(RadiusStep::Lg))
                    .px(px(ROW_PAD_PX))
                    .py(px(ROW_PAD_PX))
                    .debug_selector(move || row_selector.clone())
                    .child(inspector_row_glyph(theme, InspectorRowIcon::Agent))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(theme.typography.control_text)
                            .font_weight(ProseTypography::BODY_WEIGHT)
                            .letter_spacing(px(workspace_body_tracking(theme)))
                            .text_color(theme.colors.foreground.to_paint())
                            .child(agent.name.clone()),
                    )
                    .child(state),
            );
        }
        Some(inspector_section(
            theme,
            THREAD_SCREEN_AGENTS_SELECTOR,
            Some(InspectorSectionLabel {
                selector: THREAD_SCREEN_AGENTS_LABEL_SELECTOR,
                text: "Agents",
            }),
            list,
        ))
    }

    /// Renders the Checklist section (`thread-panel.svelte` plan section).
    ///
    /// The `rounded-lg.px-2.py-2.text-sm` rows sit under the muted
    /// "Checklist" label and the section is omitted while the checklist is
    /// empty. `font-medium` resolves through the redefined token to the
    /// workspace 410, so every row takes it with surface tracking; tone maps
    /// the exact legacy classes to theme colors otherwise: active keeps
    /// foreground; completed/pending/skipped (`text-muted-foreground`, with
    /// `line-through` on completed/skipped) use the muted token with
    /// strikethrough where legacy crosses out. The `list-disc` markers and
    /// the screen-reader `"{state}: "` prefix have no GPUI equivalent on
    /// plain text and stay gaps rather than faked bullets.
    fn render_checklist_section(&self, theme: &ArtisanTheme) -> Option<Div> {
        if self.checklist.is_empty() {
            return None;
        }
        let mut list = div().flex().min_w_0().flex_col();
        for entry in &self.checklist {
            let presented = present_checklist_entry(ChecklistEntry::new(
                entry.id.as_str(),
                entry.state,
                entry.text.as_str(),
            ));
            let mut row = div()
                .rounded(RadiusTokens::value(RadiusStep::Lg))
                .px(px(ROW_PAD_PX))
                .py(px(ROW_PAD_PX))
                .text_size(theme.typography.control_text)
                // Every row inherits the workspace 410 (`docs-responsive-surfaces`
                // scope; `font-medium` resolves through the redefined token);
                // tone and strikethrough stay per state below.
                .font_weight(ProseTypography::BODY_WEIGHT)
                .letter_spacing(px(workspace_body_tracking(theme)))
                .debug_selector(|| {
                    format!("artisan-thread-screen-checklist-entry-{}", presented.id)
                });
            row = match presented.state {
                ChecklistEntryState::Active => row.text_color(theme.colors.foreground.to_paint()),
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
        Some(inspector_section(
            theme,
            THREAD_SCREEN_CHECKLIST_SELECTOR,
            Some(InspectorSectionLabel {
                selector: THREAD_SCREEN_CHECKLIST_LABEL_SELECTOR,
                text: "Checklist",
            }),
            list,
        ))
    }

    /// Renders the inspector column (`thread-panel.svelte` root) as the
    /// ruled right sidebar of the desktop shell.
    ///
    /// A flat true-black column `width` wide — the expanded left sidebar's
    /// [`THREAD_INSPECTOR_WIDTH_PX`] once the window has room, narrower down to
    /// the shared column minimum before it hides — padded by the sidebar's
    /// [`DESKTOP_COLUMN_INSET_PX`] so labels and row text sit 18 px in, like
    /// the left column. Sections stack Context, Agents, Checklist, Terminals,
    /// spaced by the sidebar's 12 px group gap. The left edge carries a
    /// one-device-pixel rule in the shell's line paint (`rule`) spanning the
    /// column's full height, i.e. the whole body below the titlebar; it is a
    /// plain absolute element with no pointer listener, so it cannot
    /// intercept input. The titlebar end of that rule is the shell's
    /// junction crosshair, fed by [`ThreadScreen::visible_inspector_width`].
    ///
    /// A rule closes the Context section across the column's full width, so
    /// the inspector reads as a grid with the titlebar rule above it; its
    /// junction with the left rule carries the shell's crosshair.
    fn render_inspector(&self, theme: &ArtisanTheme, width: f32, rule: Pixels) -> impl IntoElement {
        let section_gap = theme.spacing.steps(3.0);
        let mut sections = div()
            .flex()
            .min_h_0()
            .flex_1()
            .flex_col()
            .gap(section_gap)
            .child(self.render_context_section(theme))
            .child(Self::render_context_rule(width, rule, section_gap));
        if let Some(agents) = self.render_agents_section(theme) {
            sections = sections.child(agents);
        }
        if let Some(checklist) = self.render_checklist_section(theme) {
            sections = sections.child(checklist);
        }
        if let Some(terminals) = self.render_terminals_section(theme) {
            sections = sections.child(terminals);
        }
        div()
            .relative()
            .flex_shrink_0()
            .w(px(width))
            .min_h_0()
            .flex()
            .flex_col()
            .p(px(DESKTOP_COLUMN_INSET_PX))
            .bg(shell_black())
            .debug_selector(|| THREAD_SCREEN_INSPECTOR_SELECTOR.to_owned())
            .child(
                div()
                    .absolute()
                    .left(px(0.0))
                    .top(px(0.0))
                    .bottom(px(0.0))
                    .w(rule)
                    .bg(DesktopTheme::neutral_dark().line)
                    .debug_selector(|| THREAD_SCREEN_INSPECTOR_RULE_SELECTOR.to_owned()),
            )
            .child(sections)
    }

    /// The full-width rule under the Context section.
    ///
    /// It escapes the column padding to run edge to edge, and sits the
    /// column inset away from the rows on both sides (the section gap plus
    /// a signed margin), the spacing the titlebar rule keeps above the
    /// first row.
    /// The crosshair centres both arms on the junction with the left rule;
    /// like that rule it is a plain element that cannot intercept input.
    fn render_context_rule(width: f32, rule: Pixels, section_gap: Pixels) -> impl IntoElement {
        let line = DesktopTheme::neutral_dark();
        // Negative when the gap exceeds the inset: the rule then pulls both
        // neighbours in so it still sits exactly the inset from each.
        let breathing = px(DESKTOP_COLUMN_INSET_PX) - section_gap;
        let arm_offset = (px(DESKTOP_CROSSHAIR_SIZE_PX) - rule) / 2.0;
        div()
            .relative()
            .flex_none()
            .w(px(width))
            .h(rule)
            .ml(px(-DESKTOP_COLUMN_INSET_PX))
            .my(breathing)
            .bg(line.line)
            .debug_selector(|| THREAD_SCREEN_CONTEXT_RULE_SELECTOR.to_owned())
            .child(
                junction_crosshair(line, rule)
                    .left(-arm_offset)
                    .top(-arm_offset),
            )
    }

    /// Renders the composer overlay frame around the packet-2 composer surface.
    ///
    /// Legacy frame (`thread-composer.svelte:526`): `prose-column-frame
    /// pointer-events-none absolute inset-x-0 bottom-0` with `pb-4 sm:pb-6`,
    /// holding `prose-column w-full max-w-(--prose-width)` children. GPUI has
    /// no pointer-events API; the container is a plain div, so only genuinely
    /// interactive descendants (card drop target, buttons, editor) intercept
    /// and transcript clicks pass around them — the native equivalent of the
    /// pass-through frame. The `px-6` frame gutters reproduce the
    /// `.prose-column-frame` bound (`min(prose, 100% − 3rem)`): identical
    /// geometry in narrow (24 px gutters) and wide (768 px centered) regimes.
    /// The frame paints after the transcript, so it sits above it. Tail
    /// clearance below the card is the transcript end space (surface lane,
    /// pending): this frame reserves nothing itself.
    fn render_composer_overlay(&self, pad_bottom_px: f32, cx: &Context<Self>) -> impl IntoElement {
        let surface = self.host.read(cx).surface().clone();
        div()
            .absolute()
            .left(px(0.0))
            .right(px(0.0))
            .bottom(px(0.0))
            .flex()
            .flex_col()
            .items_center()
            .px(px(COLUMN_PAD_X_PX))
            .pb(px(pad_bottom_px))
            .debug_selector(|| THREAD_SCREEN_COMPOSER_SELECTOR.to_owned())
            .on_children_prepainted(move |children, window, app| {
                let Some(card) = children.first() else {
                    return;
                };
                let height = f32::from(card.size.height) + pad_bottom_px + 24.0;
                let id = window.window_handle().window_id();
                let surface = surface.clone();
                window.defer(app, move |_, app| {
                    surface.update(app, |surface, cx| {
                        surface.set_composer_clearance(id, height, cx);
                    });
                });
            })
            .child(
                div()
                    .w_full()
                    .max_w(px(PROSE_WIDTH_PX))
                    .debug_selector(|| THREAD_SCREEN_COMPOSER_CARD_SELECTOR.to_owned())
                    .child(self.composer.clone()),
            )
    }

    /// Renders the gate loading branch (`thread-route-gate.svelte`).
    ///
    /// Legacy frame: `div.flex.h-full.min-h-0.items-center.justify-center`
    /// with `role="status"` and `aria-label="Loading thread"`. The mark is
    /// the shimmering stacked wordmark the application loads behind (the
    /// legacy connection overlay's loader), not a spinning arc. GPUI divs
    /// carry no DOM roles; the stable selector keeps the branch addressable
    /// instead.
    fn render_loading(theme: &ArtisanTheme) -> impl IntoElement {
        div()
            .flex()
            .h_full()
            .min_h_0()
            .items_center()
            .justify_center()
            .bg(shell_black())
            .debug_selector(|| THREAD_SCREEN_LOADING_SELECTOR.to_owned())
            .child(
                // Most threads present well inside a second; the mark only
                // appears for a wait that is actually noticeable.
                WordmarkLoader::new(THREAD_SCREEN_LOADING_SELECTOR, *theme)
                    .appear_after(THREAD_SCREEN_LOADING_MARK_DELAY)
                    .debug_selector(THREAD_SCREEN_LOADING_MARK_SELECTOR),
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
        let retry = Button::new(
            THREAD_SCREEN_RETRY_SELECTOR,
            retry_focus,
            *theme,
            MotionPolicy::Reduced,
            ButtonVariant::Outline,
            ButtonSize::Small,
            ButtonContent::text("Retry"),
        )
        .ok()
        .map(|button| {
            button
                .focus_visibility(FocusVisibility::Visible)
                .debug_selector(THREAD_SCREEN_RETRY_SELECTOR)
                .disabled(!retry_ready)
        });
        let retry = if let Some(on_retry) = on_retry {
            retry.map(|button| {
                button.on_activate(move |_, window, app| {
                    on_retry(window, app);
                })
            })
        } else {
            retry
        };
        let mut column = div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(12.0))
            .max_w(px(FAILURE_MAX_WIDTH_PX))
            .child(
                div()
                    .text_size(theme.typography.control_text)
                    .font_weight(ProseTypography::BODY_WEIGHT)
                    .letter_spacing(px(workspace_body_tracking(theme)))
                    .text_color(theme.colors.destructive.to_paint())
                    .child(message.to_owned()),
            );
        if let Some(retry) = retry {
            column = column.child(retry);
        }
        div()
            .flex()
            .h_full()
            .min_h_0()
            .items_center()
            .justify_center()
            .px(px(COLUMN_PAD_X_PX))
            .bg(shell_black())
            .debug_selector(|| THREAD_SCREEN_FAILURE_SELECTOR.to_owned())
            .child(column)
    }

    /// Renders the opened-route branch (`thread-workspace.svelte` frame).
    ///
    /// Legacy frame (`sectioned-panel.svelte`): the primary column owns the
    /// transcript full-height while the composer floats as the absolute bottom
    /// overlay; the inspector is a separate `{#if secondary}` column with no
    /// reserved space when closed. The conversation wrapper below replays that
    /// ownership — transcript at full column height with the overlay above its
    /// tail — so the composer can never extend across or cover the inspector,
    /// and composer growth never steals transcript height; when the inspector
    /// hides, the conversation reclaims its space. The column's width and
    /// visibility both follow the published content width
    /// ([`thread_inspector_width`]).
    fn render_open(
        &self,
        theme: &ArtisanTheme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // The frame inset follows the live viewport instead of assuming
        // desktop geometry.
        let pad_bottom = composer_pad_bottom(f32::from(window.bounds().size.width));
        let conversation = div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .bg(shell_black())
            .child(self.render_transcript_column(cx))
            .child(self.render_composer_overlay(pad_bottom, cx));
        let mut row = div()
            .flex()
            .flex_row()
            .min_h_0()
            .flex_1()
            .bg(shell_black())
            .child(conversation);
        if let Some(width) = self.inspector_width() {
            // The same display-aware stroke the shell draws its rules and
            // junction marks with, so the rule stays one physical pixel.
            let rule = DesktopShellStyle::resolve(false, window.scale_factor()).one_device_pixel;
            row = row.child(self.render_inspector(theme, width, rule));
        }
        div()
            .relative()
            .flex()
            .flex_col()
            .h_full()
            .min_h_0()
            .bg(shell_black())
            .debug_selector(|| THREAD_SCREEN_SELECTOR.to_owned())
            .child(row)
    }
}

/// Returns whether a terminal session stays visible in the Terminals section.
///
/// This is the legacy `is_live_terminal` boundary
/// (`lib/terminal/presentation.ts`): `opening | active` sessions show;
/// exited terminals disappear like finished agents.
pub(super) fn is_live_terminal(session: &TerminalSession) -> bool {
    matches!(
        session.state,
        TerminalState::Opening | TerminalState::Active
    )
}

impl Render for ThreadScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = ArtisanTheme::for_mode(self.theme_mode);
        match self.gate_branch() {
            ThreadRouteGateRender::OpenedRoute => {
                self.render_open(&theme, window, cx).into_any_element()
            }
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
                .bg(shell_black())
                .debug_selector(|| THREAD_SCREEN_SELECTOR.to_owned())
                .into_any_element(),
        }
    }
}
