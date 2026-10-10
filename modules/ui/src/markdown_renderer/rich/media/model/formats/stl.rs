//! STL, binary or ASCII.
//!
//! Binary STL is an 80-byte header, a triangle count, and 50 bytes per
//! triangle; ASCII STL starts with `solid` and lists `facet`s of three
//! `vertex` lines. Some binary exporters also begin their header with
//! `solid`, so a file is binary whenever its length matches its count. The
//! stored facet normals are often wrong or zero, so the face normal from
//! the vertices wins.

use super::super::mesh::{Corner, MeshBuilder, Vec3};

const HEADER: usize = 80;
const RECORD: usize = 50;

/// Whether `bytes` are a binary STL: the length its triangle count implies.
pub(in super::super) fn is_binary(bytes: &[u8]) -> bool {
    bytes.len() >= HEADER + 4
        && binary_count(bytes).is_some_and(|count| {
            count
                .checked_mul(RECORD)
                .and_then(|size| size.checked_add(HEADER + 4))
                == Some(bytes.len())
        })
}

/// Whether `bytes` read as an ASCII STL.
pub(in super::super) fn is_ascii(bytes: &[u8]) -> bool {
    let head = super::text_head(bytes);
    head.trim_start().starts_with("solid") && head.contains("facet")
}

fn binary_count(bytes: &[u8]) -> Option<usize> {
    let count = bytes.get(HEADER..HEADER + 4)?;
    usize::try_from(u32::from_le_bytes(count.try_into().ok()?)).ok()
}

pub(in super::super) fn parse(bytes: &[u8], mesh: &mut MeshBuilder) -> Result<(), String> {
    if is_binary(bytes) {
        for record in bytes[HEADER + 4..].as_chunks::<RECORD>().0 {
            let float = |at: usize| {
                f32::from_le_bytes([record[at], record[at + 1], record[at + 2], record[at + 3]])
            };
            let point = |at: usize| Vec3::new(float(at), float(at + 4), float(at + 8));
            mesh.triangle([
                Corner::at(point(12)),
                Corner::at(point(24)),
                Corner::at(point(36)),
            ])?;
        }
        return Ok(());
    }
    let text = std::str::from_utf8(bytes).map_err(|_| "the STL is neither binary nor text")?;
    let mut corners = Vec::with_capacity(3);
    for (number, line) in text.lines().enumerate() {
        let mut fields = line.split_ascii_whitespace();
        match fields.next() {
            Some("vertex") => {
                let values: Option<Vec<f32>> = fields.map(|field| field.parse().ok()).collect();
                match values.as_deref() {
                    Some([x, y, z]) => corners.push(Corner::at(Vec3::new(*x, *y, *z))),
                    _ => return Err(format!("line {}: a vertex needs three numbers", number + 1)),
                }
            }
            Some("endfacet") => {
                mesh.polygon(&corners)?;
                corners.clear();
            }
            _ => {}
        }
    }
    Ok(())
}
