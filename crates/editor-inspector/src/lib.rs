//! The right column, ported from the previous Editor's thread inspector:
//! the Project, Machine, Changes and Branch rows, then Agents and the
//! running Terminals, each section closed by a rule across the column.
//! Reads the open conversation and its host; holds nothing of its own
//! beyond UI state. A terminal opens in full in the [`TerminalDialog`],
//! which the workspace draws above everything.

mod model;
mod terminal_dialog;

pub use terminal_dialog::{TERMINAL_DIALOG_SELECTOR, TerminalDialog};

use artisan_api::{ItemId, Status, StepStatus, ThreadId};
use editor_conversation::{Conversation, ConversationEvent};
use editor_core::{Core, CoreEvent, HostData, HostEvent};
use editor_ui::AssetId;
use editor_ui::gradient::hover_fill_gradient;
use editor_ui::icon::{IconSize, IconStyle, IconTint, icon};
use editor_ui::terminal_text::TerminalScreen;
use editor_ui::theme::{ArtisanTheme, ProseTypography, RadiusStep, RadiusTokens, ThemeMode};
use gpui::{
    Animation, AnimationExt as _, AppContext as _, Context, Div, Entity, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div, px, rgb, rgb_to_hsla,
};
use model::{Model, Sources};
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::time::Duration;

/// `px-2 py-2` on rows: the 8 px inset matches the section labels and the
/// left sidebar's rows, so text in both columns shares one edge.
const ROW_PAD_PX: f32 = 8.0;
const ROW_GAP_PX: f32 = 8.0;
/// Padding of the ruled column, the sidebar's too.
const COLUMN_INSET_PX: f32 = 10.0;
/// A Context row's value never takes more than this.
const VALUE_MAX_WIDTH_PX: f32 = 144.0;
/// The slot holding an agent's state dot, the row glyph's 16 px.
const STATE_SLOT_PX: f32 = 16.0;
const STATE_DOT_PX: f32 = 6.0;
const ADDED_GREEN: u32 = 0x0034_D399;
const REMOVED_RED: u32 = 0x00F8_7171;
const MINUS_SIGN: char = '\u{2212}';
/// One full fade of a working agent's dot, out and back.
const WORKING_PULSE: Duration = Duration::from_millis(1_600);
const WORKING_PULSE_FLOOR: f32 = 0.4;

pub struct Inspector {
    core: Entity<Core>,
    conversation: Entity<Conversation>,
    host: Option<(Entity<HostData>, Subscription)>,
    dialog: Entity<TerminalDialog>,
    _subscriptions: [Subscription; 2],
}

impl Inspector {
    pub fn new(
        core: Entity<Core>,
        conversation: Entity<Conversation>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = [
            cx.subscribe(&conversation, |this, _, _: &ConversationEvent, cx| {
                this.follow_host(cx);
                cx.notify();
            }),
            cx.subscribe(&core, |this, _, _: &CoreEvent, cx| {
                this.follow_host(cx);
                cx.notify();
            }),
        ];
        let dialog = cx.new(|cx| TerminalDialog::new(core.clone(), conversation.clone(), cx));
        let mut inspector = Self {
            core,
            conversation,
            host: None,
            dialog,
            _subscriptions: subscriptions,
        };
        inspector.follow_host(cx);
        inspector
    }

    #[must_use]
    pub fn core(&self) -> &Entity<Core> {
        &self.core
    }

    #[must_use]
    pub fn conversation(&self) -> &Entity<Conversation> {
        &self.conversation
    }

    /// The full-output dialog; the workspace draws it above everything.
    #[must_use]
    pub fn terminal_dialog(&self) -> &Entity<TerminalDialog> {
        &self.dialog
    }

    /// Opens a listed terminal's whole output.
    pub fn open_terminal(&self, id: &ItemId, window: &mut Window, cx: &mut Context<Self>) {
        self.dialog
            .update(cx, |dialog, cx| dialog.open(id, window, cx));
    }

    /// Re-renders on the open host's projects, runs and repositories.
    fn follow_host(&mut self, cx: &mut Context<Self>) {
        let host = self
            .conversation
            .read(cx)
            .open_thread()
            .and_then(|(host, _)| self.core.read(cx).host(host).cloned());
        if self.host.as_ref().map(|(known, _)| known.entity_id())
            == host.as_ref().map(Entity::entity_id)
        {
            return;
        }
        self.host = host.map(|host| {
            let subscription = cx.subscribe(&host, |_, _, event: &HostEvent, cx| match event {
                HostEvent::Changed | HostEvent::Reset => cx.notify(),
                HostEvent::Live { .. } | HostEvent::Item(_) => {}
            });
            (host, subscription)
        });
    }
}

impl Render for Inspector {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
        let conversation = self.conversation.read(cx);
        let open = conversation.open_thread().cloned();
        let host = open
            .as_ref()
            .and_then(|(host, _)| self.core.read(cx).host(host))
            .map(|host| host.read(cx));
        let (Some((_, thread)), Some(host)) = (open.as_ref(), host) else {
            return column();
        };
        let sources = Sources {
            items: conversation.items().collect(),
            run: host.runs.get(thread),
        };
        let model = model::derive(&sources);
        let mut column = column()
            .child(context(host, thread, &model, &theme))
            .child(rule());
        for section in [
            Self::agents(&model, &theme, cx),
            checklist(&model, &theme),
            Self::terminals(&model, &theme, cx),
        ]
        .into_iter()
        .flatten()
        {
            column = column.child(section).child(rule());
        }
        column
    }
}

/// One shell's parsed output. Live output only grows (or restarts), so
/// only the new bytes are fed; anything else starts the screen afresh.
pub(crate) struct ShellScreen {
    fed: usize,
    fed_hash: u64,
    live: bool,
    screen: TerminalScreen,
}

impl ShellScreen {
    fn new() -> Self {
        Self {
            fed: 0,
            fed_hash: hash(""),
            live: false,
            screen: TerminalScreen::new(),
        }
    }

    fn sync(&mut self, text: &str, live: bool) {
        let continues = live == self.live
            && text.len() >= self.fed
            && text.is_char_boundary(self.fed)
            && hash(&text[..self.fed]) == self.fed_hash;
        if !continues {
            *self = Self {
                live,
                ..Self::new()
            };
        }
        if text.len() > self.fed {
            self.screen.feed(&text.as_bytes()[self.fed..]);
            self.fed = text.len();
            self.fed_hash = hash(text);
        }
    }
}

fn hash(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

impl Inspector {
    /// The subagents the thread's current run started, in the order they
    /// started: Artisan's name, the task and its state at the right edge.
    /// One opens its own view in the conversation.
    fn agents(model: &Model<'_>, theme: &ArtisanTheme, cx: &Context<Self>) -> Option<Div> {
        if model.agents.is_empty() {
            return None;
        }
        let mut list = div().flex().flex_col().min_w_0();
        for (index, agent) in model.agents.iter().enumerate() {
            let id = agent.id.clone();
            let hover_theme = *theme;
            let color = match agent.status {
                Status::Running | Status::AwaitingUser => theme.colors.primary,
                Status::Done => theme.colors.unread,
                Status::Failed => theme.colors.destructive,
                Status::Declined | Status::Cancelled => theme.colors.muted_foreground,
            };
            let dot = div()
                .size(px(STATE_DOT_PX))
                .rounded_full()
                .bg(color.to_paint());
            let dot = if agent.status == Status::Running {
                // Live work breathes like the sidebar's working dot.
                dot.with_animation(
                    ("inspector-agent", index),
                    Animation::new(WORKING_PULSE).repeat(),
                    |dot, progress| {
                        let swing = (progress * std::f32::consts::TAU).cos().mul_add(0.5, 0.5);
                        dot.opacity(WORKING_PULSE_FLOOR + (1.0 - WORKING_PULSE_FLOOR) * swing)
                    },
                )
                .into_any_element()
            } else {
                dot.into_any_element()
            };
            list = list.child(
                row()
                    .id(SharedString::from(format!("inspector-agent-{}", agent.id)))
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover_fill_gradient(hover_theme)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let id = id.clone();
                        this.conversation
                            .update(cx, |conversation, cx| conversation.open_agent(Some(id), cx));
                    }))
                    .child(glyph(theme, AssetId::TABLER_BOT_ID))
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_size(theme.typography.control_text)
                            .font_weight(ProseTypography::BODY_WEIGHT)
                            .text_color(theme.colors.foreground.to_paint())
                            .child(agent.name.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(theme.typography.control_text)
                            .text_color(theme.colors.muted_foreground.to_paint())
                            .child(agent.task.lines().next().unwrap_or_default().to_owned()),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .size(px(STATE_SLOT_PX))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(dot),
                    ),
            );
        }
        Some(section(theme, Some("Agents"), list))
    }

    /// The thread's shells still running: the command's first line in
    /// mono. Being listed means it runs, so no state mark; a click opens
    /// its output.
    fn terminals(model: &Model<'_>, theme: &ArtisanTheme, cx: &Context<Self>) -> Option<Div> {
        if model.terminals.is_empty() {
            return None;
        }
        let mut list = div().flex().min_w_0().flex_col();
        for terminal in &model.terminals {
            let command = terminal
                .command
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .unwrap_or("Shell");
            let id = terminal.id.clone();
            let hover_theme = *theme;
            list = list.child(
                row()
                    .id(SharedString::from(format!(
                        "inspector-terminal-{}",
                        terminal.id
                    )))
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover_fill_gradient(hover_theme)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_terminal(&id, window, cx);
                    }))
                    .child(glyph(theme, AssetId::TABLER_TERMINAL_2))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family(theme.typography.mono.family)
                            .text_size(theme.typography.label_text)
                            // Mono keeps normal tracking.
                            .letter_spacing(px(0.0))
                            .text_color(theme.colors.foreground.to_paint())
                            .child(command.to_owned()),
                    ),
            );
        }
        Some(section(theme, Some("Terminals"), list))
    }
}

/// The unlabelled Context section: each row names its own fact.
fn context(host: &HostData, thread: &ThreadId, model: &Model<'_>, theme: &ArtisanTheme) -> Div {
    let project = host
        .threads
        .get(thread)
        .and_then(|thread| host.projects.get(&thread.project_id));
    let mut rows = div().flex().min_w_0().flex_col();
    if let Some(project) = project {
        rows = rows.child(fact(
            theme,
            AssetId::TABLER_FOLDER,
            "Project",
            project.name.clone(),
        ));
    }
    rows = rows.child(fact(
        theme,
        AssetId::TABLER_DEVICE_LAPTOP,
        "Machine",
        host.name.clone(),
    ));
    if let Some((added, removed)) = model.change_summary() {
        let stat = |text: String, color: u32| {
            div()
                .flex_shrink_0()
                .text_size(theme.typography.control_text)
                .letter_spacing(px(0.0))
                .text_color(rgb_to_hsla(rgb(color)))
                .child(text)
        };
        rows = rows.child(
            row()
                .child(glyph(theme, AssetId::TABLER_FILE_DIFF))
                .child(label(theme, "Changes"))
                .child(stat(format!("+{added}"), ADDED_GREEN))
                .child(stat(format!("{MINUS_SIGN}{removed}"), REMOVED_RED)),
        );
    }
    if let Some(branch) = project
        .and_then(|project| host.repository(&project.id))
        .and_then(|repository| repository.branch.clone())
    {
        rows = rows.child(fact(theme, AssetId::TABLER_GIT_BRANCH, "Branch", branch));
    }
    // A linked worktree, named by its folder, as the previous Editor's
    // Worktree row.
    if let Some(worktree) = project
        .and_then(|project| host.repository(&project.id))
        .and_then(|repository| repository.worktree.clone())
    {
        rows = rows.child(fact(
            theme,
            AssetId::TABLER_FOLDER_CODE,
            "Worktree",
            worktree,
        ));
    }
    section(theme, None, rows)
}

/// The latest plan's steps (`render_checklist_section`): the step being
/// worked on in the foreground, the rest muted, done and cancelled ones
/// struck through.
fn checklist(model: &Model<'_>, theme: &ArtisanTheme) -> Option<Div> {
    if model.checklist.is_empty() {
        return None;
    }
    let mut list = div().flex().min_w_0().flex_col();
    for (index, step) in model.checklist.iter().enumerate() {
        let row = div()
            .rounded(RadiusTokens::value(RadiusStep::Lg))
            .px(px(ROW_PAD_PX))
            .py(px(ROW_PAD_PX))
            .text_size(theme.typography.control_text)
            .font_weight(ProseTypography::BODY_WEIGHT)
            .tracking(ProseTypography::TRACKING_EM)
            .debug_selector(move || format!("artisan-thread-screen-checklist-entry-{index}"));
        let row = match step.status {
            StepStatus::InProgress => row.text_color(theme.colors.foreground.to_paint()),
            StepStatus::Done => row
                .text_color(theme.colors.muted_foreground.to_paint())
                .line_through(),
            StepStatus::Pending => row.text_color(theme.colors.muted_foreground.to_paint()),
            StepStatus::Cancelled => row
                .text_color(theme.colors.muted_foreground.with_alpha(0.7).to_paint())
                .line_through(),
        };
        list = list.child(row.child(step.text.clone()));
    }
    Some(section(theme, Some("Checklist"), list))
}

/// A read-only fact: glyph, label, and the value at the right edge.
fn fact(theme: &ArtisanTheme, icon: AssetId, name: &'static str, value: String) -> Div {
    row()
        .child(glyph(theme, icon))
        .child(label(theme, name))
        .child(
            div()
                .flex_shrink_0()
                .max_w(px(VALUE_MAX_WIDTH_PX))
                .truncate()
                .text_size(theme.typography.control_text)
                .font_weight(ProseTypography::BODY_WEIGHT)
                .text_color(theme.colors.foreground.to_paint())
                .child(value),
        )
}

fn label(theme: &ArtisanTheme, text: &'static str) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .text_size(theme.typography.control_text)
        .font_weight(ProseTypography::BODY_WEIGHT)
        .text_color(theme.colors.foreground.to_paint())
        .child(text)
}

/// The ruled column: flat on the shell's black, padded like the sidebar.
fn column() -> Div {
    div()
        .size_full()
        .min_w_0()
        .min_h_0()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .p(px(COLUMN_INSET_PX))
        .tracking(ProseTypography::TRACKING_EM)
}

/// The rule closing a section, edge to edge across the column and the
/// column inset from the rows on both sides.
fn rule() -> Div {
    let breathing = px(COLUMN_INSET_PX - 12.0);
    div()
        .flex_none()
        .h(px(1.0))
        .mx(px(-COLUMN_INSET_PX))
        .my(breathing)
        .bg(editor_ui::theme::DesktopTheme::neutral_dark().line)
}

/// A section's rows under the muted label the sidebar's age groups share.
fn section(theme: &ArtisanTheme, label: Option<&'static str>, rows: Div) -> Div {
    div()
        .flex()
        .flex_col()
        .w_full()
        .min_w_0()
        .children(label.map(|label| {
            div()
                .w_full()
                .px(px(ROW_PAD_PX))
                .pb(theme.spacing.steps(1.0))
                .text_size(theme.typography.label_text)
                .text_color(theme.colors.muted_foreground.to_paint())
                .truncate()
                .child(label)
        }))
        .child(rows)
}

fn row() -> Div {
    div()
        .flex()
        .w_full()
        .min_w_0()
        .items_center()
        .gap(px(ROW_GAP_PX))
        .rounded(RadiusTokens::value(RadiusStep::Lg))
        .px(px(ROW_PAD_PX))
        .py(px(ROW_PAD_PX))
}

fn glyph(theme: &ArtisanTheme, id: AssetId) -> impl IntoElement {
    icon(IconStyle::resolve(
        *theme,
        id,
        IconSize::Default,
        IconTint::Muted,
    ))
    .flex_shrink_0()
}

#[cfg(test)]
mod tests {
    use super::ShellScreen;

    /// Live output arrives in pieces that can split an escape sequence; once
    /// the shell ends, the stored output replaces what streamed.
    #[test]
    fn live_output_parses_whole_and_the_stored_output_takes_over() {
        let mut shell = ShellScreen::new();
        shell.sync("\x1b[3", true);
        shell.sync("\x1b[31mred\x1b[0m\nnext", true);
        assert_eq!(shell.screen.plain_text(), "red\nnext");
        assert!(!shell.screen.line(0).expect("first line").runs.is_empty());

        shell.sync("all done\n", false);
        assert_eq!(shell.screen.plain_text(), "all done");
    }
}
