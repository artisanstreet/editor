//! Streaming STL decoding through the maintained reader.
use super::super::mesh::{Corner, MAX_TRIANGLES, MeshBuilder, Vec3};
use std::io::Cursor;

pub(in super::super) fn is_binary(bytes: &[u8]) -> bool {
    bytes
        .get(80..84)
        .and_then(|count| count.try_into().ok())
        .map(u32::from_le_bytes)
        .and_then(|count| (count as usize).checked_mul(50))
        .and_then(|size| size.checked_add(84))
        == Some(bytes.len())
}
pub(in super::super) fn is_ascii(bytes: &[u8]) -> bool {
    let head = super::text_head(bytes);
    head.trim_start().starts_with("solid") && head.contains("facet")
}
pub(in super::super) fn parse(bytes: &[u8], mesh: &mut MeshBuilder) -> Result<(), String> {
    let mut input = Cursor::new(bytes);
    let reader = stl_io::create_stl_reader(&mut input).map_err(|error| error.to_string())?;
    for (index, triangle) in reader.enumerate() {
        if index >= MAX_TRIANGLES {
            return Err(format!("the model has more than {MAX_TRIANGLES} triangles"));
        }
        let triangle = triangle.map_err(|error| error.to_string())?;
        // Exporter facet normals are frequently stale; MeshBuilder derives the face normal.
        mesh.triangle(
            triangle
                .vertices
                .map(|v| Corner::at(Vec3::new(v[0], v[1], v[2]))),
        )?;
    }
    Ok(())
}
