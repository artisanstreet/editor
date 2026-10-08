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
//! the dialog: the job's output with its colours, scrollable through
//! everything kept, opening at the bottom. It follows new output while the
//! reader is at the bottom and stays put once they scroll up; scrolling back
//! down follows again. New output re-renders only this view: the inspector
//! changes only when a job starts, stops, or leaves.

#![forbid(unsafe_code)]

use artisan_assets::AssetId;
use artisan_domain::{ThreadId, ThreadTerminal, ThreadTerminalState, ThreadTerminals};
use artisan_ui::button::{Button, ButtonContent, ButtonSize, ButtonVariant};
use artisan_ui::dialog::{Dialog, DialogStyle};
use artisan_ui::motion::MotionPolicy;
use artisan_ui::terminal_text::{TerminalPalette, TerminalScreen, highlights};
use artisan_ui::theme::{ArtisanTheme, RadiusStep, RadiusTokens, ThemeMode};
use gpui::prelude::{InteractiveElement as _, ParentElement as _, Styled as _};
use gpui::{
    AnyElement, ClipboardItem, Context, FocusHandle, FollowMode, IntoElement, ListAlignment,
    ListState, Render, SharedString, StyledText, WeakEntity, Window, deferred, div, list, px,
};

/// Stable debug selector prefix for the output dialog.
pub const THREAD_TERMINAL_DIALOG_SELECTOR: &str = "artisan-thread-terminal-dialog";

/// Stable debug selector for the dialog's output list.
pub const THREAD_TERMINAL_OUTPUT_SELECTOR: &str = "artisan-thread-terminal-output";

/// Above the shell's deferred menus, below the image preview.
const DIALOG_DEFERRED_PRIORITY: usize = 90;

/// Widest the output dialog grows.
const DIALOG_MAX_WIDTH_PX: f32 = 1040.0;

/// Room the dialog leaves around itself inside the window.
const DIALOG_WINDOW_MARGIN_PX: f32 = 48.0;

/// Tallest the output area grows.
const OUTPUT_MAX_HEIGHT_PX: f32 = 640.0;

/// Share of the window height the output area may take.
const OUTPUT_HEIGHT_SHARE: f32 = 0.62;

/// Output text size and line height.
const OUTPUT_TEXT_PX: f32 = 12.5;
const OUTPUT_LINE_HEIGHT_PX: f32 = 18.0;

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

/// How a job's state reads in a sentence.
#[must_use]
pub fn terminal_status(state: ThreadTerminalState) -> String {
    match state {
        ThreadTerminalState::Running => "Running".to_owned(),
        ThreadTerminalState::Exited { code: Some(code) } => format!("Exited with code {code}"),
        ThreadTerminalState::Exited { code: None } => "Exited".to_owned(),
        ThreadTerminalState::Failed => "Failed".to_owned(),
        ThreadTerminalState::Killed => "Stopped".to_owned(),
        ThreadTerminalState::Ended => "Ended with the agent's turn".to_owned(),
    }
}

/// The selected thread's shell jobs and the output dialog.
pub struct ThreadTerminalsView {
    mirror: Option<ThreadTerminalsMirror>,
    /// The job whose dialog is open.
    open: Option<String>,
    list: ListState,
    /// What the list was last told about the open job's screen.
    shown: ShownLines,
    following: bool,
    copied: bool,
    focus: FocusHandle,
    copy_focus: FocusHandle,
    latest_focus: FocusHandle,
}

#[derive(Clone, Copy, Debug, Default)]
struct ShownLines {
    lines: usize,
    truncated: u64,
    cleared: u64,
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
            list,
            shown: ShownLines::default(),
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
        } else if change.output.contains(&open) {
            self.sync_list();
            cx.notify();
        } else if change.entries {
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
        self.shown = ShownLines::default();
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

    /// Tells the list what changed on the open job's screen: lines dropped
    /// from the front, new lines, and the last few lines, which output may
    /// have rewritten in place.
    fn sync_list(&mut self) {
        let Some(screen) = self.open_terminal().map(MirroredTerminal::screen) else {
            return;
        };
        let next = ShownLines {
            lines: screen.line_count(),
            truncated: screen.truncated_line_count(),
            cleared: screen.cleared_line_count(),
        };
        let shown = self.shown;
        self.shown = next;
        let dropped = usize::try_from(next.truncated - shown.truncated).unwrap_or(usize::MAX);
        if next.cleared != shown.cleared || dropped >= shown.lines {
            self.list.reset(next.lines);
            return;
        }
        if dropped > 0 {
            self.list.splice(0..dropped, 0);
        }
        let kept = shown.lines - dropped;
        let from = kept.saturating_sub(REMEASURED_TAIL_LINES);
        if next.lines < from {
            self.list.reset(next.lines);
            return;
        }
        self.list.splice(from..kept, next.lines - from);
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

    fn render_line(&self, index: usize, theme: &ArtisanTheme) -> AnyElement {
        let style = DialogStyle::resolve(*theme, MotionPolicy::Reduced);
        let line = self
            .open_terminal()
            .and_then(|terminal| terminal.screen().line(index));
        let row = div().w_full().min_h(px(OUTPUT_LINE_HEIGHT_PX)).px(px(12.0));
        let Some(line) = line else {
            return row.into_any_element();
        };
        let palette = TerminalPalette::for_theme(theme);
        let runs = highlights(line, &palette, style.foreground, style.background);
        row.child(StyledText::new(SharedString::from(line.text.clone())).with_highlights(runs))
            .into_any_element()
    }

    fn render_dialog(
        &self,
        terminal: &MirroredTerminal,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
        let viewport = window.viewport_size();
        let width = (f32::from(viewport.width) - 2.0 * DIALOG_WINDOW_MARGIN_PX)
            .clamp(320.0, DIALOG_MAX_WIDTH_PX);
        let height =
            (f32::from(viewport.height) * OUTPUT_HEIGHT_SHARE).clamp(160.0, OUTPUT_MAX_HEIGHT_PX);
        let facts = terminal.terminal();
        let mut status = terminal_status(facts.state);
        if facts.background {
            status.push_str(" · background");
        }
        let view = cx.entity().downgrade();
        let render_theme = theme;
        let output = list(self.list.clone(), move |index, _, cx| {
            view.upgrade().map_or_else(
                || div().into_any_element(),
                |view| view.read(cx).render_line(index, &render_theme),
            )
        })
        .w_full()
        .h_full();
        let empty = terminal.screen().line_count() == 0;
        let body = div()
            .relative()
            .w(px(width))
            .h(px(height))
            .flex()
            .flex_col()
            .rounded(RadiusTokens::value(RadiusStep::Lg))
            .border_1()
            .border_color(theme.colors.border.to_paint())
            .overflow_hidden()
            .font_family(theme.typography.mono.family)
            .text_size(px(OUTPUT_TEXT_PX))
            .line_height(px(OUTPUT_LINE_HEIGHT_PX))
            .debug_selector(|| THREAD_TERMINAL_OUTPUT_SELECTOR.to_owned())
            .children(terminal.missing_earlier_output().then(|| {
                div()
                    .flex_shrink_0()
                    .px(px(12.0))
                    .py(px(6.0))
                    .border_b_1()
                    .border_color(theme.colors.border.to_paint())
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child("Earlier output is not kept; showing the newest.")
            }))
            .child(if empty {
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child("No output yet.")
                    .into_any_element()
            } else {
                div()
                    .flex_1()
                    .min_h_0()
                    .py(px(8.0))
                    .child(output)
                    .into_any_element()
            });
        let footer = self.render_footer(&theme, cx);
        let close_view = cx.entity().downgrade();
        let dialog = Dialog::new(
            "artisan-thread-terminal-dialog",
            self.focus.clone(),
            theme,
            MotionPolicy::Full,
            true,
            terminal_name(facts.command.as_deref()),
            div()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .child(body)
                .child(footer),
        )
        .description(status)
        .max_width(px(width + 2.0 * 24.0))
        .debug_selector(THREAD_TERMINAL_DIALOG_SELECTOR)
        .on_dismiss(move |_, _, cx| {
            let _ = close_view.update(cx, Self::close);
        });
        deferred(
            div()
                .absolute()
                .left(px(0.0))
                .top(px(0.0))
                .right(px(0.0))
                .bottom(px(0.0))
                .track_focus(&self.focus)
                .child(dialog),
        )
        .with_priority(DIALOG_DEFERRED_PRIORITY)
        .into_any_element()
    }

    fn render_footer(&self, theme: &ArtisanTheme, cx: &Context<Self>) -> gpui::Div {
        let copy_view = cx.entity().downgrade();
        let copy = Button::new(
            "artisan-thread-terminal-copy",
            self.copy_focus.clone(),
            *theme,
            MotionPolicy::Full,
            ButtonVariant::Ghost,
            ButtonSize::Small,
            if self.copied {
                ButtonContent::icon_text(AssetId::TABLER_CHECK, "Copied")
            } else {
                ButtonContent::icon_text(AssetId::TABLER_COPY, "Copy output")
            },
        )
        .ok()
        .map(|button| {
            button.on_activate(move |_, _, cx| {
                let _ = copy_view.update(cx, Self::copy_output);
            })
        });
        let latest_view: WeakEntity<Self> = cx.entity().downgrade();
        let latest = (!self.following)
            .then(|| {
                Button::new(
                    "artisan-thread-terminal-latest",
                    self.latest_focus.clone(),
                    *theme,
                    MotionPolicy::Full,
                    ButtonVariant::Ghost,
                    ButtonSize::Small,
                    ButtonContent::icon_text(AssetId::TABLER_ARROW_BAR_TO_DOWN, "Jump to latest"),
                )
                .ok()
            })
            .flatten()
            .map(|button| {
                button.on_activate(move |_, _, cx| {
                    let _ = latest_view.update(cx, Self::jump_to_latest);
                })
            });
        div()
            .flex()
            .items_center()
            .justify_end()
            .gap(px(8.0))
            .children(latest)
            .children(copy)
    }
}

impl Render for ThreadTerminalsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        match self.open_terminal() {
            Some(terminal) => self.render_dialog(terminal, window, cx),
            None => div().into_any_element(),
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
            assert_eq!(view.list.item_count(), 410);
            assert!(view.list.is_following_tail());
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
    fn statuses_read_as_sentences() {
        assert_eq!(
            terminal_status(ThreadTerminalState::Exited { code: Some(3) }),
            "Exited with code 3"
        );
        assert_eq!(terminal_name(None), UNNAMED_TERMINAL_LABEL);
    }
}
