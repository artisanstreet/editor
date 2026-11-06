use std::ops::Range;

pub fn raw_rows(text: &str) -> Vec<Range<usize>> {
    let mut rows = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut cap = (start + 1024).min(text.len());
        while !text.is_char_boundary(cap) {
            cap -= 1;
        }
        let end = text[start..cap]
            .find('\n')
            .map_or(cap, |offset| start + offset + 1);
        rows.push(start..end);
        start = end;
    }
    rows
}
