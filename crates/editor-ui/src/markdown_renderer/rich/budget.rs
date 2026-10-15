//! Cache charges use owned container capacities and retained string payloads.
//! SharedString's private inline/static representation is conservatively
//! charged by payload length; no dependency on its private allocation layout.
pub(in crate::markdown_renderer) fn vector<T>(values: &Vec<T>) -> usize {
    values.capacity() * std::mem::size_of::<T>()
}
pub(in crate::markdown_renderer) fn optional(value: &Option<String>) -> usize {
    value.as_ref().map_or(0, String::capacity)
}
pub(in crate::markdown_renderer) fn strings(values: &Vec<String>) -> usize {
    vector(values) + values.iter().map(String::capacity).sum::<usize>()
}
pub(in crate::markdown_renderer) fn shared(values: &[gpui::SharedString]) -> usize {
    std::mem::size_of_val(values) + values.iter().map(|value| value.len()).sum::<usize>()
}
