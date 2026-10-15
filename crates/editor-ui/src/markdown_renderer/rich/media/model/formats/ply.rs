//! Typed, bounded PLY records decoded by the maintained reader.
use super::super::mesh::{Corner, MAX_TRIANGLES, MeshBuilder, Vec3, srgb_to_linear};
use ply_rs_bw::{
    parser::{Parser, Reader},
    ply::{BeginList, Encoding, Property, PropertyAccess, PropertyAccessResult},
};
use std::io::{BufRead, Cursor};

#[derive(Default)]
struct Record {
    position: [Option<f32>; 3],
    normal: [Option<f32>; 3],
    color: [Option<f32>; 3],
    indices: Option<Vec<u32>>,
}
impl Record {
    fn corner(&self) -> Result<Corner, String> {
        let [Some(x), Some(y), Some(z)] = self.position else {
            return Err("a PLY vertex needs x, y, and z coordinates".into());
        };
        Ok(Corner {
            position: Vec3::new(x, y, z),
            normal: match self.normal {
                [Some(x), Some(y), Some(z)] => Some(Vec3::new(x, y, z)),
                _ => None,
            },
            color: match self.color {
                [Some(r), Some(g), Some(b)] => Some(Vec3::new(
                    srgb_to_linear(r.clamp(0.0, 1.0)),
                    srgb_to_linear(g.clamp(0.0, 1.0)),
                    srgb_to_linear(b.clamp(0.0, 1.0)),
                )),
                _ => None,
            },
        })
    }
}
macro_rules! bounded_list {
    ($method:ident,$ty:ty) => {
        fn $method(&mut self, _: &str, len: usize) -> BeginList<'_, $ty> {
            if len <= 4096 {
                BeginList::UseSetter
            } else {
                BeginList::UnsupportedType
            }
        }
    };
}
impl PropertyAccess for Record {
    fn new() -> Self {
        Self::default()
    }
    fn set_property(&mut self, name: &str, value: Property) -> PropertyAccessResult {
        if matches!(name, "vertex_indices" | "vertex_index") {
            let Some(indices) = value.to_u32_list() else {
                return PropertyAccessResult::UnsupportedType;
            };
            self.indices = Some(indices);
            return PropertyAccessResult::Set;
        }
        let Some(scalar) = value.to_f32_lossy() else {
            return PropertyAccessResult::Ignored;
        };
        let integer = !matches!(value, Property::Float(_) | Property::Double(_));
        match name {
            "x" => self.position[0] = Some(scalar),
            "y" => self.position[1] = Some(scalar),
            "z" => self.position[2] = Some(scalar),
            "nx" => self.normal[0] = Some(scalar),
            "ny" => self.normal[1] = Some(scalar),
            "nz" => self.normal[2] = Some(scalar),
            "red" | "green" | "blue" => {
                let slot = match name {
                    "red" => 0,
                    "green" => 1,
                    _ => 2,
                };
                self.color[slot] = Some(if integer { scalar / 255.0 } else { scalar });
            }
            _ => return PropertyAccessResult::Ignored,
        }
        PropertyAccessResult::Set
    }
    bounded_list!(begin_list_char, i8);
    bounded_list!(begin_list_uchar, u8);
    bounded_list!(begin_list_short, i16);
    bounded_list!(begin_list_ushort, u16);
    bounded_list!(begin_list_int, i32);
    bounded_list!(begin_list_uint, u32);
    bounded_list!(begin_list_float, f32);
    bounded_list!(begin_list_double, f64);
}
pub(in super::super) fn parse(bytes: &[u8], mesh: &mut MeshBuilder) -> Result<(), String> {
    // Cap header work before the library creates element/property maps.
    let header_end = bytes
        .windows(b"end_header".len())
        .position(|window| window == b"end_header")
        .ok_or("the PLY header does not end")?;
    if header_end > 64 * 1024 {
        return Err("the PLY header exceeds 64 KiB".into());
    }
    let parser = Parser::<Record>::new();
    let mut reader = Reader::new(Cursor::new(bytes));
    let header = parser
        .read_header(&mut reader)
        .map_err(|error| error.to_string())?;
    let mut records = 0usize;
    let mut work = 0usize;
    for element in header.elements.values() {
        records = records
            .checked_add(element.count)
            .ok_or("PLY record count overflow")?;
        work = work
            .checked_add(
                element
                    .count
                    .checked_mul(element.properties.len())
                    .ok_or("PLY work count overflow")?,
            )
            .ok_or("PLY work count overflow")?;
        if records > MAX_TRIANGLES * 4 || work > 8 * 1024 * 1024 {
            return Err("the PLY record work exceeds the preview budget".into());
        }
    }
    let mut vertices = Vec::new();
    let mut faces = Vec::new();
    let mut face_corners = 0usize;
    let mut triangles = 0usize;
    let mut line = String::new();
    for element in header.elements.values() {
        for _ in 0..element.count {
            let record = match header.encoding {
                Encoding::Ascii => {
                    line.clear();
                    if reader
                        .get_mut()
                        .read_line(&mut line)
                        .map_err(|error| error.to_string())?
                        == 0
                    {
                        return Err("the PLY body ends early".into());
                    }
                    parser.read_ascii_element(&line, element)
                }
                Encoding::BinaryLittleEndian => {
                    parser.read_little_endian_element(reader.get_mut(), element)
                }
                Encoding::BinaryBigEndian => {
                    parser.read_big_endian_element(reader.get_mut(), element)
                }
            }
            .map_err(|error| error.to_string())?;
            match element.name.as_str() {
                "vertex" => vertices.push(record.corner()?),
                "face" => {
                    let indices = record.indices.ok_or("a PLY face needs vertex_indices")?;
                    triangles += indices.len().saturating_sub(2);
                    face_corners += indices.len();
                    if triangles > MAX_TRIANGLES || face_corners > MAX_TRIANGLES * 5 {
                        return Err("the PLY has too many face corners".into());
                    }
                    faces.push(indices);
                }
                _ => {}
            }
        }
    }
    let mut corners = Vec::new();
    for face in faces {
        corners.clear();
        for index in face {
            corners.push(
                *vertices
                    .get(index as usize)
                    .ok_or("a PLY face names a vertex that does not exist")?,
            );
        }
        mesh.polygon(&corners)?;
    }
    Ok(())
}
