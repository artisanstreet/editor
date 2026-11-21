mod view;

use artisan_api::{Item, ItemId, Output, TextContent, ThreadId};
use std::ops::Range;
pub use view::TerminalAction;
pub(crate) use view::terminal_view;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Feed {
    pub reset: bool,
    pub bytes: Range<usize>,
}

#[derive(Clone, Debug, Default)]
pub struct StreamState {
    fed: usize,
    epoch: Option<u64>,
}

impl StreamState {
    pub fn update(&mut self, length: usize, epoch: Option<u64>) -> Feed {
        let reset = self.epoch != epoch || length < self.fed || epoch.is_none();
        if reset {
            self.fed = 0;
            self.epoch = epoch;
        }
        let bytes = self.fed..length;
        self.fed = length;
        Feed { reset, bytes }
    }

    pub(crate) const fn fed(&self) -> usize {
        self.fed
    }
}

pub(crate) fn accepts_item(
    item: &Item,
    id: &ItemId,
    thread: &ThreadId,
    previous: Option<&Item>,
) -> bool {
    &item.id == id
        && &item.thread_id == thread
        && previous.is_none_or(|known| known.seq <= item.seq)
}

pub(crate) fn shell_output(output: Option<&Output>) -> Option<&str> {
    match output {
        Some(Output {
            text: TextContent::Inline(text),
            ..
        }) => Some(text),
        Some(Output {
            text: TextContent::Stored(_),
            ..
        })
        | None => None,
    }
}

pub fn command_lines(command: &str) -> Vec<Range<usize>> {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShellPrompt {
    pub prompt: String,
    pub continuation: &'static str,
    pub coloured: bool,
}

pub fn shell_prompt(directory: Option<&str>) -> ShellPrompt {
    match directory.map(str::trim).filter(|path| !path.is_empty()) {
        Some(path) if is_windows_path(path) => ShellPrompt {
            prompt: format!("PS {}> ", path.trim_end_matches(['\\', '/'])),
            continuation: ">> ",
            coloured: false,
        },
        Some(path) => ShellPrompt {
            prompt: format!("{}$ ", home_relative(path)),
            continuation: "> ",
            coloured: true,
        },
        None => ShellPrompt {
            prompt: "$ ".to_owned(),
            continuation: "> ",
            coloured: true,
        },
    }
}

fn is_windows_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with("\\\\")
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
}

fn home_relative(path: &str) -> String {
    let path = if path.len() > 1 {
        path.trim_end_matches('/')
    } else {
        path
    };
    let below_home = path
        .strip_prefix("/root")
        .filter(|rest| rest.is_empty() || rest.starts_with('/'))
        .or_else(|| {
            ["/home/", "/Users/"].iter().find_map(|prefix| {
                let user = path.strip_prefix(prefix)?;
                let rest = user.find('/').map_or("", |at| user.split_at(at).1);
                (rest.len() < user.len()).then_some(rest)
            })
        });
    below_home.map_or_else(|| path.to_owned(), |rest| format!("~{rest}"))
}
