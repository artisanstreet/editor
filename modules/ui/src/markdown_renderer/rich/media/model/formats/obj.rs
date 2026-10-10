//! Wavefront OBJ: `v` (with optional RGB), `vn`, and `f` records.
//!
//! Faces index from 1, or from the end when negative; `v/vt/vn`, `v//vn`,
//! and `v/vt` corners all work. Materials live in a separate `.mtl` file
//! the block cannot reach, so they are ignored; vertex colors are kept.

use super::super::mesh::{Corner, MeshBuilder, Vec3, srgb_to_linear};

pub(in super::super) fn parse(text: &str, mesh: &mut MeshBuilder) -> Result<(), String> {
    let mut positions: Vec<(Vec3, Option<Vec3>)> = Vec::new();
    let mut normals: Vec<Vec3> = Vec::new();
    let mut corners: Vec<Corner> = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.split('#').next().unwrap_or_default();
        let mut fields = line.split_ascii_whitespace();
        let fail = |what: &str| format!("line {}: {what}", number + 1);
        match fields.next() {
            Some("v") => {
                let values = numbers(fields).ok_or_else(|| fail("a vertex is not numbers"))?;
                let position = match values.as_slice() {
                    [x, y, z, ..] => Vec3::new(*x, *y, *z),
                    _ => return Err(fail("a vertex needs three coordinates")),
                };
                // `v x y z r g b`: a common extension carrying sRGB color.
                let color = match values.as_slice() {
                    [_, _, _, r, g, b] => Some(Vec3::new(
                        srgb_to_linear(r.clamp(0.0, 1.0)),
                        srgb_to_linear(g.clamp(0.0, 1.0)),
                        srgb_to_linear(b.clamp(0.0, 1.0)),
                    )),
                    _ => None,
                };
                positions.push((position, color));
            }
            Some("vn") => {
                let values = numbers(fields).ok_or_else(|| fail("a normal is not numbers"))?;
                match values.as_slice() {
                    [x, y, z, ..] => normals.push(Vec3::new(*x, *y, *z)),
                    _ => return Err(fail("a normal needs three components")),
                }
            }
            Some("f") => {
                corners.clear();
                for field in fields {
                    let mut parts = field.split('/');
                    let position = parts
                        .next()
                        .and_then(|index| resolve(index, positions.len()))
                        .ok_or_else(|| fail("a face names a vertex that does not exist"))?;
                    let normal = match parts.nth(1).filter(|index| !index.is_empty()) {
                        Some(index) => Some(
                            resolve(index, normals.len())
                                .ok_or_else(|| fail("a face names a normal that does not exist"))?,
                        ),
                        None => None,
                    };
                    let (position, color) = positions[position];
                    corners.push(Corner {
                        position,
                        normal: normal.map(|index| normals[index]),
                        color,
                    });
                }
                mesh.polygon(&corners)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn numbers<'a>(fields: impl Iterator<Item = &'a str>) -> Option<Vec<f32>> {
    fields.map(|field| field.parse().ok()).collect()
}

/// A 1-based or negative (from the end) OBJ index into `len` items.
fn resolve(index: &str, len: usize) -> Option<usize> {
    let index: i64 = index.parse().ok()?;
    let len = i64::try_from(len).ok()?;
    let resolved = if index < 0 { len + index } else { index - 1 };
    (0..len)
        .contains(&resolved)
        .then(|| usize::try_from(resolved).ok())
        .flatten()
}
