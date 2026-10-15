//! Bounded in-memory OBJ/FBX import. The Client never opens sidecar paths.
use super::super::mesh::{Corner, MAX_TRIANGLES, MeshBuilder, Up, Vec3, srgb_to_linear};

const MAX_FACE_CORNERS: u32 = 4096;
const MAX_SCENE_BYTES: usize = 64 * 1024 * 1024;

pub(super) fn parse(
    bytes: &[u8],
    format: ufbx::FileFormat,
    output: &mut MeshBuilder,
) -> Result<Up, String> {
    let deny_external = |_: &str, _: &ufbx::OpenFileInfo| None;
    let opts = ufbx::LoadOpts {
        file_format: format,
        temp_allocator: ufbx::AllocatorOpts {
            memory_limit: MAX_SCENE_BYTES,
            allocation_limit: 262144,
            ..Default::default()
        },
        result_allocator: ufbx::AllocatorOpts {
            memory_limit: MAX_SCENE_BYTES,
            allocation_limit: 262144,
            ..Default::default()
        },
        node_depth_limit: 64,
        ignore_animation: true,
        ignore_embedded: true,
        load_external_files: false,
        open_file_cb: ufbx::OpenFileCb::Ref(&deny_external),
        obj_search_mtl_by_filename: false,
        index_error_handling: ufbx::IndexErrorHandling::NoIndex,
        ..Default::default()
    };
    let scene = ufbx::load_memory(bytes, opts).map_err(|error| {
        format!(
            "{}: {}",
            if format == ufbx::FileFormat::Obj {
                "OBJ"
            } else if super::fbx::is_ascii(bytes) {
                "ASCII FBX"
            } else {
                "FBX"
            },
            error.description
        )
    })?;
    let mut indices = Vec::new();
    let mut triangles = 0usize;
    let mut face_ordinal = 0usize;
    // Only diagnostics scan record locations; the maintained reader owns all OBJ grammar.
    let face_lines: Vec<_> = if format == ufbx::FileFormat::Obj {
        std::str::from_utf8(bytes)
            .map_err(|_| "the OBJ is not text")?
            .lines()
            .enumerate()
            .filter_map(|(line, text)| {
                (text.split_ascii_whitespace().next() == Some("f")).then_some(line + 1)
            })
            .collect()
    } else {
        Vec::new()
    };
    for node in &scene.nodes {
        let Some(mesh) = node.mesh.as_ref() else {
            continue;
        };
        let normal_matrix = ufbx::matrix_for_normals(&node.geometry_to_world);
        for (face_index, face) in mesh.faces.iter().enumerate() {
            let face_line = face_lines.get(face_ordinal).copied();
            let fail = || {
                face_line.map_or_else(
                    || "a face names a vertex that does not exist".to_owned(),
                    |line| format!("line {line}: a face names a vertex that does not exist"),
                )
            };
            face_ordinal += 1;
            if face.num_indices > MAX_FACE_CORNERS {
                return Err("a model face has more than 4096 corners".into());
            }
            triangles = triangles
                .checked_add(face.num_indices.saturating_sub(2) as usize)
                .ok_or("model triangle count overflow")?;
            if triangles > MAX_TRIANGLES {
                return Err(format!("the model has more than {MAX_TRIANGLES} triangles"));
            }
            let begin = face.index_begin as usize;
            let end = begin
                .checked_add(face.num_indices as usize)
                .ok_or_else(&fail)?;
            for corner in begin..end {
                if attribute3(&mesh.vertex_position, corner).is_none() {
                    return Err(fail());
                }
            }
            let count = ufbx::triangulate_face_vec(&mut indices, mesh, *face) as usize;
            let material = mesh
                .face_material
                .get(face_index)
                .and_then(|index| node.materials.get(*index as usize));
            let diffuse = material
                .filter(|material| material.fbx.diffuse_color.has_value)
                .map(|material| rgb(material.fbx.diffuse_color.value_vec4));
            for tri in indices[..count * 3].as_chunks::<3>().0 {
                let mut corners = [Corner::default(); 3];
                for (corner, index) in corners.iter_mut().zip(tri) {
                    let index = *index as usize;
                    let position = attribute3(&mesh.vertex_position, index).ok_or_else(&fail)?;
                    let position = ufbx::transform_position(&node.geometry_to_world, position);
                    *corner = Corner {
                        position: vec3(position),
                        normal: attribute3(&mesh.vertex_normal, index)
                            .map(|normal| vec3(ufbx::transform_direction(&normal_matrix, normal))),
                        color: attribute4(&mesh.vertex_color, index).map(rgb).or(diffuse),
                    };
                }
                output.triangle(corners)?;
            }
        }
    }
    if triangles == 0 {
        return Err(format!(
            "{} contains no mesh faces",
            if format == ufbx::FileFormat::Obj {
                "OBJ"
            } else if super::fbx::is_ascii(bytes) {
                "ASCII FBX"
            } else {
                "FBX"
            }
        ));
    }
    Ok(match scene.settings.axes.up {
        ufbx::CoordinateAxis::PositiveZ | ufbx::CoordinateAxis::NegativeZ => Up::Z,
        _ => Up::Y,
    })
}
fn attribute3(values: &ufbx::VertexVec3, corner: usize) -> Option<ufbx::Vec3> {
    values
        .exists
        .then(|| {
            values
                .indices
                .get(corner)
                .and_then(|index| values.values.get(*index as usize))
                .copied()
        })
        .flatten()
}
fn attribute4(values: &ufbx::VertexVec4, corner: usize) -> Option<ufbx::Vec4> {
    values
        .exists
        .then(|| {
            values
                .indices
                .get(corner)
                .and_then(|index| values.values.get(*index as usize))
                .copied()
        })
        .flatten()
}
fn vec3(v: ufbx::Vec3) -> Vec3 {
    Vec3::new(v.x as f32, v.y as f32, v.z as f32)
}
fn rgb(v: ufbx::Vec4) -> Vec3 {
    Vec3::new(
        srgb_to_linear((v.x as f32).clamp(0.0, 1.0)),
        srgb_to_linear((v.y as f32).clamp(0.0, 1.0)),
        srgb_to_linear((v.z as f32).clamp(0.0, 1.0)),
    )
}
