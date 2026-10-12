//! The whole output of one shell job, in a dialog above the workspace.
//!
//! A glass card that reads as the terminal the job ran in: the command sits
//! after the shell's prompt, its output follows with its colours, and the
//! last line is the cursor while the job runs or a fresh prompt once it
//! stopped. The card scrolls through everything kept, opens at the bottom,
//! follows new output while the reader is at the bottom and stays put once
//! they scroll up; scrolling back down follows again. Escape or a click
//! outside closes it.
//!
//! The dialog is its own entity, fed from the same sources as the inspector
//! row (the host's live shell text, then the stored item output) and parsed
//! once into its own screen. New output notifies only this view; the
//! workspace draws it cached, so nothing else re-renders for it.

use crate::ShellScreen;
use crate::model::shell_output;
use artisan_api::{ItemId, ItemKind, LiveTarget, Status, ThreadId, ToolKind};
use editor_conversation::{Conversation, ConversationEvent};
use editor_core::{Core, HostEvent};
use editor_ui::AssetId;
use editor_ui::button::{AccessibleLabel, Button, ButtonContent, ButtonSize, ButtonVariant};
use editor_ui::copy_feedback::COPY_FEEDBACK_WINDOW;
use editor_ui::dialog::DialogAnimation;
use editor_ui::glass::{
    GlassStrength, glass_blur_radius, glass_card_shadows, glass_foreground_base,
    glass_highlight_layer, glass_material_layer,
};
use editor_ui::motion::MotionPolicy;
use editor_ui::terminal_text::{TerminalPalette, highlights};
use editor_ui::theme::{ArtisanTheme, RadiusStep, RadiusTokens, ThemeMode};
use gpui::{
    AnimationExt as _, AnyElement, ClipboardItem, Context, FocusHandle, FollowMode, FontWeight,
    HighlightStyle, InteractiveElement as _, IntoElement, KeyDownEvent, ListAlignment, ListState,
    MouseButton, MouseDownEvent, ParentElement as _, Render, SharedString, Styled as _, StyledText,
    Subscription, Task, Window, div, list, px,
};
use std::ops::Range;

/// Debug selector of the terminal card.
pub const TERMINAL_DIALOG_SELECTOR: &str = "inspector-terminal-dialog";

/// Widest the terminal grows.
const DIALOG_MAX_WIDTH_PX: f32 = 1040.0;
/// Room the terminal leaves around itself inside the window.
const DIALOG_WINDOW_MARGIN_PX: f32 = 48.0;
/// Tallest the terminal grows, and its share of the window height.
const OUTPUT_MAX_HEIGHT_PX: f32 = 640.0;
const OUTPUT_HEIGHT_SHARE: f32 = 0.62;
const OUTPUT_TEXT_PX: f32 = 12.5;
const OUTPUT_LINE_HEIGHT_PX: f32 = 18.0;
/// The block cursor of a running job.
const CURSOR_WIDTH_PX: f32 = 7.5;
const CURSOR_HEIGHT_PX: f32 = 15.0;
/// Dims the window behind the terminal, lightly enough for the glass to
/// show what it covers.
const BACKDROP_OPACITY: f32 = 0.45;
/// Hover group of the card, which reveals its copy control.
const CARD_GROUP: &str = "inspector-terminal-card";
/// Trailing lines re-measured after new output: a spinner or progress bar
/// rewrites the last few lines in place.
const REMEASURED_TAIL_LINES: usize = 16;

/// The prompt of the shell a job ran in, drawn before its command.
///
/// Engines run commands through the Forge machine's shell, so the prompt
/// follows the machine: PowerShell on Windows, a POSIX shell elsewhere. The
/// job's directory (or its project's root) tells which, since Forge reports
/// paths in its own form.
#[derive(Clone, Debug, Eq, PartialEq)]
struct ShellPrompt {
    /// What the shell prints before a command, e.g. `~/projects/app$ ` or
    /// `PS C:\Users\me\app> `.
    prompt: String,
    /// What the shell prints before each continuation line of a command.
    continuation: &'static str,
    /// Whether the prompt is coloured, as default POSIX prompts are.
    coloured: bool,
}

impl ShellPrompt {
    fn for_directory(directory: Option<&str>) -> Self {
        match directory.map(str::trim).filter(|path| !path.is_empty()) {
            Some(path) if is_windows_path(path) => Self {
                prompt: format!("PS {}> ", path.trim_end_matches(['\\', '/'])),
                continuation: ">> ",
                coloured: false,
            },
            Some(path) => Self {
                prompt: format!("{}$ ", home_relative(path)),
                continuation: "> ",
                coloured: true,
            },
            None => Self {
                prompt: "$ ".to_owned(),
                continuation: "> ",
                coloured: true,
            },
        }
    }
}

/// `C:\...`, `C:/...`, or a `\\server\share` path.
fn is_windows_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with("\\\\")
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
}

/// `path` with its home directory as `~`, as POSIX prompts show it.
fn home_relative(path: &str) -> String {
    let path = if path.len() > 1 {
        path.trim_end_matches('/')
    } else {
        path
    };
    let home_end = if path == "/root" || path.starts_with("/root/") {
        Some("/root".len())
    } else {
        ["/home/", "/Users/"].iter().find_map(|prefix| {
            let user = path.strip_prefix(prefix)?;
            let user_len = user.find('/').unwrap_or(user.len());
            (user_len > 0).then_some(prefix.len() + user_len)
        })
    };
    match home_end {
        Some(end) => format!("~{}", &path[end..]),
        None => path.to_owned(),
    }
}

/// Where the job is in its life, as the last rows show it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum JobState {
    /// The cursor waits after the output.
    Running,
    /// Cancelled: `^C`, then the prompt the shell returns to.
    Killed,
    /// Finished: the next prompt.
    Stopped,
}

impl JobState {
    fn of(status: Status) -> Self {
        match status {
            Status::Running | Status::AwaitingUser => Self::Running,
            Status::Cancelled => Self::Killed,
            Status::Done | Status::Failed | Status::Declined => Self::Stopped,
        }
    }
}

/// The job on screen.
struct Job {
    thread: ThreadId,
    item: ItemId,
    command: String,
    state: JobState,
    prompt: ShellPrompt,
    shell: ShellScreen,
}

impl Job {
    /// Whether output before the screen's first line is gone: cleared by the
    /// job itself or dropped by the screen's bound.
    fn missing_earlier_output(&self) -> bool {
        self.shell.screen.cleared_earlier() || self.shell.screen.truncated_line_count() > 0
    }
}

/// How the open job's rows lay out in the list: the command after its
/// prompt, a note when earlier output is missing, the screen's lines, then
/// the cursor or the next prompt.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Layout {
    /// The command's lines.
    commands: usize,
    /// Rows before the output: the command's lines, then the note.
    lead: usize,
    /// The screen's lines.
    lines: usize,
    /// Rows after the output.
    trail: usize,
    truncated: u64,
    cleared: u64,
}

impl Layout {
    fn of(job: &Job) -> Self {
        let screen = &job.shell.screen;
        let commands = command_lines(&job.command).len();
        Self {
            commands,
            lead: commands + usize::from(job.missing_earlier_output()),
            lines: screen.line_count(),
            trail: if job.state == JobState::Killed { 2 } else { 1 },
            truncated: screen.truncated_line_count(),
            cleared: screen.cleared_line_count(),
        }
    }

    fn total(self) -> usize {
        self.lead + self.lines + self.trail
    }

    fn output(self) -> Range<usize> {
        self.lead..self.lead + self.lines
    }
}

/// The lines a command is typed as; a job without a command shows a bare
/// prompt.
fn command_lines(command: &str) -> Vec<&str> {
    let lines: Vec<&str> = command.trim_end().lines().collect();
    if lines.is_empty() { vec![""] } else { lines }
}

/// One row of the open job.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Row {
    /// Line `n` of the command; the first follows the prompt.
    Command(usize),
    /// Earlier output is missing.
    Missing,
    /// Screen line `n`.
    Output(usize),
    Cursor,
    Interrupt,
    /// The prompt the shell returned to.
    Prompt,
}

impl Row {
    fn at(state: JobState, layout: Layout, index: usize) -> Option<Self> {
        if index < layout.commands {
            return Some(Self::Command(index));
        }
        if index < layout.lead {
            return Some(Self::Missing);
        }
        if layout.output().contains(&index) {
            return Some(Self::Output(index - layout.lead));
        }
        match (index - layout.output().end, state) {
            (0, JobState::Running) => Some(Self::Cursor),
            (0, JobState::Killed) => Some(Self::Interrupt),
            (1, JobState::Killed) | (0, JobState::Stopped) => Some(Self::Prompt),
            _ => None,
        }
    }
}

pub struct TerminalDialog {
    core: gpui::Entity<Core>,
    conversation: gpui::Entity<Conversation>,
    job: Option<Job>,
    list: ListState,
    /// What the list was last told about the open job.
    shown: Layout,
    following: bool,
    copied: Option<Task<()>>,
    focus: FocusHandle,
    copy_focus: FocusHandle,
    latest_focus: FocusHandle,
    /// Where focus goes back to when the dialog closes.
    restore: Option<FocusHandle>,
    host_events: Option<Subscription>,
    _conversation_events: Subscription,
}

impl TerminalDialog {
    pub fn new(
        core: gpui::Entity<Core>,
        conversation: gpui::Entity<Conversation>,
        cx: &mut Context<Self>,
    ) -> Self {
        let list = ListState::new(0, ListAlignment::Top, px(OUTPUT_LINE_HEIGHT_PX * 20.0));
        let view = cx.entity().downgrade();
        list.set_scroll_handler(move |_, _, cx| {
            let _ = view.update(cx, Self::note_scroll);
        });
        let conversation_events =
            cx.subscribe(&conversation, |this, _, _: &ConversationEvent, cx| {
                this.sync(cx);
            });
        Self {
            core,
            conversation,
            job: None,
            list,
            shown: Layout::default(),
            following: true,
            copied: None,
            focus: cx.focus_handle(),
            copy_focus: cx.focus_handle(),
            latest_focus: cx.focus_handle(),
            restore: None,
            host_events: None,
            _conversation_events: conversation_events,
        }
    }

    /// Opens the dialog on shell `item` of the open thread, scrolled to the
    /// newest output.
    pub fn open(&mut self, item: &ItemId, window: &mut Window, cx: &mut Context<Self>) {
        let Some((host_id, thread)) = self.conversation.read(cx).open_thread().cloned() else {
            return;
        };
        let Some(host) = self.core.read(cx).host(&host_id).cloned() else {
            return;
        };
        let watched = item.clone();
        self.host_events =
            Some(
                cx.subscribe(&host, move |this, _, event: &HostEvent, cx| match event {
                    HostEvent::Live {
                        target: LiveTarget::Shell { item_id },
                        ..
                    } if *item_id == watched => this.sync(cx),
                    HostEvent::Changed | HostEvent::Reset => this.sync(cx),
                    HostEvent::Live { .. } | HostEvent::Item(_) => {}
                }),
            );
        self.job = Some(Job {
            thread,
            item: item.clone(),
            command: String::new(),
            state: JobState::Running,
            prompt: ShellPrompt::for_directory(None),
            shell: ShellScreen::new(),
        });
        self.shown = Layout::default();
        self.copied = None;
        self.list.reset(0);
        self.sync(cx);
        if self.job.is_none() {
            return;
        }
        self.list.set_follow_mode(FollowMode::Tail);
        self.following = true;
        self.restore = window.focused(cx);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss(cx);
        if let Some(restore) = self.restore.take() {
            window.focus(&restore, cx);
        }
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.host_events = None;
        if self.job.take().is_some() {
            cx.notify();
        }
    }

    #[must_use]
    pub fn is_open(&self) -> bool {
        self.job.is_some()
    }

    /// Re-reads the open job from the conversation and the host. Closes
    /// when the job is gone (another thread opened, or it left the window).
    fn sync(&mut self, cx: &mut Context<Self>) {
        let Some(job) = self.job.as_mut() else {
            return;
        };
        let conversation = self.conversation.read(cx);
        let open = conversation.open_thread();
        let shell = open
            .filter(|(_, thread)| *thread == job.thread)
            .and_then(|_| conversation.items().find(|item| item.id == job.item))
            .and_then(|item| match &item.kind {
                ItemKind::Tool {
                    tool: ToolKind::Shell { command, cwd, .. },
                    output,
                    ..
                } => Some((item.status, command, cwd, output)),
                _ => None,
            });
        let host = open.and_then(|(host, _)| self.core.read(cx).host(host));
        let (Some((status, command, cwd, output)), Some(host)) = (shell, host) else {
            self.dismiss(cx);
            return;
        };
        let host = host.read(cx);
        let streamed = (!status.is_terminal())
            .then(|| {
                host.live(
                    &job.thread,
                    &LiveTarget::Shell {
                        item_id: job.item.clone(),
                    },
                )
            })
            .flatten();
        // The stream saw everything; the stored output keeps only its head
        // and tail. A job watched while it ran keeps what streamed.
        let keep_stream = streamed.is_none() && job.shell.live && job.shell.fed > 0;
        let before = (job.shell.fed, job.shell.fed_hash, job.state);
        if !keep_stream {
            job.shell
                .sync(&shell_output(streamed, output.as_ref()), streamed.is_some());
        }
        job.state = JobState::of(status);
        job.command.clone_from(command);
        let directory = cwd.as_deref().or_else(|| {
            host.threads
                .get(&job.thread)
                .and_then(|thread| host.projects.get(&thread.project_id))
                .map(|project| project.path.as_str())
        });
        let prompt = ShellPrompt::for_directory(directory);
        let changed =
            before != (job.shell.fed, job.shell.fed_hash, job.state) || prompt != job.prompt;
        job.prompt = prompt;
        if changed || Layout::of(job) != self.shown {
            self.sync_list();
            cx.notify();
        }
    }

    /// Tells the list what changed on the open job: lines dropped from the
    /// front of its output, new lines, the last few lines, which output may
    /// have rewritten in place, and the rows after them, which change when
    /// the job stops.
    fn sync_list(&mut self) {
        let Some(next) = self.job.as_ref().map(Layout::of) else {
            return;
        };
        let shown = self.shown;
        self.shown = next;
        let dropped =
            usize::try_from(next.truncated.saturating_sub(shown.truncated)).unwrap_or(usize::MAX);
        if next.lead != shown.lead
            || next.cleared != shown.cleared
            || next.truncated < shown.truncated
            || dropped >= shown.lines
        {
            self.list.reset(next.total());
            return;
        }
        let kept = shown.lines - dropped;
        let from = kept.saturating_sub(REMEASURED_TAIL_LINES);
        if next.lines < from {
            self.list.reset(next.total());
            return;
        }
        let lead = next.lead;
        self.list
            .splice(lead + shown.lines..lead + shown.lines + shown.trail, 0);
        if dropped > 0 {
            self.list.splice(lead..lead + dropped, 0);
        }
        self.list
            .splice(lead + from..lead + kept, next.lines - from + next.trail);
    }

    fn note_scroll(&mut self, cx: &mut Context<Self>) {
        let following = self.list.is_following_tail();
        if following != self.following {
            self.following = following;
            cx.notify();
        }
    }

    fn jump_to_latest(&mut self, cx: &mut Context<Self>) {
        self.list.set_follow_mode(FollowMode::Tail);
        self.following = true;
        cx.notify();
    }

    fn copy_output(&mut self, cx: &mut Context<Self>) {
        let Some(job) = self.job.as_ref() else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(job.shell.screen.plain_text()));
        self.copied = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPY_FEEDBACK_WINDOW).await;
            let _ = this.update(cx, |this, cx| {
                this.copied = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn render_row(&self, index: usize, theme: &ArtisanTheme) -> AnyElement {
        let row = div()
            .w_full()
            .min_h(px(OUTPUT_LINE_HEIGHT_PX))
            .px(px(16.0))
            .flex()
            .items_center();
        let Some(job) = self.job.as_ref() else {
            return row.into_any_element();
        };
        let Some(kind) = Row::at(job.state, self.shown, index) else {
            return row.into_any_element();
        };
        let palette = TerminalPalette::for_theme(theme);
        let prompt_style = HighlightStyle {
            color: Some(palette.base[12]),
            font_weight: Some(FontWeight::BOLD),
            ..HighlightStyle::default()
        };
        let typed = |prefix: &str, text: &str, coloured: bool| {
            let line = SharedString::from(format!("{prefix}{text}"));
            let runs = if coloured && !prefix.is_empty() {
                // The `$ ` stays plain, as in a default bash prompt.
                let tinted = prefix.trim_end().trim_end_matches('$').len();
                vec![(0..tinted, prompt_style)]
            } else {
                Vec::new()
            };
            StyledText::new(line).with_highlights(runs)
        };
        let prompt = &job.prompt;
        let child = match kind {
            Row::Command(line) => {
                let text = command_lines(&job.command)
                    .get(line)
                    .copied()
                    .unwrap_or_default();
                if line == 0 {
                    typed(&prompt.prompt, text, prompt.coloured).into_any_element()
                } else {
                    typed(prompt.continuation, text, false).into_any_element()
                }
            }
            Row::Missing => div()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child("Earlier output is not kept; showing the newest.")
                .into_any_element(),
            Row::Output(line) => match job.shell.screen.line(line) {
                Some(line) => {
                    let foreground = theme.colors.foreground.to_paint();
                    let runs = highlights(line, &palette, foreground, gpui::transparent_black());
                    StyledText::new(SharedString::from(line.text.clone()))
                        .with_highlights(runs)
                        .into_any_element()
                }
                None => div().into_any_element(),
            },
            Row::Cursor => div()
                .w(px(CURSOR_WIDTH_PX))
                .h(px(CURSOR_HEIGHT_PX))
                .bg(theme.colors.foreground.with_alpha(0.8).to_paint())
                .into_any_element(),
            Row::Interrupt => div().child("^C").into_any_element(),
            Row::Prompt => typed(&prompt.prompt, "", prompt.coloured).into_any_element(),
        };
        row.child(child).into_any_element()
    }

    fn render_dialog(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
        let viewport = window.viewport_size();
        let width = (f32::from(viewport.width) - 2.0 * DIALOG_WINDOW_MARGIN_PX)
            .clamp(320.0, DIALOG_MAX_WIDTH_PX);
        let height =
            (f32::from(viewport.height) * OUTPUT_HEIGHT_SHARE).clamp(160.0, OUTPUT_MAX_HEIGHT_PX);
        let radius = RadiusTokens::value(RadiusStep::X2l);
        let output = list(
            self.list.clone(),
            cx.processor(move |this, index, _window, _cx| this.render_row(index, &theme)),
        )
        .w_full()
        .h_full();
        let card = div()
            .id("inspector-terminal-card")
            .group(CARD_GROUP)
            .relative()
            .w(px(width))
            .h(px(height))
            .rounded(radius)
            .backdrop_blur(glass_blur_radius(GlassStrength::Strong))
            .bg(glass_foreground_base(&theme))
            .border_1()
            .border_color(theme.colors.border.to_paint())
            .shadow(glass_card_shadows())
            .overflow_hidden()
            .occlude()
            .debug_selector(|| TERMINAL_DIALOG_SELECTOR.to_owned())
            .child(glass_material_layer(GlassStrength::Strong, radius))
            .child(glass_highlight_layer(GlassStrength::Strong, radius))
            .child(
                div()
                    .relative()
                    .size_full()
                    .py(px(14.0))
                    .font_family(theme.typography.mono.family)
                    .text_size(px(OUTPUT_TEXT_PX))
                    .line_height(px(OUTPUT_LINE_HEIGHT_PX))
                    .text_color(theme.colors.foreground.to_paint())
                    .child(output),
            )
            .children(self.controls(&theme, cx));
        let backdrop = div()
            .absolute()
            .inset_0()
            .bg(gpui::hsla(0.0, 0.0, 0.0, BACKDROP_OPACITY))
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.close(window, cx);
                    cx.stop_propagation();
                }),
            );
        // The root of a cached view is laid out on its own, at the view's
        // bounds: it fills them rather than positioning against a parent.
        let overlay = div()
            .relative()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key.eq_ignore_ascii_case("escape")
                    && !event.keystroke.modifiers.modified()
                {
                    window.prevent_default();
                    this.close(window, cx);
                    cx.stop_propagation();
                }
            }))
            .child(backdrop)
            .child(card);
        if cx.reduce_motion() {
            overlay.into_any_element()
        } else {
            overlay
                .with_animation(
                    "inspector-terminal-open",
                    DialogAnimation.gpui_clock(),
                    gpui::Styled::opacity,
                )
                .into_any_element()
        }
    }

    /// The card's floating controls: copy, revealed while the pointer is
    /// over the card, and a jump back to the newest output once the reader
    /// scrolled up.
    fn controls(&self, theme: &ArtisanTheme, cx: &Context<Self>) -> Vec<AnyElement> {
        let icon_button = |id: &'static str, focus: &FocusHandle, icon, label: &'static str| {
            let label = AccessibleLabel::new(label).ok()?;
            Button::new(
                id,
                focus.clone(),
                *theme,
                MotionPolicy::Full,
                ButtonVariant::Ghost,
                ButtonSize::IconSmall,
                ButtonContent::icon_only(icon, label),
            )
            .ok()
        };
        let copy_view = cx.entity().downgrade();
        let (copy_icon, copy_label) = if self.copied.is_some() {
            (AssetId::TABLER_CHECK, "Copied")
        } else {
            (AssetId::TABLER_COPY, "Copy output")
        };
        let copy = icon_button(
            "inspector-terminal-copy",
            &self.copy_focus,
            copy_icon,
            copy_label,
        )
        .map(|button| {
            let button = button.on_activate(move |_, _, cx| {
                let _ = copy_view.update(cx, Self::copy_output);
            });
            div()
                .absolute()
                .top(px(8.0))
                .right(px(8.0))
                .opacity(0.0)
                .group_hover(CARD_GROUP, |hover| hover.opacity(1.0))
                .child(button)
                .into_any_element()
        });
        let latest_view = cx.entity().downgrade();
        let latest = (!self.following)
            .then(|| {
                icon_button(
                    "inspector-terminal-latest",
                    &self.latest_focus,
                    AssetId::TABLER_ARROW_BAR_TO_DOWN,
                    "Jump to latest",
                )
            })
            .flatten()
            .map(|button| {
                let button = button.on_activate(move |_, _, cx| {
                    let _ = latest_view.update(cx, Self::jump_to_latest);
                });
                div()
                    .absolute()
                    .bottom(px(8.0))
                    .right(px(8.0))
                    .child(button)
                    .into_any_element()
            });
        copy.into_iter().chain(latest).collect()
    }
}

impl Render for TerminalDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.job.is_some() {
            self.render_dialog(window, cx)
        } else {
            div().into_any_element()
        }
    }
}

#[cfg(test)]
mod tests;
