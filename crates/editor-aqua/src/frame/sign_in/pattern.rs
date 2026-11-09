pub const TILE: (f32, f32) = (84.0, 48.0);
pub const CELL: (f32, f32) = (12.0, 6.0);
pub const COVERAGE: f32 = 0.46;

const CELLS: [(f32, f32); 26] = [
    (0.0, 0.0),
    (28.0, 8.0),
    (42.0, 0.0),
    (56.0, 0.0),
    (56.0, 8.0),
    (42.0, 8.0),
    (42.0, 24.0),
    (56.0, 16.0),
    (70.0, 16.0),
    (70.0, 0.0),
    (28.0, 32.0),
    (14.0, 16.0),
    (0.0, 24.0),
    (0.0, 32.0),
    (14.0, 32.0),
    (28.0, 40.0),
    (14.0, 40.0),
    (42.0, 40.0),
    (56.0, 32.0),
    (56.0, 24.0),
    (70.0, 32.0),
    (70.0, 40.0),
    (14.0, 24.0),
    (28.0, 16.0),
    (14.0, 8.0),
    (0.0, 8.0),
];

const RIDGE: [(f32, f32); 10] = [
    (0.0, 0.12),
    (0.08, 0.04),
    (0.18, 0.0),
    (0.30, 0.08),
    (0.42, 0.26),
    (0.54, 0.40),
    (0.66, 0.36),
    (0.78, 0.46),
    (0.90, 0.58),
    (1.0, 0.52),
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell {
    pub x: f32,
    pub y: f32,
    pub strength: f32,
}

#[must_use]
pub fn cells(width: f32, height: f32) -> Vec<Cell> {
    if width <= 0.0 || height <= 0.0 {
        return Vec::new();
    }
    let rows = (height / TILE.1).ceil() as u32;
    let columns = (width / TILE.0).ceil() as u32;
    (0..rows)
        .flat_map(|row| (0..columns).map(move |column| (column, row)))
        .flat_map(|(column, row)| {
            CELLS.iter().map(move |(x, y)| {
                (column as f32 * TILE.0 + x, row as f32 * TILE.1 + y)
            })
        })
        .filter_map(|(x, y)| {
            let center = ((x + CELL.0 / 2.0) / width, (y + CELL.1 / 2.0) / height);
            let inside = center.0 < 1.0 && center.1 < 1.0 && center.1 >= ridge(center.0);
            let strength = fade_right(center.0) * fade_up(center.1);
            (inside && strength > 0.0).then_some(Cell { x, y, strength })
        })
        .collect()
}

fn ridge(x: f32) -> f32 {
    RIDGE
        .windows(2)
        .find(|pair| x >= pair[0].0 && x <= pair[1].0)
        .map_or(1.0, |pair| {
            let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
            (y1 - y0).mul_add((x - x0) / (x1 - x0), y0)
        })
}

fn fade_right(x: f32) -> f32 {
    match x <= 0.3 {
        true => 1.0,
        false => (1.0 - (x - 0.3) / 0.7).max(0.0),
    }
}

fn fade_up(y: f32) -> f32 {
    let rise = 1.0 - y;
    match rise <= 0.45 {
        true => 1.0,
        false => (1.0 - (rise - 0.45) / 0.55).max(0.0),
    }
}
