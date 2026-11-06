pub(crate) mod annotated;
pub(crate) mod code_ref;
pub(crate) mod file_tree;

fn code_lines(text: &str) -> Vec<String> {
    let text = text.strip_suffix('\n').unwrap_or(text);
    if text.is_empty() {
        return Vec::new();
    }
    text.split('\n')
        .map(|line| line.trim_end_matches('\r').replace('\t', "    "))
        .collect()
}

fn excerpt_width(lines: &[String]) -> f32 {
    let columns = lines
        .iter()
        .map(|line| unicode_width::UnicodeWidthStr::width(line.as_str()))
        .max()
        .unwrap_or(0);
    super::charts::scale::f32_of_count(columns).mul_add(7.8, 12.0)
}
pub(crate) mod card;
