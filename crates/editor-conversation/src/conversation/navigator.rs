pub fn tick_indices(count: usize, slots: usize, active: Option<usize>) -> Vec<usize> {
    let slots = slots.max(1).min(count);
    if count == 0 {
        return Vec::new();
    }
    if count <= slots {
        return (0..count).collect();
    }
    let mut indices: Vec<usize> = (0..slots)
        .map(|slot| {
            if slots == 1 {
                0
            } else {
                slot * (count - 1) / (slots - 1)
            }
        })
        .collect();
    if let Some(active) = active.filter(|active| *active < count)
        && !indices.contains(&active)
        && let Some((nearest, _)) = indices
            .iter()
            .enumerate()
            .min_by_key(|(_, index)| index.abs_diff(active))
    {
        indices[nearest] = active;
        indices.sort_unstable();
    }
    indices
}
