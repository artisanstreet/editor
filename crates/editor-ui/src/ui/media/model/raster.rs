use std::sync::LazyLock;

use super::mesh::{Mesh, Vec3};

const FIELD_OF_VIEW: f32 = 30.0;

const FIT_MARGIN: f32 = 1.06;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub pan: (f32, f32),
}

impl Camera {
    pub const DEFAULT_YAW: f32 = 30.0;
    pub const DEFAULT_PITCH: f32 = 18.0;
    pub const NEAREST: f32 = 1.3;
    pub const FARTHEST: f32 = 16.0;

    pub fn fitted() -> f32 {
        FIT_MARGIN / (FIELD_OF_VIEW.to_radians() * 0.5).sin()
    }

    pub fn new(yaw: f32, pitch: f32) -> Self {
        Self {
            yaw,
            pitch: pitch.clamp(-89.0, 89.0),
            distance: Self::fitted(),
            pan: (0.0, 0.0),
        }
    }

    pub fn focal(width: f32, height: f32) -> f32 {
        0.5 * width.min(height) / (FIELD_OF_VIEW.to_radians() * 0.5).tan()
    }

    pub fn view_matrix(&self) -> [[f32; 4]; 4] {
        let r = self.rotation();
        [
            [r[0][0], r[1][0], r[2][0], 0.0],
            [r[0][1], r[1][1], r[2][1], 0.0],
            [r[0][2], r[1][2], r[2][2], 0.0],
            [-self.pan.0, -self.pan.1, -self.distance, 1.0],
        ]
    }

    pub fn projection_matrix(width: f32, height: f32) -> [[f32; 4]; 4] {
        let focal = 1.0 / (FIELD_OF_VIEW.to_radians() * 0.5).tan();
        let short = width.min(height);
        let (near, far) = (NEAR, FAR);
        [
            [focal * short / width, 0.0, 0.0, 0.0],
            [0.0, focal * short / height, 0.0, 0.0],
            [0.0, 0.0, far / (near - far), -1.0],
            [0.0, 0.0, near * far / (near - far), 0.0],
        ]
    }

    fn rotation(&self) -> [[f32; 3]; 3] {
        let (sy, cy) = self.yaw.to_radians().sin_cos();
        let (sp, cp) = self.pitch.to_radians().sin_cos();
        [
            [cy, 0.0, sy],
            [sp * sy, cp, -sp * cy],
            [-cp * sy, sp, cp * cy],
        ]
    }
}

impl Default for Camera {
    fn default() -> Self {
        Self::new(Self::DEFAULT_YAW, Self::DEFAULT_PITCH)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub base: Vec3,
    pub edges: Option<(Vec3, f32)>,
}

pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

fn rotate(m: &[[f32; 3]; 3], v: Vec3) -> Vec3 {
    Vec3::new(
        m[0][2].mul_add(v.z, m[0][1].mul_add(v.y, m[0][0] * v.x)),
        m[1][2].mul_add(v.z, m[1][1].mul_add(v.y, m[1][0] * v.x)),
        m[2][2].mul_add(v.z, m[2][1].mul_add(v.y, m[2][0] * v.x)),
    )
}

#[derive(Clone, Copy, Default)]
struct Projected {
    x: f32,
    y: f32,
    inverse_depth: f32,
}

const NEAR: f32 = 0.05;

const FAR: f32 = 64.0;

const EMPTY: u32 = u32::MAX;

struct Screen {
    corners: [Projected; 3],
    area: f32,
}

impl Screen {
    fn new(corners: [Projected; 3]) -> Self {
        let [a, b, c] = corners;
        let area = (b.y - a.y).mul_add(-(c.x - a.x), (b.x - a.x) * (c.y - a.y));
        Self { corners, area }
    }

    fn edge(p: Projected, q: Projected, x: f32, y: f32) -> f32 {
        (q.y - p.y).mul_add(-(x - p.x), (q.x - p.x) * (y - p.y))
    }

    fn weights(&self, x: f32, y: f32) -> [f32; 3] {
        let [first, second, third] = self.corners;
        let opposite_first = Self::edge(second, third, x, y) / self.area;
        let opposite_second = Self::edge(third, first, x, y) / self.area;
        [
            opposite_first,
            opposite_second,
            1.0 - opposite_first - opposite_second,
        ]
    }

    fn perspective(&self, weights: [f32; 3]) -> [f32; 3] {
        let scaled = [0, 1, 2].map(|i| weights[i] * self.corners[i].inverse_depth);
        let total = scaled[0] + scaled[1] + scaled[2];
        scaled.map(|weight| weight / total)
    }

    fn edge_distance(&self, weights: [f32; 3]) -> f32 {
        let [a, b, c] = self.corners;
        let length = |p: Projected, q: Projected| (q.x - p.x).hypot(q.y - p.y).max(1e-6);
        let sides = [length(b, c), length(c, a), length(a, b)];
        (0..3)
            .map(|i| (weights[i] * self.area / sides[i]).abs())
            .fold(f32::MAX, f32::min)
    }
}

struct Projection {
    view: Vec<Vec3>,
    projected: Vec<Projected>,
    normals: Vec<Vec3>,
}

impl Projection {
    fn new(mesh: &Mesh, camera: &Camera, width: f32, height: f32) -> Self {
        let rotation = camera.rotation();
        let focal = Camera::focal(width, height);
        let view: Vec<Vec3> = mesh
            .positions
            .iter()
            .map(|point| {
                let turned = rotate(&rotation, *point);
                Vec3::new(turned.x - camera.pan.0, turned.y - camera.pan.1, turned.z)
            })
            .collect();
        let projected = view
            .iter()
            .map(|point| {
                let depth = camera.distance - point.z;
                if depth <= NEAR {
                    return Projected::default();
                }
                let inverse_depth = 1.0 / depth;
                Projected {
                    x: (focal * point.x).mul_add(inverse_depth, width * 0.5),
                    y: (focal * point.y).mul_add(-inverse_depth, height * 0.5),
                    inverse_depth,
                }
            })
            .collect();
        let normals = mesh
            .normals
            .iter()
            .map(|normal| rotate(&rotation, *normal))
            .collect();
        Self {
            view,
            projected,
            normals,
        }
    }

    fn screen(&self, triangle: usize) -> Screen {
        let first = triangle * 3;
        Screen::new([
            self.projected[first],
            self.projected[first + 1],
            self.projected[first + 2],
        ])
    }
}

#[derive(Clone, Copy)]
struct Grid {
    width: usize,
    height: usize,
}

pub fn render(
    mesh: &Mesh,
    camera: &Camera,
    look: &Look,
    width: u32,
    height: u32,
    samples: u32,
) -> Frame {
    let samples = samples.max(1);
    let grid = Grid {
        width: (width * samples) as usize,
        height: (height * samples) as usize,
    };
    #[expect(clippy::cast_precision_loss, reason = "frame sides are small")]
    let projection = Projection::new(mesh, camera, grid.width as f32, grid.height as f32);
    let winner = visibility(&projection, grid);
    let color = shade(mesh, &projection, &winner, look, camera, grid, samples);
    Frame {
        width,
        height,
        pixels: resolve(&winner, &color, grid, samples),
    }
}

#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "sample coordinates are small, non-negative, and clamped to the grid"
)]
fn visibility(projection: &Projection, grid: Grid) -> Vec<u32> {
    let mut depth = vec![0.0_f32; grid.width * grid.height];
    let mut winner = vec![EMPTY; grid.width * grid.height];
    let (right, bottom) = (grid.width as f32 - 1.0, grid.height as f32 - 1.0);
    for triangle in 0..projection.projected.len() / 3 {
        let screen = projection.screen(triangle);
        let corners = screen.corners;
        if corners.iter().any(|corner| corner.inverse_depth <= 0.0)
            || !screen.area.is_finite()
            || screen.area.abs() < 1e-9
        {
            continue;
        }
        let low = |axis: fn(&Projected) -> f32| {
            corners
                .iter()
                .map(axis)
                .fold(f32::MAX, f32::min)
                .floor()
                .max(0.0)
        };
        let high = |axis: fn(&Projected) -> f32, limit: f32| {
            corners
                .iter()
                .map(axis)
                .fold(f32::MIN, f32::max)
                .ceil()
                .min(limit)
        };
        let (min_x, max_x) = (low(|p| p.x), high(|p| p.x, right));
        let (min_y, max_y) = (low(|p| p.y), high(|p| p.y, bottom));
        if min_x > max_x || min_y > max_y {
            continue;
        }
        let [first, second, third] = corners;
        let step = [
            -(third.y - second.y) / screen.area,
            -(first.y - third.y) / screen.area,
        ];
        let id = triangle as u32;
        for y in (min_y as usize)..=(max_y as usize) {
            let [mut wa, mut wb, _] = screen.weights(min_x + 0.5, y as f32 + 0.5);
            let row = y * grid.width;
            for x in (min_x as usize)..=(max_x as usize) {
                let wc = 1.0 - wa - wb;
                if wa >= 0.0 && wb >= 0.0 && wc >= 0.0 {
                    let nearness = wb.mul_add(second.inverse_depth, wa * first.inverse_depth)
                        + wc * third.inverse_depth;
                    let at = row + x;
                    if nearness > depth[at] {
                        depth[at] = nearness;
                        winner[at] = id;
                    }
                }
                wa += step[0];
                wb += step[1];
            }
        }
    }
    winner
}

#[expect(clippy::cast_precision_loss, reason = "sample coordinates are small")]
fn shade(
    mesh: &Mesh,
    projection: &Projection,
    winner: &[u32],
    look: &Look,
    camera: &Camera,
    grid: Grid,
    samples: u32,
) -> Vec<Vec3> {
    let lights = Lights::new();
    let edge_half_width = 0.5 * samples as f32;
    let mut color = vec![Vec3::ZERO; winner.len()];
    for (at, (out, triangle)) in color.iter_mut().zip(winner).enumerate() {
        if *triangle == EMPTY {
            continue;
        }
        let triangle = *triangle as usize;
        let screen = projection.screen(triangle);
        let (x, y) = (at % grid.width, at / grid.width);
        let weights = screen.weights(x as f32 + 0.5, y as f32 + 0.5);
        let [qa, qb, qc] = screen.perspective(weights);
        let first = triangle * 3;
        let blend =
            |values: &[Vec3]| values[first] * qa + values[first + 1] * qb + values[first + 2] * qc;
        let point = blend(&projection.view);
        let eye = Vec3::new(-point.x, -point.y, camera.distance - point.z)
            .try_normalize()
            .unwrap_or(Vec3::new(0.0, 0.0, 1.0));
        let mut normal = blend(&projection.normals).try_normalize().unwrap_or(eye);
        if normal.dot(eye) < 0.0 {
            normal = -normal;
        }
        let albedo = mesh.colors.as_deref().map_or(look.base, blend);
        let mut shaded = lights.shade(normal, eye, albedo);
        if let Some((edge, opacity)) = look.edges {
            let distance = screen.edge_distance(weights);
            let cover = (edge_half_width + 0.5 - distance).clamp(0.0, 1.0) * opacity;
            shaded = shaded * (1.0 - cover) + edge * cover;
        }
        *out = shaded;
    }
    color
}

#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    reason = "sample counts are tiny and alpha is at most 255"
)]
fn resolve(winner: &[u32], color: &[Vec3], grid: Grid, samples: u32) -> Vec<u8> {
    let step = samples as usize;
    let (width, height) = (grid.width / step, grid.height / step);
    let total = samples * samples;
    let mut pixels = vec![0_u8; width * height * 4];
    for (index, pixel) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (column, row) = (index % width, index / width);
        let mut sum = Vec3::ZERO;
        let mut covered = 0_u32;
        for sample_row in row * step..(row + 1) * step {
            let start = sample_row * grid.width + column * step;
            for at in start..start + step {
                if winner[at] != EMPTY {
                    sum += color[at];
                    covered += 1;
                }
            }
        }
        if covered == 0 {
            continue;
        }
        let mean = sum * (1.0 / covered as f32);
        *pixel = [
            encode(mean.z),
            encode(mean.y),
            encode(mean.x),
            ((covered * 255 + total / 2) / total) as u8,
        ];
    }
    pixels
}

struct Lights {
    key: Vec3,
    key_half: Vec3,
    fill: Vec3,
    rim: Vec3,
}

impl Lights {
    const KEY: Vec3 = Vec3::new(1.0, 0.96, 0.9);
    const FILL: Vec3 = Vec3::new(0.26, 0.29, 0.34);
    const RIM: Vec3 = Vec3::new(0.32, 0.32, 0.34);
    const SKY: Vec3 = Vec3::new(0.3, 0.31, 0.34);
    const GROUND: Vec3 = Vec3::new(0.12, 0.11, 0.1);

    fn new() -> Self {
        let unit = |v: Vec3| v.try_normalize().unwrap_or(Vec3::new(0.0, 0.0, 1.0));
        let key = unit(Vec3::new(-0.45, 0.65, 0.6));
        Self {
            key,
            key_half: unit(key + Vec3::new(0.0, 0.0, 1.0)),
            fill: unit(Vec3::new(0.7, 0.05, 0.5)),
            rim: unit(Vec3::new(0.15, 0.45, -0.9)),
        }
    }

    fn shade(&self, normal: Vec3, eye: Vec3, albedo: Vec3) -> Vec3 {
        let tint = |a: Vec3, b: Vec3| Vec3::new(a.x * b.x, a.y * b.y, a.z * b.z);
        let sky = normal.y.mul_add(0.5, 0.5).clamp(0.0, 1.0);
        let ambient = Self::GROUND * (1.0 - sky) + Self::SKY * sky;
        let key = normal.dot(self.key).max(0.0);
        let fill = normal.dot(self.fill).max(0.0);
        let grazing = 1.0 - normal.dot(eye).clamp(0.0, 1.0);
        let rim = normal.dot(self.rim).max(0.0) * grazing;
        let light = ambient + Self::KEY * key + Self::FILL * fill + Self::RIM * rim;
        let specular = power48(normal.dot(self.key_half).max(0.0)) * 0.22 * key.min(1.0).sqrt();
        tint(albedo, light) + Self::KEY * specular
    }
}

fn power48(x: f32) -> f32 {
    let x2 = x * x;
    let x4 = x2 * x2;
    let x8 = x4 * x4;
    let x16 = x8 * x8;
    let x32 = x16 * x16;
    x32 * x16
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped into the table first"
)]
fn encode(linear: f32) -> u8 {
    static TABLE: LazyLock<Vec<u8>> = LazyLock::new(|| {
        (0..4096)
            .map(|i| {
                #[expect(clippy::cast_precision_loss, reason = "a 4096-entry table")]
                let c = i as f32 / 4095.0;
                let s = if c <= 0.003_130_8 {
                    c * 12.92
                } else {
                    1.055f32.mul_add(c.powf(1.0 / 2.4), -0.055)
                };
                (s * 255.0 + 0.5).clamp(0.0, 255.0) as u8
            })
            .collect()
    });
    let index = linear.clamp(0.0, 1.0).mul_add(4095.0, 0.5) as usize;
    TABLE[index.min(4095)]
}
