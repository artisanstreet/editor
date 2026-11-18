use std::fmt::Display;
use std::io::IsTerminal;

pub fn interactive() -> bool {
    std::io::stdin().is_terminal() && std::io::stderr().is_terminal()
}

#[derive(Debug, thiserror::Error)]
pub enum AskError {
    #[error("cancelled")]
    Cancelled,
    #[error("read the answer")]
    Read(#[source] std::io::Error),
    #[error("no terminal to ask {question}; pass {flag}")]
    NoTerminal { question: String, flag: String },
}

pub fn prompt_error(error: std::io::Error) -> AskError {
    if error.kind() == std::io::ErrorKind::Interrupted {
        cliclack::outro_cancel("Cancelled").ok();
        AskError::Cancelled
    } else {
        AskError::Read(error)
    }
}

pub fn needs_flag(question: &str, flag: &str) -> AskError {
    AskError::NoTerminal {
        question: question.to_owned(),
        flag: flag.to_owned(),
    }
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

pub fn note(title: impl Display, body: impl Display) {
    if interactive() {
        cliclack::note(title, body).ok();
    } else {
        eprintln!("{title}\n{body}");
    }
}

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

    pub fn clear(self) {
        if let Some(bar) = self.bar {
            bar.clear();
        }
    }
}

pub fn task<T, E: std::error::Error>(
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
