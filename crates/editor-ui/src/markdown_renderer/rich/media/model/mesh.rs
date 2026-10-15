//! The mesh every format parses into, and the little vector math it needs.
//!
//! A [`Mesh`] is a flat triangle soup: three corners per triangle, each with
//! its own normal and optional color, so a parser never has to agree with
//! another on indexing or attribute layout. [`MeshBuilder`] takes polygons
//! in the file's own coordinates, fans them into triangles, fills missing
//! normals with the face normal, turns Z-up files Y-up, and finally centers
//! the model and scales it into the unit sphere, so the camera frames every
//! model the same way whatever its units.

pub(super) use glam::{DMat4 as Mat4, Vec3};

/// The most triangles one model may hold; protects retained geometry.
pub(super) const MAX_TRIANGLES: usize = 100_000;

/// The maintained matrix inverse handles singular transforms as zero.
/// Multiplying by the determinant sign preserves reflected face winding.
pub(super) fn normal_matrix(world: Mat4) -> glam::DMat3 {
    let linear = glam::DMat3::from_mat4(world);
    linear.inverse_or_zero().transpose() * linear.determinant().signum()
}

pub(super) fn transform_normal(matrix: &glam::DMat3, normal: Vec3) -> Vec3 {
    (*matrix * normal.as_dvec3())
        .try_normalize()
        .map_or(Vec3::ZERO, |normal| normal.as_vec3())
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
                "the model has more than {} triangles",
                MAX_TRIANGLES
            ));
        }
        if corners.iter().any(|corner| !corner.position.is_finite()) {
            self.skipped += 1;
            return Ok(());
        }
        let [a, b, c] = corners.map(|corner| corner.position.as_dvec3());
        let Some(face) = (b - a).cross(c - a).try_normalize() else {
            // A sliver with no area draws nothing.
            return Ok(());
        };
        let face = face.as_vec3();
        for corner in corners {
            self.positions.push(corner.position);
            let normal = corner
                .normal
                .filter(|normal| normal.is_finite())
                .and_then(|normal| {
                    normal
                        .as_dvec3()
                        .try_normalize()
                        .map(|normal| normal.as_vec3())
                })
                .unwrap_or(face);
            self.normals.push(normal);
            let color = corner.color.filter(|color| color.is_finite()).map(|color| {
                Vec3::new(
                    color.x.clamp(0.0, 1.0),
                    color.y.clamp(0.0, 1.0),
                    color.z.clamp(0.0, 1.0),
                )
            });
            self.colored |= color.is_some();
            self.colors.push(color);
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
        let (low, high) = self.positions.iter().map(|point| point.as_dvec3()).fold(
            (
                glam::DVec3::splat(f64::INFINITY),
                glam::DVec3::splat(f64::NEG_INFINITY),
            ),
            |(low, high), point| (low.min(point), high.max(point)),
        );
        let center = (low + high) * 0.5;
        let radius = self
            .positions
            .iter()
            .map(|point| (point.as_dvec3() - center).length())
            .fold(0.0_f64, f64::max);
        let scale = if radius > 0.0 { radius.recip() } else { 1.0 };
        let positions = self
            .positions
            .into_iter()
            .map(|point| lift(((point.as_dvec3() - center) * scale).as_vec3()))
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finite_extreme_and_tiny_models_normalize_inside_unit_sphere() {
        for scale in [f32::MAX, f32::MIN_POSITIVE, 1.0] {
            let mut builder = MeshBuilder::default();
            builder
                .triangle([
                    Corner::at(Vec3::new(-scale, 0.0, 0.0)),
                    Corner::at(Vec3::new(scale, 0.0, 0.0)),
                    Corner::at(Vec3::new(0.0, scale, 0.0)),
                ])
                .unwrap();
            let mesh = builder.finish(Up::Y).unwrap();
            assert_eq!(mesh.positions.len(), 3);
            assert!(
                mesh.positions
                    .iter()
                    .all(|position| position.is_finite() && position.length() <= 1.000_001)
            );
            assert!(
                mesh.normals
                    .iter()
                    .all(|normal| normal.is_finite() && (normal.z - 1.0).abs() < 1e-6)
            );
        }
    }

    #[test]
    fn maintained_matrix_preserves_reflected_winding_and_singular_fallback() {
        let reflected = normal_matrix(Mat4::from_scale(glam::DVec3::new(-1.0, 1.0, 1.0)));
        assert_eq!(transform_normal(&reflected, Vec3::Z), -Vec3::Z);
        let singular = normal_matrix(Mat4::from_scale(glam::DVec3::ZERO));
        assert_eq!(transform_normal(&singular, Vec3::Z), Vec3::ZERO);
    }

    #[test]
    fn nonfinite_vertex_colors_never_reach_renderer_geometry() {
        let mut builder = MeshBuilder::default();
        builder
            .triangle([
                Corner {
                    position: Vec3::new(0.0, 0.0, 0.0),
                    color: Some(Vec3::new(f32::NAN, 1.0, 0.0)),
                    ..Default::default()
                },
                Corner {
                    position: Vec3::new(1.0, 0.0, 0.0),
                    color: Some(Vec3::new(2.0, -1.0, 0.5)),
                    ..Default::default()
                },
                Corner::at(Vec3::new(0.0, 1.0, 0.0)),
            ])
            .unwrap();
        let colors = builder.finish(Up::Y).unwrap().colors.unwrap();
        assert_eq!(
            colors,
            vec![
                Vec3::new(1.0, 1.0, 1.0),
                Vec3::new(1.0, 0.0, 0.5),
                Vec3::new(1.0, 1.0, 1.0)
            ]
        );
    }
}
