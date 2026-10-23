//! glTF parsing, validation, accessor decoding and node transforms belong to
//! the maintained gltf crate. This adapter only chooses self-contained
//! buffers, applies preview budgets and maps primitives to our mesh format.

use super::super::mesh::{
    Corner, MAX_TRIANGLES, Mat4, MeshBuilder, Vec3, normal_matrix, transform_normal,
};
use base64::Engine as _;
use gltf::{Semantic, buffer::Source, mesh::Mode};
use std::borrow::Cow;

const MAX_BUFFER_BYTES: usize = 64 * 1024 * 1024;
const MAX_ACCESSOR_ELEMENTS: usize = MAX_TRIANGLES * 3;
const MAX_NODE_VISITS: usize = 4096;

pub(in super::super) fn is_glb(bytes: &[u8]) -> bool {
    bytes.starts_with(b"glTF")
}
pub(in super::super) fn is_json(bytes: &[u8]) -> bool {
    let head = super::text_head(bytes);
    head.trim_start().starts_with('{') && head.contains("\"asset\"")
}

pub(in super::super) fn parse(bytes: &[u8], mesh: &mut MeshBuilder) -> Result<(), String> {
    // gltf1.4.1 subtracts the12-byte header from its declared length without
    // checking for underflow. Reject inconsistent envelopes before library parsing.
    if is_glb(bytes) {
        let declared = bytes
            .get(8..12)
            .and_then(|length| length.try_into().ok())
            .map(u32::from_le_bytes)
            .ok_or("GLB header is truncated")?;
        if declared < 12 || declared as usize != bytes.len() {
            return Err("GLB declared length does not match its bytes".into());
        }
    }
    let file = gltf::Gltf::from_slice(bytes).map_err(|error| format!("Invalid glTF: {error}"))?;
    for accessor in file.accessors() {
        if accessor.count() > MAX_ACCESSOR_ELEMENTS {
            return Err("glTF accessor exceeds the preview element budget".into());
        }
    }
    let mut retained = 0usize;
    let mut buffers = Vec::new();
    for buffer in file.buffers() {
        retained = retained
            .checked_add(buffer.length())
            .ok_or("glTF buffer size overflow")?;
        if retained > MAX_BUFFER_BYTES {
            return Err("glTF buffers exceed the64 MiB preview budget".into());
        }
        let data = match buffer.source() {
            Source::Bin => {
                Cow::Borrowed(file.blob.as_deref().ok_or("GLB binary buffer is missing")?)
            }
            Source::Uri(uri) => {
                let Some(data) = uri.strip_prefix("data:") else {
                    return Err(format!(
                        "glTF buffer `{uri}` is external; use a self-contained .glb"
                    ));
                };
                let (header, body) = data.split_once(',').ok_or("Invalid glTF data URI")?;
                if !header.ends_with(";base64") {
                    return Err("glTF buffer data URI must be base64".into());
                }
                let available = MAX_BUFFER_BYTES - retained + buffer.length();
                if body.len() / 4 * 3 > available + 2 {
                    return Err("glTF embedded buffer exceeds preview budget".into());
                }
                Cow::Owned(
                    base64::engine::general_purpose::STANDARD
                        .decode(body)
                        .map_err(|error| format!("Invalid glTF buffer: {error}"))?,
                )
            }
        };
        if data.len() < buffer.length() || data.len() > MAX_BUFFER_BYTES {
            return Err("glTF buffer length does not fit its declaration or preview budget".into());
        }
        retained = retained
            .checked_add(data.len() - buffer.length())
            .ok_or("glTF buffer size overflow")?;
        if retained > MAX_BUFFER_BYTES {
            return Err("glTF buffers exceed the64 MiB preview budget".into());
        }
        buffers.push(data);
    }
    let mut visits = 0;
    if let Some(scene) = file.default_scene().or_else(|| file.scenes().next()) {
        for root in scene.nodes() {
            node(&root, Mat4::IDENTITY, 0, &mut visits, &buffers, mesh)?;
        }
    } else {
        for entry in file.meshes() {
            primitive_mesh(&entry, Mat4::IDENTITY, &buffers, mesh)?;
        }
    }
    Ok(())
}

fn node(
    node: &gltf::Node<'_>,
    parent: Mat4,
    depth: usize,
    visits: &mut usize,
    buffers: &[Cow<'_, [u8]>],
    mesh: &mut MeshBuilder,
) -> Result<(), String> {
    *visits += 1;
    if depth > 64 || *visits > MAX_NODE_VISITS {
        return Err("glTF node traversal exceeds preview budget".into());
    }
    let matrix = node.transform().matrix();
    let flat = std::array::from_fn(|index| matrix[index / 4][index % 4]);
    let world = parent * Mat4::from_cols_array(&flat.map(f64::from));
    if let Some(entry) = node.mesh() {
        primitive_mesh(&entry, world, buffers, mesh)?;
    }
    for child in node.children() {
        self::node(&child, world, depth + 1, visits, buffers, mesh)?;
    }
    Ok(())
}

#[expect(
    clippy::many_single_char_names,
    reason = "x, y, z and r, g, b name the channels they hold"
)]
fn primitive_mesh(
    entry: &gltf::Mesh<'_>,
    world: Mat4,
    buffers: &[Cow<'_, [u8]>],
    mesh: &mut MeshBuilder,
) -> Result<(), String> {
    let normal_matrix = normal_matrix(world);
    for primitive in entry.primitives() {
        let mode = match primitive.mode() {
            Mode::Triangles => 4,
            Mode::TriangleStrip => 5,
            Mode::TriangleFan => 6,
            Mode::Points | Mode::Lines | Mode::LineLoop | Mode::LineStrip => continue,
        };
        if primitive.get(&Semantic::Positions).is_none() {
            continue;
        }
        let reader =
            primitive.reader(|buffer| buffers.get(buffer.index()).map(std::convert::AsRef::as_ref));
        let positions: Vec<_> = reader
            .read_positions()
            .ok_or("glTF positions cannot be read")?
            .collect();
        let normals = reader
            .read_normals()
            .map(std::iter::Iterator::collect::<Vec<_>>);
        let colors = reader
            .read_colors(0)
            .map(|colors| colors.into_rgba_f32().collect::<Vec<_>>());
        let has_color = colors.is_some() || primitive.material().index().is_some();
        let factor = primitive
            .material()
            .pbr_metallic_roughness()
            .base_color_factor();
        let indices: Vec<usize> = match reader.read_indices() {
            Some(indices) => indices.into_u32().map(|index| index as usize).collect(),
            None if primitive.indices().is_some() => {
                return Err("glTF indices cannot be read".into());
            }
            None => (0..positions.len()).collect(),
        };
        let corner = |index: usize| -> Result<Corner, String> {
            let [x, y, z] = *positions
                .get(index)
                .ok_or("glTF index points past its vertices")?;
            let [r, g, b, _] = colors
                .as_ref()
                .and_then(|colors| colors.get(index))
                .copied()
                .unwrap_or([1.0; 4]);
            Ok(Corner {
                position: world
                    .transform_point3(Vec3::new(x, y, z).as_dvec3())
                    .as_vec3(),
                normal: normals
                    .as_ref()
                    .and_then(|normals| normals.get(index))
                    .map(|[x, y, z]| transform_normal(&normal_matrix, Vec3::new(*x, *y, *z))),
                color: has_color.then(|| Vec3::new(r * factor[0], g * factor[1], b * factor[2])),
            })
        };
        for [a, b, c] in assemble(mode, &indices) {
            mesh.triangle([corner(a)?, corner(b)?, corner(c)?])?;
        }
    }
    Ok(())
}

fn assemble(mode: u32, indices: &[usize]) -> Vec<[usize; 3]> {
    match mode {
        5 => indices
            .windows(3)
            .enumerate()
            .map(|(at, w)| {
                // Every other strip triangle flips to keep one winding.
                if at % 2 == 0 {
                    [w[0], w[1], w[2]]
                } else {
                    [w[1], w[0], w[2]]
                }
            })
            .collect(),
        6 => match indices.split_first() {
            Some((first, rest)) => rest.windows(2).map(|w| [*first, w[0], w[1]]).collect(),
            None => Vec::new(),
        },
        _ => indices.as_chunks::<3>().0.to_vec(),
    }
}
