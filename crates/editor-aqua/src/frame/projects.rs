#[must_use]
pub fn title_of(title: Option<&str>) -> String {
    title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or("New thread")
        .to_owned()
}
