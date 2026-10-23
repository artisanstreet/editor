//! How `ae` talks to people and to agents. In a terminal: prompts, spinners
//! and progress bars (cliclack, on stderr). Without one: plain lines on
//! stderr and no questions; a choice that has no flag value fails at once
//! with the flag to pass. Data (tables, JSON) always goes to stdout.

use std::fmt::Display;
use std::io::IsTerminal;

/// Whether someone can answer questions and watch progress.
pub fn interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

/// A prompt's error: Ctrl+C or Esc becomes "cancelled".
pub fn prompt_error(error: std::io::Error) -> anyhow::Error {
    if error.kind() == std::io::ErrorKind::Interrupted {
        cliclack::outro_cancel("Cancelled").ok();
        anyhow::anyhow!("cancelled")
    } else {
        anyhow::Error::new(error).context("read the answer")
    }
}

/// What to say when a choice can't be asked.
pub fn needs_flag(question: &str, flag: &str) -> anyhow::Error {
    anyhow::anyhow!("no terminal to ask {question}; pass {flag}")
}

pub fn intro(title: impl Display) {
    if interactive() {
        cliclack::intro(title).ok();
    }
}

pub fn outro(message: impl Display) {
    if interactive() {
        cliclack::outro(message).ok();
    } else {
        eprintln!("{message}");
    }
}

pub fn step(message: impl Display) {
    if interactive() {
        cliclack::log::step(message).ok();
    } else {
        eprintln!("{message}");
    }
}

pub fn success(message: impl Display) {
    if interactive() {
        cliclack::log::success(message).ok();
    } else {
        eprintln!("{message}");
    }
}

pub fn info(message: impl Display) {
    if interactive() {
        cliclack::log::info(message).ok();
    } else {
        eprintln!("{message}");
    }
}

pub fn warning(message: impl Display) {
    if interactive() {
        cliclack::log::warning(message).ok();
    } else {
        eprintln!("warning: {message}");
    }
}

/// A titled block: the install summary, a pairing code.
pub fn note(title: impl Display, body: impl Display) {
    if interactive() {
        cliclack::note(title, body).ok();
    } else {
        eprintln!("{title}\n{body}");
    }
}

/// A spinner, or a progress bar once its length is known. Without a
/// terminal it prints where it starts and how it ends.
pub struct Progress {
    bar: Option<cliclack::ProgressBar>,
}

impl Progress {
    pub fn spinner(message: impl Display) -> Self {
        if !interactive() {
            eprintln!("{message}");
            return Self { bar: None };
        }
        let bar = cliclack::spinner();
        bar.start(message);
        Self { bar: Some(bar) }
    }

    /// A byte progress bar.
    pub fn download(message: impl Display, length: u64) -> Self {
        if !interactive() {
            eprintln!("{message}");
            return Self { bar: None };
        }
        let bar = cliclack::progress_bar(length).with_download_template();
        bar.start(message);
        Self { bar: Some(bar) }
    }

    pub fn advance(&self, bytes: u64) {
        if let Some(bar) = &self.bar {
            bar.inc(bytes);
        }
    }

    pub fn message(&self, message: impl Display) {
        if let Some(bar) = &self.bar {
            bar.set_message(message);
        }
    }

    pub fn done(self, message: impl Display) {
        match self.bar {
            Some(bar) => bar.stop(message),
            None => eprintln!("{message}"),
        }
    }

    pub fn fail(self, message: impl Display) {
        match self.bar {
            Some(bar) => bar.error(message),
            None => eprintln!("error: {message}"),
        }
    }

    /// Clears it, for a failure reported elsewhere.
    pub fn clear(self) {
        if let Some(bar) = self.bar {
            bar.clear();
        }
    }
}

/// Runs `work` under a spinner that ends with its outcome.
pub fn task<T>(
    message: impl Display,
    done: impl Display,
    work: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let progress = Progress::spinner(message);
    match work() {
        Ok(value) => {
            progress.done(done);
            Ok(value)
        }
        Err(error) => {
            progress.fail(format!("{error:#}"));
            Err(error)
        }
    }
}

/// Fixed-width columns for stdout tables.
pub fn table(header: &[&str], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = header.iter().map(|cell| cell.chars().count()).collect();
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            if let Some(width) = widths.get_mut(index) {
                *width = (*width).max(cell.chars().count());
            }
        }
    }
    let line = |cells: Vec<String>| {
        let mut text = String::new();
        for (index, cell) in cells.iter().enumerate() {
            if index + 1 == cells.len() {
                text.push_str(cell);
            } else {
                let pad = widths[index] - cell.chars().count();
                text.push_str(cell);
                text.push_str(&" ".repeat(pad + 2));
            }
        }
        text.trim_end().to_owned()
    };
    let mut lines = vec![line(header.iter().map(|cell| (*cell).to_owned()).collect())];
    lines.extend(rows.iter().map(|row| line(row.clone())));
    lines.join("\n")
}
