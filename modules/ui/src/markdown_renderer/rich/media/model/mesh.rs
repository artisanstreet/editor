//! The mesh every format parses into, and the little vector math it needs.
//!
//! A [`Mesh`] is a flat triangle soup: three corners per triangle, each with
//! its own normal and optional color, so a parser never has to agree with
//! another on indexing or attribute layout. [`MeshBuilder`] takes polygons
//! in the file's own coordinates, fans them into triangles, fills missing
//! normals with the face normal, turns Z-up files Y-up, and finally centers
//! the model and scales it into the unit sphere, so the camera frames every
//! model the same way whatever its units.

use std::ops::{Add, Mul, Neg, Sub};

/// The most triangles one model may hold. The 12 MiB file cap keeps real
/// files well below it; it bounds what a malicious count can allocate.
pub(super) const MAX_TRIANGLES: usize = 4_000_000;

/// A point or direction.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Vec3 {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) z: f32,
}

impl Vec3 {
    pub(super) const ZERO: Self = Self::new(0.0, 0.0, 0.0);

    pub(super) const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub(super) fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    pub(super) fn cross(self, other: Self) -> Self {
        Self::new(
            self.y * other.z - self.z * other.y,
            self.z * other.x - self.x * other.z,
            self.x * other.y - self.y * other.x,
        )
    }

    pub(super) fn length(self) -> f32 {
        self.dot(self).sqrt()
    }

    /// The unit vector along `self`, or `None` for a zero or non-finite one.
    pub(super) fn normalized(self) -> Option<Self> {
        let length = self.length();
        (length.is_finite() && length > 1e-20).then(|| self * (1.0 / length))
    }

    pub(super) fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }

    fn min(self, other: Self) -> Self {
        Self::new(
            self.x.min(other.x),
            self.y.min(other.y),
            self.z.min(other.z),
        )
    }

    fn max(self, other: Self) -> Self {
        Self::new(
            self.x.max(other.x),
            self.y.max(other.y),
            self.z.max(other.z),
        )
    }
}

impl Add for Vec3 {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }
}

impl Sub for Vec3 {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }
}

impl Mul<f32> for Vec3 {
    type Output = Self;

    fn mul(self, scale: f32) -> Self {
        Self::new(self.x * scale, self.y * scale, self.z * scale)
    }
}

impl Neg for Vec3 {
    type Output = Self;

    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z)
    }
}

/// An affine transform, row-major: `rows[r][c]`, translation in column 3.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Mat4 {
    pub(super) rows: [[f32; 4]; 4],
}

impl Mat4 {
    pub(super) const IDENTITY: Self = Self {
        rows: [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ],
    };

    /// A column-major array of 16, as glTF writes matrices.
    pub(super) fn from_column_major(values: &[f32; 16]) -> Self {
        let mut rows = [[0.0; 4]; 4];
        for (column, chunk) in values.as_chunks::<4>().0.iter().enumerate() {
            for (row, value) in chunk.iter().enumerate() {
                rows[row][column] = *value;
            }
        }
        Self { rows }
    }

    pub(super) fn translation(offset: Vec3) -> Self {
        let mut matrix = Self::IDENTITY;
        matrix.rows[0][3] = offset.x;
        matrix.rows[1][3] = offset.y;
        matrix.rows[2][3] = offset.z;
        matrix
    }

    pub(super) fn scale(scale: Vec3) -> Self {
        let mut matrix = Self::IDENTITY;
        matrix.rows[0][0] = scale.x;
        matrix.rows[1][1] = scale.y;
        matrix.rows[2][2] = scale.z;
        matrix
    }

    /// The rotation of the unit quaternion `[x, y, z, w]`.
    pub(super) fn rotation(quaternion: [f32; 4]) -> Self {
        let [x, y, z, w] = quaternion;
        let norm = (x * x + y * y + z * z + w * w).sqrt();
        if !norm.is_finite() || norm < 1e-12 {
            return Self::IDENTITY;
        }
        let (x, y, z, w) = (x / norm, y / norm, z / norm, w / norm);
        Self {
            rows: [
                [
                    1.0 - 2.0 * (y * y + z * z),
                    2.0 * (x * y - z * w),
                    2.0 * (x * z + y * w),
                    0.0,
                ],
                [
                    2.0 * (x * y + z * w),
                    1.0 - 2.0 * (x * x + z * z),
                    2.0 * (y * z - x * w),
                    0.0,
                ],
                [
                    2.0 * (x * z - y * w),
                    2.0 * (y * z + x * w),
                    1.0 - 2.0 * (x * x + y * y),
                    0.0,
                ],
                [0.0, 0.0, 0.0, 1.0],
            ],
        }
    }

    /// A rotation about one axis (0 = X, 1 = Y, 2 = Z) by `degrees`.
    pub(super) fn axis_rotation(axis: usize, degrees: f32) -> Self {
        let (sin, cos) = degrees.to_radians().sin_cos();
        let mut matrix = Self::IDENTITY;
        let (a, b) = match axis {
            0 => (1, 2),
            1 => (2, 0),
            _ => (0, 1),
        };
        matrix.rows[a][a] = cos;
        matrix.rows[a][b] = -sin;
        matrix.rows[b][a] = sin;
        matrix.rows[b][b] = cos;
        matrix
    }

    pub(super) fn transform_point(&self, point: Vec3) -> Vec3 {
        let r = &self.rows;
        Vec3::new(
            r[0][0] * point.x + r[0][1] * point.y + r[0][2] * point.z + r[0][3],
            r[1][0] * point.x + r[1][1] * point.y + r[1][2] * point.z + r[1][3],
            r[2][0] * point.x + r[2][1] * point.y + r[2][2] * point.z + r[2][3],
        )
    }

    /// The matrix that carries normals: the inverse transpose of the linear
    /// part, up to scale (the cofactor matrix). Normals are renormalized
    /// after, so the determinant's magnitude does not matter; its sign does,
    /// and the cofactor matrix keeps mirrored transforms' normals outward.
    pub(super) fn normal_matrix(&self) -> [[f32; 3]; 3] {
        let m = &self.rows;
        let cofactor = |r0: usize, r1: usize, c0: usize, c1: usize| {
            m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0]
        };
        [
            [
                cofactor(1, 2, 1, 2),
                -cofactor(1, 2, 0, 2),
                cofactor(1, 2, 0, 1),
            ],
            [
                -cofactor(0, 2, 1, 2),
                cofactor(0, 2, 0, 2),
                -cofactor(0, 2, 0, 1),
            ],
            [
                cofactor(0, 1, 1, 2),
                -cofactor(0, 1, 0, 2),
                cofactor(0, 1, 0, 1),
            ],
        ]
    }
}

impl Mul for Mat4 {
    type Output = Self;

    fn mul(self, other: Self) -> Self {
        let mut rows = [[0.0; 4]; 4];
        for (row, out) in rows.iter_mut().enumerate() {
            for (column, cell) in out.iter_mut().enumerate() {
                *cell = (0..4)
                    .map(|k| self.rows[row][k] * other.rows[k][column])
                    .sum();
            }
        }
        Self { rows }
    }
}

/// Applies a [`Mat4::normal_matrix`].
pub(super) fn transform_normal(matrix: &[[f32; 3]; 3], normal: Vec3) -> Vec3 {
    Vec3::new(
        matrix[0][0] * normal.x + matrix[0][1] * normal.y + matrix[0][2] * normal.z,
        matrix[1][0] * normal.x + matrix[1][1] * normal.y + matrix[1][2] * normal.z,
        matrix[2][0] * normal.x + matrix[2][1] * normal.y + matrix[2][2] * normal.z,
    )
}

/// Which axis points up in a file.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Up {
    #[default]
    Y,
    Z,
}

/// One polygon corner as a parser read it.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Corner {
    pub(super) position: Vec3,
    pub(super) normal: Option<Vec3>,
    /// Linear RGB, `0..=1`.
    pub(super) color: Option<Vec3>,
}

impl Corner {
    pub(super) fn at(position: Vec3) -> Self {
        Self {
            position,
            ..Self::default()
        }
    }
}

/// A model ready to draw: centered, inside the unit sphere, Y up.
#[derive(Debug)]
pub(super) struct Mesh {
    /// Three per triangle.
    pub(super) positions: Vec<Vec3>,
    /// Unit length, one per position.
    pub(super) normals: Vec<Vec3>,
    /// Linear RGB per position, when the file colors any corner.
    pub(super) colors: Option<Vec<Vec3>>,
}

impl Mesh {
    pub(super) fn triangles(&self) -> usize {
        self.positions.len() / 3
    }

    /// The same triangles as GPU geometry. Corners of an uncolored mesh
    /// flag their color unused, so the style's base color shows.
    pub(super) fn to_gpu(&self) -> gpui::Mesh {
        let vertices = self
            .positions
            .iter()
            .zip(&self.normals)
            .enumerate()
            .map(|(index, (position, normal))| gpui::MeshVertex {
                position: [position.x, position.y, position.z],
                normal: [normal.x, normal.y, normal.z],
                color: self.colors.as_ref().map_or([0.0; 4], |colors| {
                    let color = colors[index];
                    [color.x, color.y, color.z, 1.0]
                }),
            })
            .collect();
        gpui::Mesh::new(vertices)
    }
}

/// Collects polygons into a [`Mesh`].
#[derive(Default)]
pub(super) struct MeshBuilder {
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
    colors: Vec<Option<Vec3>>,
    colored: bool,
    /// Polygons dropped for a non-finite corner.
    skipped: usize,
}

impl MeshBuilder {
    /// Adds one convex polygon, fanned from its first corner. Polygons with
    /// fewer than three corners, and triangles with a non-finite or
    /// coincident corner, are dropped.
    pub(super) fn polygon(&mut self, corners: &[Corner]) -> Result<(), String> {
        let Some((first, rest)) = corners.split_first() else {
            return Ok(());
        };
        for pair in rest.windows(2) {
            self.triangle([*first, pair[0], pair[1]])?;
        }
        Ok(())
    }

    pub(super) fn triangle(&mut self, corners: [Corner; 3]) -> Result<(), String> {
        if self.positions.len() / 3 >= MAX_TRIANGLES {
            return Err(format!(
                "the model has more than {} million triangles",
                MAX_TRIANGLES / 1_000_000
            ));
        }
        if corners.iter().any(|corner| !corner.position.is_finite()) {
            self.skipped += 1;
            return Ok(());
        }
        let [a, b, c] = corners.map(|corner| corner.position);
        let Some(face) = (b - a).cross(c - a).normalized() else {
            // A sliver with no area draws nothing.
            return Ok(());
        };
        for corner in corners {
            self.positions.push(corner.position);
            let normal = corner
                .normal
                .filter(|normal| normal.is_finite())
                .and_then(Vec3::normalized)
                .unwrap_or(face);
            self.normals.push(normal);
            self.colored |= corner.color.is_some();
            self.colors.push(corner.color);
        }
        Ok(())
    }

    /// Centers the model, scales it into the unit sphere, and turns it Y up.
    pub(super) fn finish(self, up: Up) -> Result<Mesh, String> {
        if self.positions.is_empty() {
            return Err(if self.skipped > 0 {
                "every face of the model has an invalid vertex".to_owned()
            } else {
                "the model has no faces".to_owned()
            });
        }
        let lift = |v: Vec3| match up {
            Up::Y => v,
            // Z-up to Y-up: a quarter turn about X, keeping handedness.
            Up::Z => Vec3::new(v.x, v.z, -v.y),
        };
        let (low, high) = self.positions.iter().fold(
            (
                Vec3::new(f32::MAX, f32::MAX, f32::MAX),
                Vec3::new(f32::MIN, f32::MIN, f32::MIN),
            ),
            |(low, high), point| (low.min(*point), high.max(*point)),
        );
        let center = (low + high) * 0.5;
        let radius = self
            .positions
            .iter()
            .map(|point| (*point - center).length())
            .fold(0.0_f32, f32::max);
        let scale = if radius > 0.0 && radius.is_finite() {
            1.0 / radius
        } else {
            1.0
        };
        let positions = self
            .positions
            .into_iter()
            .map(|point| lift((point - center) * scale))
            .collect();
        let normals = self.normals.into_iter().map(lift).collect();
        let colors = self.colored.then(|| {
            self.colors
                .into_iter()
                .map(|color| color.unwrap_or(Vec3::new(1.0, 1.0, 1.0)))
                .collect()
        });
        Ok(Mesh {
            positions,
            normals,
            colors,
        })
    }
}

/// An sRGB channel in `0..=1` as linear light.
pub(super) fn srgb_to_linear(channel: f32) -> f32 {
    if channel <= 0.040_45 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}
