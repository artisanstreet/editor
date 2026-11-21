use std::fmt::Display;
use std::io::{IsTerminal, Write as _};

pub(crate) fn print(text: impl Display) {
    let _stdout_closed = writeln!(std::io::stdout().lock(), "{text}");
}

pub(crate) fn status(text: impl Display) {
    let _stderr_closed = writeln!(std::io::stderr().lock(), "{text}");
}

pub(crate) fn count(amount: u64, one: &str, many: &str) -> String {
    let noun = if amount == 1 { one } else { many };
    format!("{amount} {noun}")
}

pub(crate) fn interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum AskError {
    #[error("cancelled")]
    Cancelled,
    #[error("read the answer")]
    Read(#[source] std::io::Error),
    #[error("no terminal to ask {question}; pass {flag}")]
    NoTerminal { question: String, flag: String },
}

pub(crate) fn prompt_error(error: std::io::Error) -> AskError {
    if error.kind() == std::io::ErrorKind::Interrupted {
        let _terminal_gone = cliclack::outro_cancel("Cancelled");
        AskError::Cancelled
    } else {
        AskError::Read(error)
    }
}

pub(crate) fn needs_flag(question: &str, flag: &str) -> AskError {
    AskError::NoTerminal {
        question: question.to_owned(),
        flag: flag.to_owned(),
    }
}

pub(crate) fn intro(title: impl Display) {
    if interactive() {
        let _terminal_gone = cliclack::intro(title);
    }
}

pub(crate) fn outro(message: impl Display) {
    if interactive() {
        let _terminal_gone = cliclack::outro(message);
    } else {
        status(message);
    }
}

pub(crate) fn step(message: impl Display) {
    if interactive() {
        let _terminal_gone = cliclack::log::step(message);
    } else {
        status(message);
    }
}

pub(crate) fn success(message: impl Display) {
    if interactive() {
        let _terminal_gone = cliclack::log::success(message);
    } else {
        status(message);
    }
}

pub(crate) fn info(message: impl Display) {
    if interactive() {
        let _terminal_gone = cliclack::log::info(message);
    } else {
        status(message);
    }
}

pub(crate) fn warning(message: impl Display) {
    if interactive() {
        let _terminal_gone = cliclack::log::warning(message);
    } else {
        status(format_args!("warning: {message}"));
    }
}

pub(crate) fn note(title: impl Display, body: impl Display) {
    if interactive() {
        let _terminal_gone = cliclack::note(title, body);
    } else {
        status(format_args!("{title}\n{body}"));
    }
}

pub(crate) struct Progress {
    bar: Option<cliclack::ProgressBar>,
}

impl Progress {
    pub(crate) fn spinner(message: impl Display) -> Self {
        if !interactive() {
            status(message);
            return Self { bar: None };
        }
        let bar = cliclack::spinner();
        bar.start(message);
        Self { bar: Some(bar) }
    }

    pub(crate) fn download(message: impl Display, length: u64) -> Self {
        if !interactive() {
            status(message);
            return Self { bar: None };
        }
        let bar = cliclack::progress_bar(length).with_download_template();
        bar.start(message);
        Self { bar: Some(bar) }
    }

    pub(crate) fn advance(&self, bytes: u64) {
        if let Some(bar) = &self.bar {
            bar.inc(bytes);
        }
    }

    pub(crate) fn message(&self, message: impl Display) {
        if let Some(bar) = &self.bar {
            bar.set_message(message);
        }
    }

    pub(crate) fn done(self, message: impl Display) {
        match self.bar {
            Some(bar) => bar.stop(message),
            None => status(message),
        }
    }

    pub(crate) fn fail(self, message: impl Display) {
        match self.bar {
            Some(bar) => bar.error(message),
            None => status(format_args!("error: {message}")),
        }
    }

    pub(crate) fn clear(self) {
        if let Some(bar) = self.bar {
            bar.clear();
        }
    }
}

pub(crate) fn task<T, E: std::error::Error>(
    message: impl Display,
    done: impl Display,
    work: impl FnOnce() -> Result<T, E>,
) -> Result<T, E> {
    let progress = Progress::spinner(message);
    match work() {
        Ok(value) => {
            progress.done(done);
            Ok(value)
        }
        Err(error) => {
            progress.fail(diagnostics::chain(&error));
            Err(error)
        }
    }
}

pub(crate) fn table(header: &[&str], rows: &[Vec<String>]) -> String {
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
