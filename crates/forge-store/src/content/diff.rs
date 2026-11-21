use artisan_api::EditTotals;

const LONGEST_MARKER: usize = b"diff --git ".len();

#[derive(Default)]
pub(crate) struct DiffScan {
    totals: EditTotals,
    in_hunk: bool,
    head: [u8; LONGEST_MARKER],
    head_len: usize,
}

impl DiffScan {
    pub(crate) fn feed(&mut self, bytes: &[u8]) {
        let mut rest = bytes;
        while !rest.is_empty() {
            let (line, ended) = match memchr::memchr(b'\n', rest) {
                Some(end) => (&rest[..end], Some(end + 1)),
                None => (rest, None),
            };
            let room = LONGEST_MARKER - self.head_len;
            let taken = line.len().min(room);
            self.head[self.head_len..self.head_len + taken].copy_from_slice(&line[..taken]);
            self.head_len += taken;
            match ended {
                Some(next) => {
                    self.end_line();
                    rest = &rest[next..];
                }
                None => rest = &[],
            }
        }
    }

    pub(crate) fn finish(mut self) -> EditTotals {
        self.end_line();
        self.totals
    }

    fn end_line(&mut self) {
        let head = &self.head[..self.head_len];
        let file_header = !self.in_hunk && (head.starts_with(b"+++") || head.starts_with(b"---"));
        match head.first() {
            _ if head.starts_with(b"diff --git ") => self.in_hunk = false,
            _ if head.starts_with(b"@@") => self.in_hunk = true,
            _ if file_header => {}
            Some(b'+') => self.totals.added = self.totals.added.saturating_add(1),
            Some(b'-') => self.totals.removed = self.totals.removed.saturating_add(1),
            _ => {}
        }
        self.head_len = 0;
    }
}

pub(crate) fn totals(diff: &str) -> EditTotals {
    let mut scan = DiffScan::default();
    scan.feed(diff.as_bytes());
    scan.finish()
}

#[cfg(test)]
#[path = "../../tests/content/diff.rs"]
mod tests;
