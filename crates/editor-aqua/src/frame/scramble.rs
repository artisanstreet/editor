pub const GLYPHS: [char; 14] = [
    'A', 'C', 'D', 'E', 'G', 'L', 'N', 'O', 'P', 'R', 'S', 'T', 'U', 'Z',
];
pub const STAGGER_MS: f64 = 18.0;
pub const SETTLE_MS: f64 = 170.0;
pub const TICK_MS: f64 = 34.0;

pub type Row = Vec<Option<char>>;
pub type Rows = Vec<Row>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cell {
    pub from: Option<char>,
    pub to: Option<char>,
}

#[must_use]
pub fn rows(lines: &[&str]) -> Rows {
    lines
        .iter()
        .map(|line| line.chars().map(Some).collect())
        .collect()
}

#[must_use]
pub fn squeeze(rows: &Rows) -> Rows {
    rows.iter()
        .map(|row| match row.as_slice() {
            [first, .., last] if row.len() > 2 => vec![*first, *last],
            _ => row.clone(),
        })
        .collect()
}

#[must_use]
pub fn cells_between(shown: &Rows, target: &[&str]) -> Vec<Vec<Cell>> {
    shown
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let next: Vec<char> = target
                .get(index)
                .map_or_else(Vec::new, |line| line.chars().collect());
            (0..row.len().max(next.len()))
                .map(|column| Cell {
                    from: row.get(column).copied().flatten(),
                    to: next.get(column).copied(),
                })
                .collect()
        })
        .collect()
}

#[must_use]
pub fn glyph_at(index: usize, tick: u32) -> char {
    let position = u32::try_from(index + 1).unwrap_or(u32::MAX);
    let mixed = position
        .wrapping_mul(2_654_435_761)
        .wrapping_add(tick.wrapping_add(1).wrapping_mul(40_503));
    GLYPHS[mixed as usize % GLYPHS.len()]
}

#[must_use]
pub fn cell_char(cell: Cell, index: usize, elapsed_ms: f64) -> Option<char> {
    let local = (index as f64).mul_add(-STAGGER_MS, elapsed_ms);
    match local {
        local if local < 0.0 => cell.from,
        local if local >= SETTLE_MS => cell.to,
        local => Some(glyph_at(index, (local / TICK_MS).floor() as u32)),
    }
}

#[must_use]
pub fn total_ms(cells: &[Vec<Cell>]) -> f64 {
    let count = cells.iter().map(Vec::len).sum::<usize>();
    (count.saturating_sub(1) as f64).mul_add(STAGGER_MS, SETTLE_MS)
}

#[must_use]
pub fn frame_at(cells: &[Vec<Cell>], elapsed_ms: f64) -> Rows {
    cells
        .iter()
        .scan(0_usize, |offset, row| {
            let start = *offset;
            *offset += row.len();
            Some(
                row.iter()
                    .enumerate()
                    .map(|(column, cell)| cell_char(*cell, start + column, elapsed_ms))
                    .collect(),
            )
        })
        .collect()
}
