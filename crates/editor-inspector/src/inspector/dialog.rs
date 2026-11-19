use super::{StreamState, accepts_item, command_lines, shell_output};
use artisan_api::{Item, ItemId, ItemKind, LiveTarget, Status, TextContent, ThreadId, ToolKind};
use editor_core::{
    CompletionKey, CompletionSlot, Core, CoreEvent, HostEvent, HostId, HostInstance,
    client::CallError, delivery::Delivery,
};
use editor_sources::ContentText;
use editor_ui::markdown::terminal::TerminalScreen;
use std::{num::NonZeroUsize, ops::Range, sync::Arc};

struct Completion(CompletionKey, Result<Item, CallError>);

pub struct TerminalDialog {
    pub(crate) job: Option<Job>,
    error: Option<String>,
    pub(crate) following: bool,
    generation: u64,
    fetch: CompletionSlot,
    delivery: Delivery<Completion>,
}

impl std::fmt::Debug for TerminalDialog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalDialog").finish_non_exhaustive()
    }
}

pub struct Job {
    pub(crate) host: HostId,
    pub(crate) instance: HostInstance,
    thread: ThreadId,
    pub(crate) item: ItemId,
    pub(crate) data: Option<Arc<Item>>,
    pub(crate) command: String,
    pub(crate) commands: Vec<Range<usize>>,
    pub(crate) cwd: Option<String>,
    pub(crate) status: Status,
    pub(crate) screen: TerminalScreen,
    pub(crate) unverified: bool,
    pub(crate) command_source: Option<ContentText>,
    pub(crate) output_source: Option<ContentText>,
    stream: StreamState,
    source: Option<(Option<u64>, usize, u64)>,
}

impl std::fmt::Debug for Job {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Job").finish_non_exhaustive()
    }
}

impl Job {
    fn missing(&self) -> bool {
        self.unverified || self.screen.cleared_earlier() || self.screen.truncated_line_count() > 0
    }

    pub(crate) fn copy_output(&self) -> String {
        let mut text = self.screen.plain_text();
        if self.unverified {
            text.push_str("\n[Live output only; Forge supplied no final snapshot.]");
        }
        text
    }
}

impl Default for TerminalDialog {
    fn default() -> Self {
        Self {
            job: None,
            error: None,
            following: true,
            generation: 0,
            fetch: CompletionSlot::default(),
            delivery: Delivery::new(NonZeroUsize::MIN),
        }
    }
}

impl TerminalDialog {
    pub const fn job(&self) -> Option<&Job> {
        self.job.as_ref()
    }

    pub fn wake_mailbox(&self) -> Option<&aqua::mailbox::Mailbox<()>> {
        self.delivery.wake_mailbox()
    }

    pub fn open(&mut self, core: &Core, open: Option<&(HostId, ThreadId)>, item: &ItemId) -> bool {
        let Some((host, thread)) = open else {
            return false;
        };
        let Some(host_data) = core.host(host) else {
            return false;
        };
        self.generation = self.generation.wrapping_add(1);
        self.job = Some(Job {
            host: host.clone(),
            instance: host_data.instance(),
            thread: thread.clone(),
            item: item.clone(),
            data: None,
            command: String::new(),
            commands: Vec::new(),
            cwd: None,
            status: Status::Running,
            screen: TerminalScreen::new(),
            unverified: false,
            command_source: None,
            output_source: None,
            stream: StreamState::default(),
            source: None,
        });
        self.error = None;
        self.following = true;
        self.fetch(core);
        true
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    pub(crate) fn close(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.fetch.cancel();
        self.job = None;
        self.error = None;
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub const fn loading(&self) -> bool {
        self.fetch.is_running()
    }

    pub(crate) fn follow(&mut self, open: Option<&(HostId, ThreadId)>) {
        if self
            .job
            .as_ref()
            .is_some_and(|job| open != Some(&(job.host.clone(), job.thread.clone())))
        {
            self.close();
        }
    }

    pub fn fetch(&mut self, core: &Core) {
        let Some(job) = &self.job else {
            return;
        };
        let Some(host) = core.host_if_instance(&job.host, job.instance) else {
            return;
        };
        let client = host.client.clone();
        let item = job.item.clone();
        let sender = self.delivery.sender();
        self.error = None;
        self.fetch
            .spawn(&job.host, job.instance, move |key| async move {
                let result = client.item(&item).await;
                sender.offer(Completion(key, result)).await;
            });
    }

    pub fn drain(&mut self, core: &Core) -> bool {
        if self
            .job
            .as_ref()
            .is_some_and(|job| core.host_if_instance(&job.host, job.instance).is_none())
        {
            self.close();
            self.delivery.drain();
            return true;
        }
        let mut changed = false;
        for Completion(key, result) in self.delivery.drain() {
            if !self.fetch.finish(core, &key) {
                continue;
            }
            match result {
                Ok(item) => {
                    self.set_item(Arc::new(item));
                    self.sync(core);
                }
                Err(error) => {
                    self.error = Some(format!(
                        "Couldn't load shell output: {}",
                        diagnostics::chain(&error)
                    ));
                }
            }
            changed = true;
        }
        changed
    }

    pub(crate) fn on_core_event(&mut self, core: &Core, event: &CoreEvent) -> bool {
        let Some(job) = &self.job else {
            return false;
        };
        if core.host_if_instance(&job.host, job.instance).is_none() {
            self.close();
            return true;
        }
        let CoreEvent::Host { host, event } = event else {
            return false;
        };
        if host != &job.host {
            return false;
        }
        match event {
            HostEvent::Reset => {
                self.fetch(core);
                self.sync(core);
                true
            }
            HostEvent::Item(item) if item.id == job.item && item.thread_id == job.thread => {
                self.set_item(Arc::clone(item));
                self.sync(core);
                true
            }
            HostEvent::Live {
                thread_id,
                target: LiveTarget::Shell { item_id },
            } if thread_id == &job.thread && item_id == &job.item => {
                self.sync(core);
                true
            }
            HostEvent::ImagesChanged
            | HostEvent::EngineInstall { .. }
            | HostEvent::ThreadSummaryChanged { .. }
            | HostEvent::Changed
            | HostEvent::WslDistro { .. }
            | HostEvent::ThreadChanged { .. }
            | HostEvent::Item(_)
            | HostEvent::Live { .. }
            | HostEvent::EditorCall { .. } => false,
        }
    }

    fn set_item(&mut self, item: Arc<Item>) {
        let Some(job) = &mut self.job else {
            return;
        };
        if accepts_item(&item, &job.item, &job.thread, job.data.as_deref()) {
            job.data = Some(item);
        }
    }

    fn sync(&mut self, core: &Core) {
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
            return;
        };
        let Some(host) = core.host_if_instance(&job.host, job.instance) else {
            self.close();
            return;
        };
        let live = (!item.status.is_terminal())
            .then(|| {
                host.live_snapshot(
                    &job.thread,
                    &LiveTarget::Shell {
                        item_id: job.item.clone(),
                    },
                )
            })
            .flatten();
        let (epoch, length) = live.as_ref().map_or((None, 0), |stream| {
            (Some(stream.epoch), stream.text.len_bytes())
        });
        let source = (epoch, length, item.seq);
        if job.source == Some(source) {
            return;
        }
        job.source = Some(source);
        match command {
            TextContent::Inline(command) => {
                job.command_source = None;
                if command != &job.command {
                    job.command.clone_from(command);
                    job.commands = command_lines(command);
                }
            }
            TextContent::Stored(source) => {
                job.command.clear();
                job.commands.clear();
                if job
                    .command_source
                    .as_ref()
                    .is_none_or(|view| view.window().source() != source)
                {
                    job.command_source = Some(ContentText::new(host, source.clone()));
                }
            }
        }
        job.cwd.clone_from(cwd);
        job.status = item.status;
        if let Some(live) = live {
            job.output_source = None;
            let feed = job.stream.update(live.text.len_bytes(), Some(live.epoch));
            if feed.reset {
                job.screen = TerminalScreen::new();
            }
            for chunk in live.text.byte_slice(feed.bytes).chunks() {
                job.screen.feed(chunk.as_bytes());
            }
            job.unverified = false;
        } else if let Some(artisan_api::Output {
            text: TextContent::Stored(source),
            ..
        }) = output
        {
            if job
                .output_source
                .as_ref()
                .is_none_or(|view| view.window().source() != source)
            {
                job.output_source = Some(ContentText::new(host, source.clone()));
            }
            job.unverified = false;
        } else if let Some(text) = shell_output(output.as_ref()) {
            job.output_source = None;
            let feed = job.stream.update(text.len(), None);
            if feed.reset {
                job.screen = TerminalScreen::new();
            }
            job.screen.feed(&text.as_bytes()[feed.bytes]);
            job.unverified = false;
        } else if item.status.is_terminal() && job.stream.fed() > 0 {
            job.unverified = true;
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub commands: usize,
    pub lead: usize,
    pub lines: usize,
    pub truncated: u64,
    pub cleared: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Row {
    Command(usize),
    Missing,
    Output(usize),
    Status,
}

impl Layout {
    pub(crate) fn of(job: &Job) -> Self {
        Self {
            commands: job.commands.len(),
            lead: job.commands.len() + usize::from(job.missing()),
            lines: job.screen.line_count(),
            truncated: job.screen.truncated_line_count(),
            cleared: job.screen.cleared_line_count(),
        }
    }

    pub(crate) const fn total(self) -> usize {
        self.lead + self.lines + 1
    }

    const fn output(self) -> Range<usize> {
        self.lead..self.lead + self.lines
    }

    pub(crate) fn row(self, index: usize) -> Option<Row> {
        if index < self.commands {
            Some(Row::Command(index))
        } else if index < self.lead {
            Some(Row::Missing)
        } else if self.output().contains(&index) {
            Some(Row::Output(index - self.lead))
        } else if index == self.output().end {
            Some(Row::Status)
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListChange {
    Reset(usize),
    Splice { range: Range<usize>, count: usize },
}

pub fn list_changes(
    shown: Layout,
    next: Layout,
    item_count: usize,
    dirty: &[usize],
) -> Vec<ListChange> {
    let dropped =
        usize::try_from(next.truncated.saturating_sub(shown.truncated)).unwrap_or(usize::MAX);
    if next.lead != shown.lead
        || next.cleared != shown.cleared
        || next.truncated < shown.truncated
        || dropped > shown.lines
        || next.lines < shown.lines.saturating_sub(dropped)
        || shown.total() != item_count
    {
        return vec![ListChange::Reset(next.total())];
    }
    let mut changes = vec![ListChange::Splice {
        range: shown.output().end..shown.total(),
        count: 0,
    }];
    if dropped > 0 {
        changes.push(ListChange::Splice {
            range: next.lead..next.lead + dropped,
            count: 0,
        });
    }
    let kept = shown.lines - dropped;
    changes.extend(
        dirty
            .iter()
            .filter(|line| **line < kept)
            .map(|line| ListChange::Splice {
                range: next.lead + line..next.lead + line + 1,
                count: 1,
            }),
    );
    changes.push(ListChange::Splice {
        range: next.lead + kept..next.lead + kept,
        count: next.lines - kept + 1,
    });
    changes
}
