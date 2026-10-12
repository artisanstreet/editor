//! Stanford PLY: ASCII and both binary byte orders.
//!
//! The header declares elements and their properties; the block reads the
//! `vertex` element's `x y z`, `nx ny nz`, and `red green blue`, and the
//! `face` element's index list (`vertex_indices` or `vertex_index`). Every
//! other element and property is skipped by its declared type.

use super::super::mesh::{Corner, MeshBuilder, Vec3, srgb_to_linear};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Encoding {
    Ascii,
    Little,
    Big,
}

#[derive(Clone, Copy, Debug)]
enum Scalar {
    I8,
    U8,
    I16,
    U16,
    I32,
    U32,
    F32,
    F64,
}

impl Scalar {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "char" | "int8" => Self::I8,
            "uchar" | "uint8" => Self::U8,
            "short" | "int16" => Self::I16,
            "ushort" | "uint16" => Self::U16,
            "int" | "int32" => Self::I32,
            "uint" | "uint32" => Self::U32,
            "float" | "float32" => Self::F32,
            "double" | "float64" => Self::F64,
            _ => return None,
        })
    }

    const fn size(self) -> usize {
        match self {
            Self::I8 | Self::U8 => 1,
            Self::I16 | Self::U16 => 2,
            Self::I32 | Self::U32 | Self::F32 => 4,
            Self::F64 => 8,
        }
    }

    /// Whether colors of this type run `0..=255` rather than `0..=1`.
    const fn is_integer(self) -> bool {
        !matches!(self, Self::F32 | Self::F64)
    }
}

#[derive(Debug)]
enum Kind {
    Scalar(Scalar),
    List { count: Scalar, item: Scalar },
}

#[derive(Debug)]
struct Property {
    name: String,
    kind: Kind,
}

#[derive(Debug)]
struct Element {
    name: String,
    count: usize,
    properties: Vec<Property>,
}

/// Reads values in the file's encoding.
struct Reader<'a> {
    encoding: Encoding,
    bytes: &'a [u8],
    at: usize,
    words: std::str::SplitAsciiWhitespace<'a>,
}

impl Reader<'_> {
    fn read(&mut self, scalar: Scalar) -> Result<f64, String> {
        if self.encoding == Encoding::Ascii {
            return self
                .words
                .next()
                .and_then(|word| word.parse::<f64>().ok())
                .ok_or_else(|| "the PLY body ends early or holds a non-number".to_owned());
        }
        let size = scalar.size();
        let raw = self
            .bytes
            .get(self.at..self.at + size)
            .ok_or("the PLY body ends early")?;
        self.at += size;
        let mut buffer = [0_u8; 8];
        buffer[..size].copy_from_slice(raw);
        if self.encoding == Encoding::Big {
            buffer[..size].reverse();
        }
        let [b0, b1, b2, b3, ..] = buffer;
        Ok(match scalar {
            Scalar::I8 => f64::from(i8::from_le_bytes([b0])),
            Scalar::U8 => f64::from(b0),
            Scalar::I16 => f64::from(i16::from_le_bytes([b0, b1])),
            Scalar::U16 => f64::from(u16::from_le_bytes([b0, b1])),
            Scalar::I32 => f64::from(i32::from_le_bytes([b0, b1, b2, b3])),
            Scalar::U32 => f64::from(u32::from_le_bytes([b0, b1, b2, b3])),
            Scalar::F32 => f64::from(f32::from_le_bytes([b0, b1, b2, b3])),
            Scalar::F64 => f64::from_le_bytes(buffer),
        })
    }
}

pub(in super::super) fn parse(bytes: &[u8], mesh: &mut MeshBuilder) -> Result<(), String> {
    let (encoding, elements, body) = header(bytes)?;
    let text = if encoding == Encoding::Ascii {
        std::str::from_utf8(&bytes[body..]).map_err(|_| "the ASCII PLY body is not text")?
    } else {
        ""
    };
    let mut reader = Reader {
        encoding,
        bytes,
        at: body,
        words: text.split_ascii_whitespace(),
    };
    let mut vertices: Vec<Corner> = Vec::new();
    let mut corners: Vec<Corner> = Vec::new();
    for element in &elements {
        match element.name.as_str() {
            "vertex" => read_vertices(&mut reader, element, &mut vertices)?,
            "face" => {
                for _ in 0..element.count {
                    for property in &element.properties {
                        match &property.kind {
                            Kind::Scalar(scalar) => {
                                reader.read(*scalar)?;
                            }
                            Kind::List { count, item } => {
                                let length = index(reader.read(*count)?)
                                    .ok_or("a face's corner count is invalid")?;
                                let indices = matches!(
                                    property.name.as_str(),
                                    "vertex_indices" | "vertex_index"
                                );
                                corners.clear();
                                for _ in 0..length {
                                    let value = reader.read(*item)?;
                                    if indices {
                                        let corner =
                                            index(value).and_then(|at| vertices.get(at)).ok_or(
                                                "a face names a vertex that does not exist",
                                            )?;
                                        corners.push(*corner);
                                    }
                                }
                                if indices {
                                    mesh.polygon(&corners)?;
                                }
                            }
                        }
                    }
                }
            }
            _ => skip(&mut reader, element)?,
        }
    }
    Ok(())
}

fn read_vertices(
    reader: &mut Reader<'_>,
    element: &Element,
    vertices: &mut Vec<Corner>,
) -> Result<(), String> {
    // Every vertex costs at least one byte, so the count cannot outrun the file.
    if element.count > reader.bytes.len() {
        return Err("the PLY declares more vertices than it holds".to_owned());
    }
    vertices.reserve(element.count);
    let mut values = vec![0.0_f64; element.properties.len()];
    let slot = |name: &str| element.properties.iter().position(|p| p.name == name);
    let position = [slot("x"), slot("y"), slot("z")];
    let normal = [slot("nx"), slot("ny"), slot("nz")];
    let color = [slot("red"), slot("green"), slot("blue")];
    let color_scale = color[0].map_or(1.0, |at| match element.properties[at].kind {
        Kind::Scalar(scalar) if scalar.is_integer() => 255.0,
        _ => 1.0,
    });
    #[expect(clippy::cast_possible_truncation, reason = "f32 is the mesh precision")]
    let vector = |values: &[f64], slots: [Option<usize>; 3]| match slots {
        [Some(x), Some(y), Some(z)] => Some(Vec3::new(
            values[x] as f32,
            values[y] as f32,
            values[z] as f32,
        )),
        _ => None,
    };
    for _ in 0..element.count {
        for (value, property) in values.iter_mut().zip(&element.properties) {
            *value = match property.kind {
                Kind::Scalar(scalar) => reader.read(scalar)?,
                Kind::List { count, item } => {
                    let length = index(reader.read(count)?).ok_or("a list length is invalid")?;
                    for _ in 0..length {
                        reader.read(item)?;
                    }
                    0.0
                }
            };
        }
        vertices.push(Corner {
            position: vector(&values, position).ok_or("PLY vertices need x, y, and z")?,
            normal: vector(&values, normal),
            color: vector(&values, color).map(|rgb| {
                let channel = |c: f32| srgb_to_linear((c / color_scale).clamp(0.0, 1.0));
                Vec3::new(channel(rgb.x), channel(rgb.y), channel(rgb.z))
            }),
        });
    }
    Ok(())
}

fn skip(reader: &mut Reader<'_>, element: &Element) -> Result<(), String> {
    if element.count > reader.bytes.len() {
        return Err("the PLY declares more data than it holds".to_owned());
    }
    for _ in 0..element.count {
        for property in &element.properties {
            match property.kind {
                Kind::Scalar(scalar) => {
                    reader.read(scalar)?;
                }
                Kind::List { count, item } => {
                    let length = index(reader.read(count)?).ok_or("a list length is invalid")?;
                    for _ in 0..length {
                        reader.read(item)?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// A non-negative whole number as an index.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "checked whole and non-negative first"
)]
fn index(value: f64) -> Option<usize> {
    (value >= 0.0 && value.fract() == 0.0 && value < 4.0e9).then_some(value as usize)
}

/// The encoding, the elements, and where the body starts.
fn header(bytes: &[u8]) -> Result<(Encoding, Vec<Element>, usize), String> {
    let marker = b"end_header";
    let end = bytes
        .windows(marker.len())
        .take(64 * 1024)
        .position(|window| window == marker)
        .ok_or("the PLY header never ends")?;
    let mut body = end + marker.len();
    // The header line ends with `\n` or `\r\n`.
    if bytes.get(body) == Some(&b'\r') {
        body += 1;
    }
    if bytes.get(body) == Some(&b'\n') {
        body += 1;
    }
    let text = std::str::from_utf8(&bytes[..end]).map_err(|_| "the PLY header is not text")?;
    let mut encoding = None;
    let mut elements: Vec<Element> = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_ascii_whitespace().collect();
        match fields.as_slice() {
            ["format", format, ..] => {
                encoding = Some(match *format {
                    "ascii" => Encoding::Ascii,
                    "binary_little_endian" => Encoding::Little,
                    "binary_big_endian" => Encoding::Big,
                    other => return Err(format!("the PLY format `{other}` is unknown")),
                });
            }
            ["element", name, count] => elements.push(Element {
                name: (*name).to_owned(),
                count: count.parse().map_err(|_| "an element count is invalid")?,
                properties: Vec::new(),
            }),
            ["property", "list", count, item, name] => {
                let element = elements
                    .last_mut()
                    .ok_or("a property precedes every element")?;
                element.properties.push(Property {
                    name: (*name).to_owned(),
                    kind: Kind::List {
                        count: Scalar::parse(count).ok_or("a list count type is unknown")?,
                        item: Scalar::parse(item).ok_or("a list item type is unknown")?,
                    },
                });
            }
            ["property", scalar, name] => {
                let element = elements
                    .last_mut()
                    .ok_or("a property precedes every element")?;
                element.properties.push(Property {
                    name: (*name).to_owned(),
                    kind: Kind::Scalar(Scalar::parse(scalar).ok_or("a property type is unknown")?),
                });
            }
            _ => {}
        }
    }
    Ok((
        encoding.ok_or("the PLY header names no format")?,
        elements,
        body,
    ))
}
