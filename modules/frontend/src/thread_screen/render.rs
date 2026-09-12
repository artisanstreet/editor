//! Render tree composition for [`ThreadScreen`].
//!
//! Extracted verbatim from `thread_screen.rs` during the module split;
//! `is_live_terminal` was widened to `pub(super)` for the parent test suite.

#[allow(clippy::wildcard_imports)]
use super::*;

/// Builds one inspector glass card: the native `ShaderGlassSurface` default
/// (quiet) treatment at the `radius-xl` (14 px) card radius, with the material
/// and highlight paint layers from the shared composer/picker helper. Content
/// carries its own padding (card `p-1`, loading `p-3`) like the reference.
fn inspector_glass_card(theme: &ArtisanTheme, content: impl IntoElement) -> Div {
    let radius = RadiusTokens::value(RadiusStep::Xl);
    div()
        .relative()
        .overflow_hidden()
        .w_full()
        .min_w_0()
        .min_h_0()
        .rounded(radius)
        .backdrop_blur(glass_blur_radius(GlassStrength::Quiet))
        .bg(glass_foreground_base(theme))
        .shadow(glass_card_shadows())
        .child(glass_material_layer(GlassStrength::Quiet, radius))
        .child(glass_highlight_layer(GlassStrength::Quiet, radius))
        .child(content)
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
    /// card width, plus the honest empty state for a scene with no turns yet.
    ///
    /// The host owns the full column so the surface root — the turn
    /// navigator rail's positioning context — spans the card, not the prose
    /// box: the rail anchors right-8 of the card and centers in it. Reading
    /// rhythm stays prose-bound one layer down, inside the surface itself:
    /// each turn root carries the shared max-width, auto margins, and
    /// gutters, so standalone surface fixtures keep the identical column
    /// without this frame. The 40 px top spacing lives inside the scroll
    /// content (owned by the surface); the empty overlay stays centered on
    /// the column itself, not the content.
    fn render_transcript_column(
        &self,
        theme: &ArtisanTheme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let empty = show_empty_transcript(self.host.read(cx).controller_view().turn_views.len());
        let mut column = div()
            .relative()
            .min_h_0()
            .flex_1()
            .overflow_hidden()
            .bg(shell_black())
            .debug_selector(|| THREAD_SCREEN_TRANSCRIPT_SELECTOR.to_owned())
            .child(div().w_full().h_full().child(self.host.clone()));
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
                            .font_weight(ProseTypography::BODY_WEIGHT)
                            .letter_spacing(px(workspace_body_tracking(theme)))
                            .text_color(theme.colors.muted_foreground.to_paint())
                            .child("No messages yet."),
                    ),
            );
        }
        column
    }

    /// Renders one environment-card row: glyph, flexible label, truncating value.
    ///
    /// Legacy frame: `div.flex.min-w-0.items-center.gap-2.rounded-lg.px-2.py-2`
    /// with the `size-4 text-muted-foreground` row glyph, a `flex-1` label,
    /// and a `max-w-36 truncate` value
    /// (`thread-environment-card.svelte:372-376`).
    fn render_environment_row(
        label: &str,
        value: String,
        icon: InspectorRowIcon,
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
            .debug_selector(|| THREAD_SCREEN_ENV_ROW_SELECTOR.to_owned())
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

    /// Renders the environment card (`thread-environment-card.svelte`).
    ///
    /// Legacy frame: `ShaderGlassSurface` at `radius-xl` around the `p-1`
    /// child holding Machine, Changes, Branch, and Worktree rows. The project
    /// selector row belongs to packet 2's project picker and is a gap; the
    /// remote chip is icon-only in legacy with a host-mark brand glyph that
    /// has no exact catalog entry, so it stays a gap rather than a fake chip.
    fn render_environment_card(&self, theme: &ArtisanTheme) -> impl IntoElement {
        let projection = present_thread_environment(&self.environment);
        let mut rows = div()
            .flex()
            .min_w_0()
            .flex_col()
            .text_size(theme.typography.control_text)
            .child(Self::render_environment_row(
                "Machine",
                projection.machine_label,
                InspectorRowIcon::Machine,
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
                theme,
            ));
        }
        if let Some(worktree_label) = projection.current_worktree_label {
            rows = rows.child(Self::render_environment_row(
                "Worktree",
                worktree_label,
                InspectorRowIcon::Worktree,
                theme,
            ));
        }
        div()
            .w_full()
            .min_w_0()
            .debug_selector(|| THREAD_SCREEN_ENV_CARD_SELECTOR.to_owned())
            .child(inspector_glass_card(
                theme,
                div().min_w_0().p(px(CARD_INSET_PX)).child(rows),
            ))
    }

    /// Renders the terminals card (`thread-terminals-card.svelte`).
    ///
    /// Legacy branches: a skeleton shimmer while loading (`flex flex-col
    /// gap-2 p-3` with `h-4` bars at 3/5 and 2/5 widths), the glass card only
    /// when at least one live terminal exists, and nothing otherwise. Liveness
    /// is `opening | active` (`lib/terminal/presentation.ts`
    /// `is_live_terminal`); exited terminals disappear like finished agents.
    /// Rows follow `thread-terminals.svelte`: the `terminal-2` glyph plus
    /// display name plus muted command line. Click-to-inspect and the
    /// tail-viewer dialog need transport wiring and are gaps, so rows render
    /// without a fake affordance.
    fn render_terminals_card(&self, theme: &ArtisanTheme) -> Option<impl IntoElement> {
        if self.terminals_loading {
            let bar = |fraction: f32| {
                div()
                    .h(px(LOADING_BAR_PX))
                    .w(gpui::relative(fraction))
                    .rounded(px(4.0))
                    .bg(theme.colors.muted.to_paint())
            };
            return Some(
                div().w_full().min_w_0().child(inspector_glass_card(
                    theme,
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(ROW_GAP_PX))
                        .p(px(LOADING_PAD_PX))
                        .debug_selector(|| String::from("artisan-thread-screen-terminals-loading"))
                        .child(bar(0.6))
                        .child(bar(0.4)),
                )),
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
        Some(
            div().w_full().min_w_0().child(inspector_glass_card(
                theme,
                div()
                    .min_w_0()
                    .p(px(CARD_INSET_PX))
                    .child(
                        div().flex().min_w_0().flex_col().child(
                            div()
                                .px(px(ROW_PAD_PX))
                                .pt(px(ROW_PAD_PX))
                                .pb(px(4.0))
                                .text_size(theme.typography.control_text)
                                .font_weight(ProseTypography::BODY_WEIGHT)
                                .letter_spacing(px(workspace_body_tracking(theme)))
                                .text_color(theme.colors.foreground.to_paint())
                                .child("Terminals"),
                        ),
                    )
                    .child(list),
            )),
        )
    }

    /// Renders the checklist card (`thread-panel.svelte` plan section).
    ///
    /// Legacy frame: `ShaderGlassSurface` at `radius-xl` around the `p-1`
    /// child holding the `h2.px-2.pt-2.pb-1.text-sm.font-medium` "Checklist"
    /// heading and the `rounded-lg.px-2.py-2.text-sm` rows. `font-medium`
    /// resolves through the redefined token to the workspace 410, so every
    /// row and the heading take it with surface tracking; tone maps the exact
    /// legacy classes to theme colors otherwise: active keeps foreground;
    /// completed/pending/skipped (`text-muted-foreground`, with
    /// `line-through` on completed/skipped) use the muted token with
    /// strikethrough where legacy crosses out. The `list-disc` markers and
    /// the screen-reader `"{state}: "` prefix have no GPUI equivalent on
    /// plain text and stay gaps rather than faked bullets.
    fn render_checklist_card(&self, theme: &ArtisanTheme) -> Option<impl IntoElement> {
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
        Some(
            div().w_full().min_w_0().child(inspector_glass_card(
                theme,
                div().min_w_0().p(px(CARD_INSET_PX)).child(
                    div()
                        .flex()
                        .min_w_0()
                        .flex_col()
                        .child(
                            div()
                                .px(px(ROW_PAD_PX))
                                .pt(px(ROW_PAD_PX))
                                .pb(px(4.0))
                                .text_size(theme.typography.control_text)
                                .font_weight(ProseTypography::BODY_WEIGHT)
                                .letter_spacing(px(workspace_body_tracking(theme)))
                                .text_color(theme.colors.foreground.to_paint())
                                .child("Checklist"),
                        )
                        .child(list),
                ),
            )),
        )
    }

    /// Renders the inspector column (`thread-panel.svelte` root).
    ///
    /// Legacy frame: `div.relative.flex.h-full.min-h-0.flex-col.p-1` around
    /// the `flex.min-h-0.flex-1.flex-col.gap-4` card group. The width is the
    /// live viewport clamp; cards keep their themed fills against the black
    /// column gutters.
    fn render_inspector(&self, theme: &ArtisanTheme, width_px: f32) -> impl IntoElement {
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
            .w(px(width_px))
            .min_h_0()
            .flex()
            .flex_col()
            .p(px(INSPECTOR_PAD_PX))
            .bg(shell_black())
            .debug_selector(|| THREAD_SCREEN_INSPECTOR_SELECTOR.to_owned())
            .child(cards)
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
    fn render_composer_overlay(&self, pad_bottom_px: f32) -> impl IntoElement {
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
            .bg(shell_black())
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
    /// hides, the conversation reclaims its space.
    fn render_open(
        &self,
        theme: &ArtisanTheme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // No native minimum window width is enforced, so the frame inset
        // follows the live viewport instead of assuming desktop.
        let pad_bottom = composer_pad_bottom(f32::from(window.bounds().size.width));
        let conversation = div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.0))
            .min_h(px(0.0))
            .bg(shell_black())
            .child(self.render_transcript_column(theme, cx))
            .child(self.render_composer_overlay(pad_bottom));
        let mut row = div()
            .flex()
            .flex_row()
            .min_h_0()
            .flex_1()
            .bg(shell_black())
            .child(conversation);
        if self.inspector_visible() {
            row = row.child(self.render_inspector(theme, self.inspector_width()));
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

/// Returns whether a terminal session stays visible on the terminals card.
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
