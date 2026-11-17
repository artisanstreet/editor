use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex, PoisonError};
use tracing_subscriber::fmt::MakeWriter;

const LINES: usize = 200;
const LINE_BYTES: usize = 2048;

#[derive(Clone, Debug, Default)]
pub struct RecentLines {
    lines: Arc<Mutex<VecDeque<String>>>,
}

impl RecentLines {
    pub fn snapshot(&self) -> Vec<String> {
        self.lines
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }

    fn push(&self, bytes: &[u8]) {
        let text = String::from_utf8_lossy(bytes);
        let mut lines = self.lines.lock().unwrap_or_else(PoisonError::into_inner);
        for line in text.lines().filter(|line| !line.is_empty()) {
            if lines.len() == LINES {
                lines.pop_front();
            }
            lines.push_back(bounded(line));
        }
    }
}

fn bounded(line: &str) -> String {
    if line.len() <= LINE_BYTES {
        return line.to_owned();
    }
    let (kept, _) = line.split_at(line.floor_char_boundary(LINE_BYTES));
    format!("{kept}… ({} bytes)", line.len())
}

#[derive(Debug)]
pub struct RecentWriter {
    recent: RecentLines,
}

impl io::Write for RecentWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.recent.push(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for RecentLines {
    type Writer = RecentWriter;

    fn make_writer(&'a self) -> Self::Writer {
        RecentWriter {
            recent: self.clone(),
        }
    }
}
