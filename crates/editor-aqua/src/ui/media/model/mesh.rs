pub use glam::{DMat4 as Mat4, Vec3};

pub const MAX_TRIANGLES: usize = 100_000;

pub fn normal_matrix(world: Mat4) -> glam::DMat3 {
    let linear = glam::DMat3::from_mat4(world);
    linear.inverse_or_zero().transpose() * linear.determinant().signum()
}

pub fn transform_normal(matrix: &glam::DMat3, normal: Vec3) -> Vec3 {
    (*matrix * normal.as_dvec3())
        .try_normalize()
        .map_or(Vec3::ZERO, glam::DVec3::as_vec3)
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Up {
    #[default]
    Y,
    Z,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Corner {
    pub position: Vec3,
    pub normal: Option<Vec3>,
    pub color: Option<Vec3>,
}

impl Corner {
    pub fn at(position: Vec3) -> Self {
        Self {
            position,
            ..Self::default()
        }
    }
}

#[derive(Debug)]
pub struct Mesh {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub colors: Option<Vec<Vec3>>,
}

impl Mesh {
    pub const fn triangles(&self) -> usize {
        self.positions.len() / 3
    }
}

#[derive(Default)]
pub struct MeshBuilder {
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
    colors: Vec<Option<Vec3>>,
    colored: bool,
    skipped: usize,
}

impl MeshBuilder {
    pub fn polygon(&mut self, corners: &[Corner]) -> Result<(), String> {
        let Some((first, rest)) = corners.split_first() else {
            return Ok(());
        };
        for pair in rest.windows(2) {
            self.triangle([*first, pair[0], pair[1]])?;
        }
        Ok(())
    }

    pub fn triangle(&mut self, corners: [Corner; 3]) -> Result<(), String> {
        if self.positions.len() / 3 >= MAX_TRIANGLES {
            return Err(format!("the model has more than {MAX_TRIANGLES} triangles"));
        }
        if corners.iter().any(|corner| !corner.position.is_finite()) {
            self.skipped += 1;
            return Ok(());
        }
        let [a, b, c] = corners.map(|corner| corner.position.as_dvec3());
        let Some(face) = (b - a).cross(c - a).try_normalize() else {
            return Ok(());
        };
        let face = face.as_vec3();
        for corner in corners {
            self.positions.push(corner.position);
            let normal = corner
                .normal
                .filter(|normal| normal.is_finite())
                .and_then(|normal| normal.as_dvec3().try_normalize().map(glam::DVec3::as_vec3))
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

    pub fn finish(self, up: Up) -> Result<Mesh, String> {
        if self.positions.is_empty() {
            return Err(if self.skipped > 0 {
                "every face of the model has an invalid vertex".to_owned()
            } else {
                "the model has no faces".to_owned()
            });
        }
        let lift = |v: Vec3| match up {
            Up::Y => v,
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

pub fn srgb_to_linear(channel: f32) -> f32 {
    if channel <= 0.040_45 {
        channel / 12.92
    } else {
        ((channel + 0.055) / 1.055).powf(2.4)
    }
}
