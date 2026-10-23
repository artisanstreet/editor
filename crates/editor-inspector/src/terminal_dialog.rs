//! A shell Item retained by identity, with incremental terminal parsing and shared modal ownership.
use crate::{ShellScreen, model::shell_output};
use artisan_api::{Item, ItemId, ItemKind, LiveTarget, Status, ThreadId, ToolKind};
use editor_conversation::{Conversation, ConversationEvent};
use editor_core::{Core, HostEvent, HostId};
use editor_ui::theme::{ActiveTheme as _, TextStep, TypographyTokens};
use editor_ui::{
    AssetId,
    button::{Button, ButtonVariant},
    copy_feedback::COPY_FEEDBACK_WINDOW,
    dialog::Dialog,
    glass::{
        GlassStrength, glass_blur_radius, glass_card_shadows, glass_foreground_base,
        glass_highlight_layer, glass_material_layer,
    },
    terminal_text::{TerminalPalette, highlights},
    theme::{ArtisanTheme, Opacity, RadiusStep, RadiusTokens},
};
use gpui::{
    AnyElement, AppContext, ClipboardItem, Context, FollowMode, InteractiveElement as _,
    IntoElement, ListAlignment, ListState, ParentElement as _, Render, SharedString, Styled as _,
    StyledText, Subscription, Task, Window, div, list, px,
};
use std::{ops::Range, sync::Arc};
pub const TERMINAL_DIALOG_SELECTOR: &str = "inspector-terminal-dialog";
const OUTPUT_LINE_HEIGHT: f32 = 18.0;

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
    home_end.map_or_else(|| path.to_owned(), |end| format!("~{}", &path[end..]))
}

struct Job {
    host: HostId,
    thread: ThreadId,
    item: ItemId,
    data: Option<Arc<Item>>,
    command: String,
    commands: Vec<Range<usize>>,
    #[cfg(test)]
    command_offset_builds: usize,
    cwd: Option<String>,
    status: Status,
    shell: ShellScreen,
    source: Option<(Option<u64>, usize, u64)>,
    unverified: bool,
    command_source: Option<gpui::Entity<editor_sources::ContentText>>,
    output_source: Option<gpui::Entity<editor_sources::ContentText>>,
}
impl Job {
    const fn missing(&self) -> bool {
        self.unverified
            || self.shell.screen.cleared_earlier()
            || self.shell.screen.truncated_line_count() > 0
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Layout {
    commands: usize,
    lead: usize,
    lines: usize,
    truncated: u64,
    cleared: u64,
}
impl Layout {
    fn of(job: &Job) -> Self {
        Self {
            commands: job.commands.len(),
            lead: job.commands.len() + usize::from(job.missing()),
            lines: job.shell.screen.line_count(),
            truncated: job.shell.screen.truncated_line_count(),
            cleared: job.shell.screen.cleared_line_count(),
        }
    }
    const fn total(self) -> usize {
        self.lead + self.lines + 1
    }
    const fn output(self) -> Range<usize> {
        self.lead..self.lead + self.lines
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Row {
    Command(usize),
    Missing,
    Output(usize),
    Status,
}
impl Row {
    fn at(layout: Layout, index: usize) -> Option<Self> {
        if index < layout.commands {
            Some(Self::Command(index))
        } else if index < layout.lead {
            Some(Self::Missing)
        } else if layout.output().contains(&index) {
            Some(Self::Output(index - layout.lead))
        } else if index == layout.output().end {
            Some(Self::Status)
        } else {
            None
        }
    }
}
fn command_lines(command: &str) -> Vec<Range<usize>> {
    let mut offset = 0;
    command
        .split_inclusive('\n')
        .map(|line| {
            let start = offset;
            offset += line.len();
            start..start + line.trim_end_matches(['\r', '\n']).len()
        })
        .collect()
}
pub struct TerminalDialog {
    core: gpui::Entity<Core>,
    conversation: gpui::Entity<Conversation>,
    job: Option<Job>,
    list: ListState,
    shown: Layout,
    following: bool,
    copied: Option<Task<()>>,
    overlay_window: Option<gpui::AnyWindowHandle>,
    generation: u64,
    error: Option<String>,
    loading: bool,
    load: Task<()>,
    host_events: Option<Subscription>,
    _conversation_events: Subscription,
}
impl TerminalDialog {
    pub fn new(
        core: gpui::Entity<Core>,
        conversation: gpui::Entity<Conversation>,
        cx: &mut Context<Self>,
    ) -> Self {
        let list = ListState::new(0, ListAlignment::Top, px(OUTPUT_LINE_HEIGHT * 20.0));
        let owner = cx.entity().downgrade();
        list.set_scroll_handler(move |_, _, cx| {
            owner.update(cx, Self::note_scroll).ok();
        });
        let subscription = cx.subscribe(&conversation, |this, _, _: &ConversationEvent, cx| {
            this.follow(cx);
        });
        Self {
            core,
            conversation,
            job: None,
            list,
            shown: Layout::default(),
            following: true,
            copied: None,
            overlay_window: None,
            generation: 0,
            error: None,
            loading: false,
            load: Task::ready(()),
            host_events: None,
            _conversation_events: subscription,
        }
    }
    pub fn open(&mut self, item: &ItemId, window: &mut Window, cx: &mut Context<Self>) {
        let Some((host_id, thread)) = self.conversation.read(cx).open_thread().cloned() else {
            return;
        };
        let Some(host) = self.core.read(cx).host(&host_id).cloned() else {
            return;
        };
        let watched = item.clone();
        let watched_thread = thread.clone();
        self.host_events =
            Some(
                cx.subscribe(&host, move |this, _, event: &HostEvent, cx| match event {
                    HostEvent::Live {
                        thread_id,
                        target: LiveTarget::Shell { item_id },
                    } if thread_id == &watched_thread && item_id == &watched => this.sync(cx),
                    HostEvent::Item(item)
                        if item.id == watched && item.thread_id == watched_thread =>
                    {
                        this.set_item(Arc::clone(item));
                        this.sync(cx);
                    }
                    HostEvent::Reset => {
                        this.fetch(cx);
                        this.sync(cx);
                    }
                    HostEvent::ImagesChanged
                    | HostEvent::EngineInstall { .. }
                    | HostEvent::ThreadSummaryChanged { .. }
                    | HostEvent::Changed
                    | HostEvent::WslDistro { .. }
                    | HostEvent::ThreadChanged { .. }
                    | HostEvent::Item(_)
                    | HostEvent::Live { .. }
                    | HostEvent::EditorCall { .. } => {}
                }),
            );
        self.generation = self.generation.wrapping_add(1);
        self.job = Some(Job {
            host: host_id,
            thread,
            item: item.clone(),
            data: None,
            command: String::new(),
            commands: Vec::new(),
            #[cfg(test)]
            command_offset_builds: 0,
            cwd: None,
            status: Status::Running,
            shell: ShellScreen::new(),
            source: None,
            unverified: false,
            command_source: None,
            output_source: None,
        });
        self.shown = Layout::default();
        self.error = None;
        self.copied = None;
        self.list.reset(0);
        self.loading = false;
        self.list.set_follow_mode(FollowMode::Tail);
        self.following = true;
        editor_ui::overlay::begin(TERMINAL_DIALOG_SELECTOR, window, cx);
        self.overlay_window = Some(window.window_handle());
        self.fetch(cx);
        cx.notify();
    }
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss(cx);
        editor_ui::overlay::dismiss(TERMINAL_DIALOG_SELECTOR, window, cx);
    }
    fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.host_events = None;
        self.generation = self.generation.wrapping_add(1);
        self.load = Task::ready(());
        self.copied = None;
        self.loading = false;
        self.error = None;
        if self.job.take().is_some() {
            if let Some(window) = self.overlay_window.take() {
                window
                    .update(cx, |_, window, cx| {
                        editor_ui::overlay::dismiss(TERMINAL_DIALOG_SELECTOR, window, cx);
                    })
                    .ok();
            }
            cx.notify();
        }
    }
    pub const fn is_open(&self) -> bool {
        self.job.is_some()
    }
    fn follow(&mut self, cx: &mut Context<Self>) {
        if self.job.as_ref().is_some_and(|job| {
            self.conversation.read(cx).open_thread()
                != Some(&(job.host.clone(), job.thread.clone()))
        }) {
            self.dismiss(cx);
        }
    }
    fn fetch(&mut self, cx: &mut Context<Self>) {
        let Some(job) = &self.job else {
            return;
        };
        let Some(host) = self.core.read(cx).host(&job.host).cloned() else {
            return;
        };
        let client = host.read(cx).client.clone();
        let id = job.item.clone();
        let generation = self.generation;
        self.loading = true;
        self.error = None;
        self.load = cx.spawn(async move |this, cx| {
            let result = client.item(&id).await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(item) => {
                        this.set_item(Arc::new(item));
                        this.sync(cx);
                    }
                    Err(error) => {
                        this.error = Some(format!("Couldn't load shell output: {error:#}"));
                    }
                }
                cx.notify();
            })
            .ok();
        });
    }
    fn set_item(&mut self, item: Arc<Item>) {
        let Some(job) = &mut self.job else {
            return;
        };
        if item.id != job.item
            || item.thread_id != job.thread
            || job.data.as_ref().is_some_and(|known| known.seq > item.seq)
        {
            return;
        }
        job.data = Some(item);
    }
    fn sync(&mut self, cx: &mut Context<Self>) {
        let Some(job) = &mut self.job else {
            return;
        };
        let Some(item) = &job.data else {
            return;
        };
        let ItemKind::Tool {
            tool: ToolKind::Shell { command, cwd, .. },
            output,
            ..
        } = &item.kind
        else {
            self.error = Some("This item is not a shell command.".into());
            cx.notify();
            return;
        };
        let Some(host) = self.core.read(cx).host(&job.host).cloned() else {
            self.dismiss(cx);
            return;
        };
        let streamed = (!item.status.is_terminal())
            .then(|| {
                host.read(cx).live_snapshot(
                    &job.thread,
                    &LiveTarget::Shell {
                        item_id: job.item.clone(),
                    },
                )
            })
            .flatten();
        let (epoch, length) = streamed.as_ref().map_or((None, 0), |stream| {
            (Some(stream.epoch), stream.text.len_bytes())
        });
        let source = (epoch, length, item.seq);
        if job.source == Some(source) {
            return;
        }
        job.source = Some(source);
        match command {
            artisan_api::TextContent::Inline(command) => {
                job.command_source = None;
                if command != &job.command {
                    job.command.clone_from(command);
                    job.commands = command_lines(command);
                    #[cfg(test)]
                    {
                        job.command_offset_builds += 1;
                    }
                    self.list.reset(0);
                }
            }
            artisan_api::TextContent::Stored(source) => {
                job.command.clear();
                job.commands.clear();
                if job
                    .command_source
                    .as_ref()
                    .is_none_or(|view| view.read(cx).source(cx) != source)
                {
                    job.command_source = Some(cx.new(|cx| {
                        editor_sources::ContentText::new(&host, source.clone(), *cx.theme(), cx)
                    }));
                }
            }
        }
        job.cwd.clone_from(cwd);
        job.status = item.status;
        if let Some(stream) = streamed {
            job.output_source = None;
            job.shell.sync_rope(&stream.text, stream.epoch);
            job.unverified = false;
        } else if let Some(artisan_api::Output {
            text: artisan_api::TextContent::Stored(source),
            ..
        }) = output
        {
            if job
                .output_source
                .as_ref()
                .is_none_or(|view| view.read(cx).source(cx) != source)
            {
                job.output_source = Some(cx.new(|cx| {
                    editor_sources::ContentText::new(&host, source.clone(), *cx.theme(), cx)
                }));
            }
            job.unverified = false;
        } else if let Some(text) = shell_output(output.as_ref()) {
            job.output_source = None;
            job.shell.sync(text, None);
            job.unverified = false;
        } else if item.status.is_terminal() && job.shell.fed > 0 {
            job.unverified = true;
        }
        let dirty = job.shell.screen.take_dirty_line_indices();
        self.sync_list(&dirty);
        cx.notify();
    }
    fn sync_list(&mut self, dirty: &[usize]) {
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
            || dropped > shown.lines
            || next.lines < shown.lines.saturating_sub(dropped)
        {
            self.list.reset(next.total());
            return;
        }
        if shown.total() != self.list.item_count() {
            self.list.reset(next.total());
            return;
        }
        // Remove the old status and evicted head before replacing changed lines.
        self.list.splice(shown.output().end..shown.total(), 0);
        if dropped > 0 {
            self.list.splice(next.lead..next.lead + dropped, 0);
        }
        let kept = shown.lines - dropped;
        for &line in dirty.iter().filter(|line| **line < kept) {
            self.list.splice(next.lead + line..next.lead + line + 1, 1);
        }
        self.list
            .splice(next.lead + kept..next.lead + kept, next.lines - kept + 1);
    }
    fn note_scroll(&mut self, cx: &mut Context<Self>) {
        let following = self.list.is_following_tail();
        if self.following != following {
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
        let Some(job) = &self.job else {
            return;
        };
        let mut text = job.shell.screen.plain_text();
        if job.unverified {
            text.push_str("\n[Live output only; Forge supplied no final snapshot.]");
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(COPY_FEEDBACK_WINDOW).await;
            this.update(cx, |this, cx| {
                this.copied = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
    fn render_row(&self, index: usize, theme: &ArtisanTheme) -> AnyElement {
        let row = div().w_full().min_h(px(OUTPUT_LINE_HEIGHT)).px(px(16.0));
        let Some(job) = &self.job else {
            return row.into_any_element();
        };
        match Row::at(self.shown, index) {
            Some(Row::Command(index)) => {
                let prompt = ShellPrompt::for_directory(job.cwd.as_deref());
                let prefix = if index == 0 {
                    prompt.prompt.as_str()
                } else {
                    prompt.continuation
                };
                let command = job
                    .commands
                    .get(index)
                    .map(|range| &job.command[range.clone()])
                    .unwrap_or_default();
                let line = SharedString::from(format!("{prefix}{command}"));
                let highlights = if index == 0 && prompt.coloured {
                    vec![(
                        0..prefix.trim_end().trim_end_matches('$').len(),
                        gpui::HighlightStyle {
                            color: Some(TerminalPalette::for_theme(theme).base[12]),
                            font_weight: Some(gpui::FontWeight::BOLD),
                            ..gpui::HighlightStyle::default()
                        },
                    )]
                } else {
                    Vec::new()
                };
                row.child(StyledText::new(line).with_highlights(highlights))
                    .into_any_element()
            }
            Some(Row::Missing) => row
                .text_color(theme.colors.text_muted.to_paint())
                .child(if job.unverified {
                    "Live output only; Forge supplied no final snapshot."
                } else {
                    "Earlier output is not kept; showing the newest."
                })
                .into_any_element(),
            Some(Row::Output(index)) => {
                if let Some(line) = job.shell.screen.line(index) {
                    let palette = TerminalPalette::for_theme(theme);
                    let runs = highlights(
                        line,
                        &palette,
                        theme.colors.text.to_paint(),
                        gpui::transparent_black(),
                    );
                    row.child(
                        StyledText::new(SharedString::from(line.text.clone()))
                            .with_highlights(runs),
                    )
                    .into_any_element()
                } else {
                    row.into_any_element()
                }
            }
            Some(Row::Status) => {
                if job.status == Status::Running {
                    row.child(
                        div()
                            .w(px(7.5))
                            .h(px(15.0))
                            .bg(theme.colors.text_muted.to_paint()),
                    )
                    .into_any_element()
                } else {
                    row.child(ShellPrompt::for_directory(job.cwd.as_deref()).prompt)
                        .into_any_element()
                }
            }
            None => row.into_any_element(),
        }
    }
}
impl Render for TerminalDialog {
    #[expect(
        clippy::too_many_lines,
        reason = "the dialog's header, output list and footer build one element tree"
    )]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(job) = &self.job else {
            return div().into_any_element();
        };
        let theme = *cx.theme();
        let height = (f32::from(window.viewport_size().height) * 0.62)
            .min(640.0)
            .min((f32::from(window.viewport_size().height) - 160.0).max(32.0));
        let owner = cx.entity().downgrade();
        let actions = div()
            .absolute()
            .top(px(8.0))
            .right(px(8.0))
            .opacity(0.0)
            .group_hover("inspector-terminal-card", |style| style.opacity(1.0))
            .child(
                Button::icon(
                    "inspector-terminal-copy",
                    theme,
                    if self.copied.is_some() {
                        AssetId::TABLER_CHECK
                    } else {
                        AssetId::TABLER_COPY
                    },
                    "Copy output",
                )
                .variant(ButtonVariant::Ghost)
                .disabled(job.output_source.is_some())
                .on_activate(move |_, _, cx| {
                    owner.update(cx, Self::copy_output).ok();
                }),
            );
        let mut content = div().relative().flex().flex_col().py(px(14.0));
        if let Some(error) = &self.error {
            let owner = cx.entity().downgrade();
            content = content.child(error.clone()).child(
                Button::text("terminal-retry", theme, "Retry").on_activate(move |_, _, cx| {
                    owner.update(cx, Self::fetch).ok();
                }),
            );
        } else if self.loading && job.data.is_none() {
            content = content.child("Loading shell output…");
        }
        if let Some(source) = &job.command_source {
            content = content.child(source.clone());
        }
        if let Some(source) = &job.output_source {
            content = content.child(source.clone());
        }
        content = content.child(
            list(
                self.list.clone(),
                cx.processor(move |this, index, _, _| this.render_row(index, &theme)),
            )
            .h(px(height))
            .w_full()
            .font_family(theme.typography.mono.family)
            .text_size(TypographyTokens::size(TextStep::Terminal))
            .line_height(px(OUTPUT_LINE_HEIGHT)),
        );
        content = content.child(actions);
        if !self.following {
            let owner = cx.entity().downgrade();
            content = content.child(
                div().absolute().bottom(px(8.0)).right(px(8.0)).child(
                    Button::icon(
                        "inspector-terminal-latest",
                        theme,
                        AssetId::TABLER_ARROW_BAR_TO_DOWN,
                        "Jump to latest",
                    )
                    .variant(ButtonVariant::Ghost)
                    .on_activate(move |_, _, cx| {
                        owner.update(cx, Self::jump_to_latest).ok();
                    }),
                ),
            );
        }
        let radius = RadiusTokens::value(RadiusStep::X2l);
        let card = div()
            .id("inspector-terminal-card")
            .group("inspector-terminal-card")
            .relative()
            .w_full()
            .rounded(radius)
            .backdrop_blur(glass_blur_radius(GlassStrength::Strong))
            .bg(glass_foreground_base(&theme))
            .border_1()
            .border_color(theme.colors.text.at(Opacity::GlassBorder).to_paint())
            .shadow(glass_card_shadows(
                &theme,
                editor_ui::theme::Elevation::Overlay,
            ))
            .overflow_hidden()
            .occlude()
            .child(glass_material_layer(&theme, GlassStrength::Strong, radius))
            .child(glass_highlight_layer(&theme, GlassStrength::Strong, radius))
            .child(content);
        let owner = cx.entity().downgrade();
        let dialog = Dialog::new(TERMINAL_DIALOG_SELECTOR, theme, "Terminal output", card)
            .bare()
            .max_width(
                (window.viewport_size().width - px(96.0))
                    .max(px(1.0))
                    .min(px(1040.0)),
            )
            .debug_selector(TERMINAL_DIALOG_SELECTOR)
            .on_dismiss(move |_, window, cx| {
                owner.update(cx, |this, cx| this.close(window, cx)).ok();
            });
        dialog.into_any_element()
    }
}
#[cfg(test)]
mod tests;
