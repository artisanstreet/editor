//! The open thread's shell jobs: the Terminals inspector rows and the
//! output dialog behind them.
//!
//! The Forge pushes every shell job the thread's engine runs opened, each
//! with the raw output it produced since the previous push
//! ([`artisan_domain::ThreadTerminals`]). [`ThreadTerminalsMirror`] keeps
//! one [`TerminalScreen`] per job and feeds it only the bytes it does not
//! have yet, so escape sequences split across pushes parse whole and the
//! screen never re-parses old output. A push that skips bytes (the Forge
//! dropped older output) starts the screen afresh and says so.
//!
//! [`ThreadTerminalsView`] owns the mirror for the selected thread and draws
//! the dialog: a glass card that reads as the terminal the job ran in. The
//! command sits after the shell's prompt, its output follows with its
//! colours, and the last line is the cursor while the job runs or a fresh
//! prompt once it stopped. The card scrolls through everything kept, opens
//! at the bottom, follows new output while the reader is at the bottom and
//! stays put once they scroll up; scrolling back down follows again. New
//! output re-renders only this view: the inspector changes only when a job
//! starts, stops, or leaves.

#![forbid(unsafe_code)]

use std::ops::Range;

use artisan_assets::AssetId;
use artisan_domain::{ThreadId, ThreadTerminal, ThreadTerminalState, ThreadTerminals};
use artisan_ui::button::{AccessibleLabel, Button, ButtonContent, ButtonSize, ButtonVariant};
use artisan_ui::dialog::DialogAnimation;
use artisan_ui::glass::{
    GlassStrength, glass_blur_radius, glass_card_shadows, glass_foreground_base,
    glass_highlight_layer, glass_material_layer,
};
use artisan_ui::motion::MotionPolicy;
use artisan_ui::terminal_text::{TerminalPalette, TerminalScreen, highlights};
use artisan_ui::theme::{ArtisanTheme, RadiusStep, RadiusTokens, ThemeMode};
use gpui::prelude::{InteractiveElement as _, ParentElement as _, Styled as _};
use gpui::{
    AnimationExt as _, AnyElement, ClipboardItem, ColorExt as _, Context, FocusHandle, FollowMode,
    FontWeight, HighlightStyle, IntoElement, KeyDownEvent, ListAlignment, ListState, MouseButton,
    MouseDownEvent, Render, SharedString, StyledText, Window, deferred, div, list, px,
};

/// Stable debug selector for the dialog's overlay.
pub const THREAD_TERMINAL_DIALOG_SELECTOR: &str = "artisan-thread-terminal-dialog";

/// Stable debug selector for the terminal card.
pub const THREAD_TERMINAL_OUTPUT_SELECTOR: &str = "artisan-thread-terminal-output";

/// Above the shell's deferred menus, below the image preview.
const DIALOG_DEFERRED_PRIORITY: usize = 90;

/// Widest the terminal grows.
const DIALOG_MAX_WIDTH_PX: f32 = 1040.0;

/// Room the terminal leaves around itself inside the window.
const DIALOG_WINDOW_MARGIN_PX: f32 = 48.0;

/// Tallest the terminal grows.
const OUTPUT_MAX_HEIGHT_PX: f32 = 640.0;

/// Share of the window height the terminal may take.
const OUTPUT_HEIGHT_SHARE: f32 = 0.62;

/// Output text size and line height.
const OUTPUT_TEXT_PX: f32 = 12.5;
const OUTPUT_LINE_HEIGHT_PX: f32 = 18.0;

/// The block cursor of a running job.
const CURSOR_WIDTH_PX: f32 = 7.5;
const CURSOR_HEIGHT_PX: f32 = 15.0;

/// Dims the window behind the terminal, lightly enough for the glass to
/// show what it covers.
const BACKDROP_OPACITY: f32 = 0.45;

/// Hover group of the terminal card, which reveals its copy control.
const CARD_GROUP: &str = "artisan-thread-terminal-card";

/// Trailing lines re-measured after new output: a spinner or progress bar
/// rewrites the last few lines in place.
const REMEASURED_TAIL_LINES: usize = 16;

/// Label for a job whose engine did not report its command.
pub const UNNAMED_TERMINAL_LABEL: &str = "Shell";

/// One inspector row: a shell job of the open thread.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadTerminalEntry {
    /// The job, as the Forge names it.
    pub terminal_id: String,
    /// The command's first line, or [`UNNAMED_TERMINAL_LABEL`].
    pub name: String,
    /// Where the job is in its life.
    pub state: ThreadTerminalState,
    /// Whether the job runs in the background.
    pub background: bool,
}

/// One mirrored shell job.
#[derive(Debug)]
pub struct MirroredTerminal {
    terminal: ThreadTerminal,
    /// Offset just past the last output byte fed to `screen`.
    next: u64,
    /// Whether output before the screen's first byte was dropped.
    dropped: bool,
    screen: TerminalScreen,
}

impl MirroredTerminal {
    fn new(terminal: ThreadTerminal) -> Self {
        let mut mirrored = Self {
            next: terminal.output.start,
            dropped: terminal.output.start > 0,
            screen: TerminalScreen::new(),
            terminal,
        };
        mirrored.feed_pushed();
        mirrored
    }

    /// The job, without output: output lives in the screen.
    #[must_use]
    pub fn terminal(&self) -> &ThreadTerminal {
        &self.terminal
    }

    /// The job's output as terminal lines.
    #[must_use]
    pub fn screen(&self) -> &TerminalScreen {
        &self.screen
    }

    /// Whether older output is missing: dropped by the Forge, or cleared by
    /// the job itself.
    #[must_use]
    pub fn missing_earlier_output(&self) -> bool {
        self.dropped || self.screen.cleared_earlier() || self.screen.truncated_line_count() > 0
    }

    /// Feeds the output carried on `self.terminal`, then drops it.
    fn feed_pushed(&mut self) {
        let output = std::mem::take(&mut self.terminal.output);
        let end = output.end();
        if end <= self.next {
            return;
        }
        if output.start > self.next {
            // The Forge dropped output between pushes: start afresh.
            self.screen = TerminalScreen::new();
            self.dropped = true;
            self.screen.feed(&output.bytes);
        } else {
            let skip = usize::try_from(self.next - output.start).unwrap_or(usize::MAX);
            self.screen
                .feed(output.bytes.get(skip..).unwrap_or_default());
        }
        self.next = end;
    }

    /// Takes a pushed update; returns whether its facts changed and whether
    /// it brought output.
    fn update(&mut self, pushed: ThreadTerminal) -> (bool, bool) {
        let output = pushed.output.end() > self.next;
        let facts = self.terminal.command != pushed.command
            || self.terminal.background != pushed.background
            || self.terminal.state != pushed.state
            || self.terminal.ended_at != pushed.ended_at;
        self.terminal = pushed;
        self.feed_pushed();
        (facts, output)
    }

    fn entry(&self) -> ThreadTerminalEntry {
        ThreadTerminalEntry {
            terminal_id: self.terminal.terminal_id.clone(),
            name: terminal_name(self.terminal.command.as_deref()),
            state: self.terminal.state,
            background: self.terminal.background,
        }
    }
}

/// What one push changed.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MirrorChange {
    /// A job started, stopped, left, or changed its facts.
    pub entries: bool,
    /// Jobs whose output grew.
    pub output: Vec<String>,
}

/// The shell jobs of one thread with their output.
#[derive(Debug)]
pub struct ThreadTerminalsMirror {
    thread_id: ThreadId,
    /// Oldest first, as the Forge orders them.
    terminals: Vec<MirroredTerminal>,
}

impl ThreadTerminalsMirror {
    /// An empty mirror of `thread_id`'s jobs.
    #[must_use]
    pub fn new(thread_id: ThreadId) -> Self {
        Self {
            thread_id,
            terminals: Vec::new(),
        }
    }

    /// The mirrored thread.
    #[must_use]
    pub fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Takes one push. A job the push no longer lists is gone.
    pub fn apply(&mut self, pushed: ThreadTerminals) -> MirrorChange {
        let mut change = MirrorChange::default();
        let mut previous = std::mem::take(&mut self.terminals);
        change.entries = previous.len() != pushed.terminals.len();
        for terminal in pushed.terminals {
            let existing = previous
                .iter()
                .position(|mirrored| mirrored.terminal.terminal_id == terminal.terminal_id)
                .map(|index| previous.swap_remove(index));
            let mirrored = if let Some(mut mirrored) = existing {
                let (facts, output) = mirrored.update(terminal);
                change.entries |= facts;
                if output {
                    change.output.push(mirrored.terminal.terminal_id.clone());
                }
                mirrored
            } else {
                change.entries = true;
                let mirrored = MirroredTerminal::new(terminal);
                change.output.push(mirrored.terminal.terminal_id.clone());
                mirrored
            };
            self.terminals.push(mirrored);
        }
        change
    }

    /// One mirrored job.
    #[must_use]
    pub fn terminal(&self, terminal_id: &str) -> Option<&MirroredTerminal> {
        self.terminals
            .iter()
            .find(|mirrored| mirrored.terminal.terminal_id == terminal_id)
    }

    /// The inspector rows: running jobs first, then stopped ones, newest
    /// first within each.
    #[must_use]
    pub fn entries(&self) -> Vec<ThreadTerminalEntry> {
        let running = self
            .terminals
            .iter()
            .rev()
            .filter(|mirrored| mirrored.terminal.state.is_running());
        let stopped = self
            .terminals
            .iter()
            .rev()
            .filter(|mirrored| !mirrored.terminal.state.is_running());
        running
            .chain(stopped)
            .map(MirroredTerminal::entry)
            .collect()
    }
}

/// A command's first line, trimmed, or [`UNNAMED_TERMINAL_LABEL`].
#[must_use]
pub fn terminal_name(command: Option<&str>) -> String {
    command
        .and_then(|command| command.lines().map(str::trim).find(|line| !line.is_empty()))
        .map_or_else(|| UNNAMED_TERMINAL_LABEL.to_owned(), str::to_owned)
}

/// The prompt of the shell a job ran in, drawn before its command.
///
/// Engines run commands through the Forge machine's shell, not a terminal
/// emulator, so the prompt follows the machine: PowerShell on Windows, a
/// POSIX shell (bash or zsh) on Linux, macOS, and WSL. The project's root
/// path tells which, since the Forge reports it in its own form.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShellPrompt {
    /// What the shell prints before a command, e.g. `~/projects/app$ ` or
    /// `PS C:\Users\me\app> `.
    pub prompt: String,
    /// What the shell prints before each continuation line of a command.
    pub continuation: &'static str,
    /// Whether the prompt is coloured, as default POSIX prompts are.
    pub coloured: bool,
}

impl Default for ShellPrompt {
    fn default() -> Self {
        Self::for_directory(None)
    }
}

impl ShellPrompt {
    /// The prompt of a shell sitting in `directory`, when it is known.
    #[must_use]
    pub fn for_directory(directory: Option<&str>) -> Self {
        let directory = directory.map(str::trim).filter(|path| !path.is_empty());
        match directory {
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

/// The selected thread's shell jobs and the output dialog.
pub struct ThreadTerminalsView {
    mirror: Option<ThreadTerminalsMirror>,
    /// The job whose dialog is open.
    open: Option<String>,
    /// The prompt of the shell the selected thread's jobs run in.
    prompt: ShellPrompt,
    list: ListState,
    /// What the list was last told about the open job.
    shown: Layout,
    following: bool,
    copied: bool,
    focus: FocusHandle,
    copy_focus: FocusHandle,
    latest_focus: FocusHandle,
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
    fn of(terminal: &MirroredTerminal) -> Self {
        let screen = terminal.screen();
        let commands = command_lines(terminal.terminal().command.as_deref()).len();
        let lead = commands + usize::from(terminal.missing_earlier_output());
        let trail = match terminal.terminal().state {
            // `^C`, then the prompt the shell returns to.
            ThreadTerminalState::Killed => 2,
            // The cursor while it runs; the next prompt once it stopped.
            _ => 1,
        };
        Self {
            commands,
            lead,
            lines: screen.line_count(),
            trail,
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
fn command_lines(command: Option<&str>) -> Vec<&str> {
    let lines: Vec<&str> = command
        .map(|command| command.trim_end().lines().collect())
        .unwrap_or_default();
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
    /// The running job's cursor.
    Cursor,
    /// The stopped job's interrupt.
    Interrupt,
    /// The prompt the shell returned to.
    Prompt,
}

impl Row {
    fn at(state: ThreadTerminalState, layout: Layout, index: usize) -> Option<Self> {
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
            (0, ThreadTerminalState::Running) => Some(Self::Cursor),
            (0, ThreadTerminalState::Killed) => Some(Self::Interrupt),
            (1, ThreadTerminalState::Killed) | (0, _) => Some(Self::Prompt),
            _ => None,
        }
    }
}

impl ThreadTerminalsView {
    /// A view with no thread.
    pub fn new(cx: &mut Context<Self>) -> Self {
        let list = ListState::new(0, ListAlignment::Top, px(OUTPUT_LINE_HEIGHT_PX * 20.0));
        let view = cx.entity().downgrade();
        list.set_scroll_handler(move |_, _, cx| {
            let _ = view.update(cx, Self::note_scroll);
        });
        Self {
            mirror: None,
            open: None,
            prompt: ShellPrompt::default(),
            list,
            shown: Layout::default(),
            following: true,
            copied: false,
            focus: cx.focus_handle(),
            copy_focus: cx.focus_handle(),
            latest_focus: cx.focus_handle(),
        }
    }

    /// Follows `thread`: another thread's jobs and dialog go away.
    pub fn select_thread(&mut self, thread: Option<&ThreadId>, cx: &mut Context<Self>) {
        if self.mirror.as_ref().map(ThreadTerminalsMirror::thread_id) == thread {
            return;
        }
        self.mirror = thread.cloned().map(ThreadTerminalsMirror::new);
        if self.open.take().is_some() {
            cx.notify();
        }
    }

    /// Sets the directory the selected thread's jobs run in: its project's
    /// root, as the Forge reports it.
    pub fn set_directory(&mut self, directory: Option<&str>, cx: &mut Context<Self>) {
        let prompt = ShellPrompt::for_directory(directory);
        if prompt != self.prompt {
            self.prompt = prompt;
            if self.open.is_some() {
                cx.notify();
            }
        }
    }

    /// Takes one push for the selected thread. Returns whether the
    /// inspector rows changed; a push for another thread is stale and
    /// changes nothing.
    pub fn apply(&mut self, pushed: ThreadTerminals, cx: &mut Context<Self>) -> bool {
        let Some(mirror) = self
            .mirror
            .as_mut()
            .filter(|mirror| mirror.thread_id() == &pushed.thread_id)
        else {
            return false;
        };
        let change = mirror.apply(pushed);
        let Some(open) = self.open.clone() else {
            return change.entries;
        };
        if mirror.terminal(&open).is_none() {
            self.open = None;
            cx.notify();
        } else if change.entries || change.output.contains(&open) {
            // A stop swaps the cursor for the next prompt.
            self.sync_list();
            cx.notify();
        }
        change.entries
    }

    /// The inspector rows of the selected thread.
    #[must_use]
    pub fn entries(&self) -> Vec<ThreadTerminalEntry> {
        self.mirror
            .as_ref()
            .map(ThreadTerminalsMirror::entries)
            .unwrap_or_default()
    }

    /// Opens the dialog on `terminal_id`, scrolled to the newest output.
    pub fn open(&mut self, terminal_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let known = self
            .mirror
            .as_ref()
            .is_some_and(|mirror| mirror.terminal(terminal_id).is_some());
        if !known {
            return;
        }
        self.open = Some(terminal_id.to_owned());
        self.shown = Layout::default();
        self.copied = false;
        self.list.reset(0);
        self.sync_list();
        self.list.set_follow_mode(FollowMode::Tail);
        self.following = true;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Closes the dialog.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        if self.open.take().is_some() {
            cx.notify();
        }
    }

    /// Whether the dialog is open.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    fn open_terminal(&self) -> Option<&MirroredTerminal> {
        let open = self.open.as_deref()?;
        self.mirror.as_ref()?.terminal(open)
    }

    /// Tells the list what changed on the open job: lines dropped from the
    /// front of its output, new lines, the last few lines, which output may
    /// have rewritten in place, and the rows after them, which change when
    /// the job stops.
    fn sync_list(&mut self) {
        let Some(next) = self.open_terminal().map(Layout::of) else {
            return;
        };
        let shown = self.shown;
        self.shown = next;
        let dropped = usize::try_from(next.truncated - shown.truncated).unwrap_or(usize::MAX);
        if next.lead != shown.lead || next.cleared != shown.cleared || dropped >= shown.lines {
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
        let Some(terminal) = self.open_terminal() else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(terminal.screen().plain_text()));
        self.copied = true;
        cx.notify();
    }

    fn render_row(&self, index: usize, theme: &ArtisanTheme) -> AnyElement {
        let row = div()
            .w_full()
            .min_h(px(OUTPUT_LINE_HEIGHT_PX))
            .px(px(16.0))
            .flex()
            .items_center();
        let Some(terminal) = self.open_terminal() else {
            return row.into_any_element();
        };
        let facts = terminal.terminal();
        let Some(kind) = Row::at(facts.state, self.shown, index) else {
            return row.into_any_element();
        };
        let palette = TerminalPalette::for_theme(theme);
        let foreground = theme.colors.foreground.to_paint();
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
        let prompt = &self.prompt;
        let child = match kind {
            Row::Command(line) => {
                let text = command_lines(facts.command.as_deref())
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
            Row::Output(line) => match terminal.screen().line(line) {
                Some(line) => {
                    let background = gpui::transparent_black();
                    let runs = highlights(line, &palette, foreground, background);
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
        let view = cx.entity().downgrade();
        let output = list(self.list.clone(), move |index, _, cx| {
            view.upgrade().map_or_else(
                || div().into_any_element(),
                |view| view.read(cx).render_row(index, &theme),
            )
        })
        .w_full()
        .h_full();
        let card = div()
            .id("artisan-thread-terminal-card")
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
            .debug_selector(|| THREAD_TERMINAL_OUTPUT_SELECTOR.to_owned())
            .child(glass_material_layer(GlassStrength::Strong, radius))
            .child(glass_highlight_layer(GlassStrength::Strong, radius))
            .child(
                div()
                    .relative()
                    .size_full()
                    .py(px(14.0))
                    .font_family(theme.typography.mono.family)
                    .tracking(0.0)
                    .text_size(px(OUTPUT_TEXT_PX))
                    .line_height(px(OUTPUT_LINE_HEIGHT_PX))
                    .text_color(theme.colors.foreground.to_paint())
                    .child(output),
            )
            .children(self.render_controls(&theme, cx));
        let backdrop = div()
            .absolute()
            .top(px(0.0))
            .right(px(0.0))
            .bottom(px(0.0))
            .left(px(0.0))
            .bg(gpui::black().opacity(BACKDROP_OPACITY))
            .occlude()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _: &MouseDownEvent, _, cx| {
                    view.close(cx);
                    cx.stop_propagation();
                }),
            );
        let overlay = div()
            .absolute()
            .top(px(0.0))
            .right(px(0.0))
            .bottom(px(0.0))
            .left(px(0.0))
            .flex()
            .items_center()
            .justify_center()
            .track_focus(&self.focus)
            .debug_selector(|| THREAD_TERMINAL_DIALOG_SELECTOR.to_owned())
            .on_key_down(cx.listener(|view, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key.eq_ignore_ascii_case("escape")
                    && !event.keystroke.modifiers.modified()
                {
                    window.prevent_default();
                    view.close(cx);
                    cx.stop_propagation();
                }
            }))
            .child(backdrop)
            .child(card);
        let overlay = if cx.reduce_motion() {
            overlay.into_any_element()
        } else {
            overlay
                .with_animation(
                    "artisan-thread-terminal-open",
                    DialogAnimation.gpui_clock(),
                    |element, progress| element.opacity(progress),
                )
                .into_any_element()
        };
        deferred(overlay)
            .with_priority(DIALOG_DEFERRED_PRIORITY)
            .into_any_element()
    }

    /// The card's floating controls: copy, revealed while the pointer is
    /// over the card, and a jump back to the newest output once the reader
    /// scrolled up.
    fn render_controls(&self, theme: &ArtisanTheme, cx: &Context<Self>) -> Vec<AnyElement> {
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
        let (copy_icon, copy_label) = if self.copied {
            (AssetId::TABLER_CHECK, "Copied")
        } else {
            (AssetId::TABLER_COPY, "Copy output")
        };
        let copy = icon_button(
            "artisan-thread-terminal-copy",
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
                    "artisan-thread-terminal-latest",
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

impl Render for ThreadTerminalsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.open_terminal().is_some() {
            self.render_dialog(window, cx)
        } else {
            div().into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use artisan_domain::{RunId, ThreadTerminalOutput, UnixMillis};
    use gpui::AppContext as _;

    use super::*;

    fn thread() -> ThreadId {
        ThreadId::parse("thread-1".to_owned()).expect("thread id")
    }

    fn job(id: &str, state: ThreadTerminalState, start: u64, bytes: &[u8]) -> ThreadTerminal {
        ThreadTerminal {
            terminal_id: id.to_owned(),
            run_id: RunId::parse("run-1".to_owned()).expect("run id"),
            command: Some("npm run dev\n# watch".to_owned()),
            background: true,
            state,
            started_at: UnixMillis::from_millis(1),
            ended_at: None,
            output: ThreadTerminalOutput {
                start,
                bytes: bytes.to_vec(),
            },
        }
    }

    fn push(terminals: Vec<ThreadTerminal>) -> ThreadTerminals {
        ThreadTerminals {
            thread_id: thread(),
            terminals,
        }
    }

    fn text(mirror: &ThreadTerminalsMirror, id: &str) -> String {
        mirror.terminal(id).expect("job").screen().plain_text()
    }

    #[test]
    fn output_split_across_pushes_parses_whole() {
        let mut mirror = ThreadTerminalsMirror::new(thread());
        let running = ThreadTerminalState::Running;
        mirror.apply(push(vec![job("a", running, 0, b"\x1b[3")]));
        let change = mirror.apply(push(vec![job("a", running, 3, b"1mred\x1b[0m\n")]));
        assert_eq!(change.output, vec!["a".to_owned()]);
        assert!(!change.entries);
        let terminal = mirror.terminal("a").expect("job");
        let line = terminal.screen().line(0).expect("line");
        assert_eq!(line.text, "red");
        assert!(!line.runs.is_empty());
        assert!(!terminal.missing_earlier_output());
    }

    #[test]
    fn a_resent_window_feeds_only_new_bytes() {
        let mut mirror = ThreadTerminalsMirror::new(thread());
        let running = ThreadTerminalState::Running;
        mirror.apply(push(vec![job("a", running, 0, b"one\n")]));
        mirror.apply(push(vec![job("a", running, 0, b"one\ntwo\n")]));
        assert_eq!(text(&mirror, "a"), "one\ntwo");
    }

    #[test]
    fn a_gap_starts_the_screen_afresh_and_says_so() {
        let mut mirror = ThreadTerminalsMirror::new(thread());
        let running = ThreadTerminalState::Running;
        mirror.apply(push(vec![job("a", running, 0, b"one\n")]));
        mirror.apply(push(vec![job("a", running, 100, b"later\n")]));
        assert_eq!(text(&mirror, "a"), "later");
        assert!(mirror.terminal("a").expect("job").missing_earlier_output());
    }

    #[test]
    fn running_jobs_list_first_and_a_stop_changes_the_rows() {
        let mut mirror = ThreadTerminalsMirror::new(thread());
        let running = ThreadTerminalState::Running;
        mirror.apply(push(vec![
            job("old", running, 0, b""),
            job("new", running, 0, b""),
        ]));
        let stopped = ThreadTerminalState::Exited { code: Some(1) };
        let change = mirror.apply(push(vec![
            job("old", running, 0, b""),
            job("new", stopped, 0, b""),
        ]));
        assert!(change.entries);
        let entries = mirror.entries();
        assert_eq!(entries[0].terminal_id, "old");
        assert_eq!(entries[1].terminal_id, "new");
        assert_eq!(entries[0].name, "npm run dev");
    }

    #[test]
    fn a_job_missing_from_a_push_is_gone() {
        let mut mirror = ThreadTerminalsMirror::new(thread());
        let running = ThreadTerminalState::Running;
        mirror.apply(push(vec![job("a", running, 0, b"")]));
        let change = mirror.apply(push(Vec::new()));
        assert!(change.entries);
        assert!(mirror.terminal("a").is_none());
    }

    struct Probe {
        terminals: gpui::Entity<ThreadTerminalsView>,
    }

    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().relative().child(self.terminals.clone())
        }
    }

    fn lines(range: std::ops::Range<usize>) -> Vec<u8> {
        range
            .flat_map(|index| format!("\x1b[32mline\x1b[0m {index}\n").into_bytes())
            .collect()
    }

    /// The dialog opens on the newest output, follows what arrives while the
    /// reader is at the bottom, and closes when its job is gone.
    #[gpui::test]
    fn the_dialog_opens_at_the_bottom_and_follows(cx: &mut gpui::TestAppContext) {
        let (probe, cx) = cx.add_window_view(|_, cx| Probe {
            terminals: cx.new(ThreadTerminalsView::new),
        });
        let terminals = cx.update(|_, app| probe.read(app).terminals.clone());
        let running = ThreadTerminalState::Running;
        let first = lines(0..400);
        let first_len = first.len() as u64;
        cx.update(|window, app| {
            terminals.update(app, |view, cx| {
                view.select_thread(Some(&thread()), cx);
                assert!(view.apply(push(vec![job("a", running, 0, &first)]), cx));
                view.open("a", window, cx);
            });
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds(THREAD_TERMINAL_OUTPUT_SELECTOR).is_some());
        let top = cx.update(|_, app| terminals.read(app).list.logical_scroll_top().item_ix);
        assert!(
            top > 300,
            "the dialog opens at the newest output, not line {top}"
        );

        cx.update(|_, app| {
            terminals.update(app, |view, cx| {
                assert!(!view.apply(
                    push(vec![job("a", running, first_len, &lines(400..410))]),
                    cx
                ));
            });
        });
        cx.run_until_parked();
        cx.update(|_, app| {
            let view = terminals.read(app);
            // Two command lines, the output, and the running job's cursor.
            assert_eq!(view.list.item_count(), 2 + 410 + 1);
            assert!(view.list.is_following_tail());
        });

        cx.update(|_, app| {
            terminals.update(app, |view, cx| {
                let killed = ThreadTerminalState::Killed;
                assert!(view.apply(push(vec![job("a", killed, first_len, b"")]), cx));
                // `^C` and the prompt the shell returns to replace the cursor.
                assert_eq!(view.list.item_count(), 2 + 410 + 2);
                assert_eq!(Row::at(killed, view.shown, 2 + 410), Some(Row::Interrupt));
                assert_eq!(Row::at(killed, view.shown, 2 + 411), Some(Row::Prompt));
            });
        });

        cx.update(|_, app| {
            terminals.update(app, |view, cx| {
                assert!(view.apply(push(Vec::new()), cx));
                assert!(!view.is_open());
            });
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds(THREAD_TERMINAL_OUTPUT_SELECTOR).is_none());
    }

    #[test]
    fn a_job_without_a_command_shows_a_bare_prompt() {
        assert_eq!(terminal_name(None), UNNAMED_TERMINAL_LABEL);
        assert_eq!(command_lines(None), vec![""]);
        assert_eq!(command_lines(Some("cat <<'EOF'\nhi\nEOF\n")).len(), 3);
    }

    #[test]
    fn the_prompt_follows_the_machine_the_job_ran_on() {
        let prompt = |path| ShellPrompt::for_directory(Some(path)).prompt;
        assert_eq!(prompt("/home/me/projects/app"), "~/projects/app$ ");
        assert_eq!(prompt("/Users/me/app/"), "~/app$ ");
        assert_eq!(prompt("/home/me"), "~$ ");
        assert_eq!(prompt("/root/app"), "~/app$ ");
        assert_eq!(prompt("/srv/app"), "/srv/app$ ");
        assert_eq!(prompt("/"), "/$ ");
        let windows = ShellPrompt::for_directory(Some(r"C:\Users\me\Projects\"));
        assert_eq!(windows.prompt, r"PS C:\Users\me\Projects> ");
        assert_eq!(windows.continuation, ">> ");
        assert!(!windows.coloured);
        assert_eq!(ShellPrompt::for_directory(None).prompt, "$ ");
    }
}
