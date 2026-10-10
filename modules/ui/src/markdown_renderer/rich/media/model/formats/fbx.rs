//! Autodesk FBX, binary (versions 7.x, the format every modern exporter
//! writes). ASCII FBX is refused with a note.
//!
//! The file is a tree of node records, each with typed properties; arrays
//! may be zlib-compressed. The block reads three parts of it:
//!
//! - `Objects`: `Geometry` meshes (`Vertices`, `PolygonVertexIndex`, the
//!   `LayerElementNormal` and `LayerElementMaterial` layers), `Model`s with
//!   their transform properties, and `Material` diffuse colors.
//! - `Connections`: geometry to model, model to parent model, material to
//!   model, in order (a polygon's material index counts the model's
//!   materials in connection order).
//! - `GlobalSettings`: the up axis.
//!
//! Each model's world transform follows the FBX SDK's chain of translation,
//! rotation offset and pivot, pre- and post-rotation, scaling offset and
//! pivot, and its geometric transform applies to its own geometry only.

use std::collections::HashMap;
use std::io::Read as _;

use super::super::mesh::{Corner, Mat4, MeshBuilder, Up, Vec3, transform_normal};

const MAGIC: &[u8] = b"Kaydara FBX Binary  \0";

/// The deepest node record or model hierarchy read.
const MAX_DEPTH: usize = 64;

/// The most array elements decompressed in total, so a small file cannot
/// claim gigabytes.
const MAX_ELEMENTS: usize = 64 * 1024 * 1024;

pub(in super::super) fn is_binary(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC)
}

/// Whether `bytes` look like an ASCII FBX, which starts with a `; FBX` comment.
pub(in super::super) fn is_ascii(bytes: &[u8]) -> bool {
    super::text_head(bytes).trim_start().starts_with("; FBX")
}

#[derive(Debug)]
enum Property {
    Integer(i64),
    Float(f64),
    Text(String),
    Floats(Vec<f64>),
    Integers(Vec<i64>),
    Other,
}

impl Property {
    fn integer(&self) -> Option<i64> {
        match self {
            Self::Integer(value) => Some(*value),
            #[expect(
                clippy::cast_possible_truncation,
                reason = "enum values stored as doubles"
            )]
            Self::Float(value) => Some(*value as i64),
            _ => None,
        }
    }

    fn float(&self) -> Option<f64> {
        match self {
            #[expect(clippy::cast_precision_loss, reason = "small integer properties")]
            Self::Integer(value) => Some(*value as f64),
            Self::Float(value) => Some(*value),
            _ => None,
        }
    }

    fn text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            _ => None,
        }
    }
}

#[derive(Debug)]
struct Node {
    name: String,
    properties: Vec<Property>,
    children: Vec<Node>,
}

impl Node {
    fn child(&self, name: &str) -> Option<&Self> {
        self.children.iter().find(|child| child.name == name)
    }

    fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Self> {
        self.children.iter().filter(move |child| child.name == name)
    }

    fn id(&self) -> Option<i64> {
        self.properties.first().and_then(Property::integer)
    }

    fn floats(&self) -> Option<&[f64]> {
        match self.properties.first() {
            Some(Property::Floats(values)) => Some(values),
            _ => None,
        }
    }

    fn integers(&self) -> Option<&[i64]> {
        match self.properties.first() {
            Some(Property::Integers(values)) => Some(values),
            _ => None,
        }
    }

    fn text(&self) -> Option<&str> {
        self.properties.first().and_then(Property::text)
    }

    /// A `Properties70` entry's values, after its name, type, label, and flags.
    fn property(&self, name: &str) -> Option<&[Property]> {
        self.child("Properties70")?
            .children_named("P")
            .find(|entry| entry.properties.first().and_then(Property::text) == Some(name))
            .and_then(|entry| entry.properties.get(4..))
    }

    fn vector(&self, name: &str) -> Option<Vec3> {
        #[expect(clippy::cast_possible_truncation, reason = "f32 is the mesh precision")]
        match self.property(name)? {
            [x, y, z, ..] => Some(Vec3::new(
                x.float()? as f32,
                y.float()? as f32,
                z.float()? as f32,
            )),
            _ => None,
        }
    }
}

/// Reads node records from the file.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    /// Whether offsets are 64-bit (version 7500 and later).
    wide: bool,
    /// Array elements read so far, against [`MAX_ELEMENTS`].
    elements: usize,
}

impl Reader<'_> {
    fn take(&mut self, length: usize) -> Result<&[u8], String> {
        let end = self
            .at
            .checked_add(length)
            .ok_or("an FBX record is too long")?;
        let bytes = self
            .bytes
            .get(self.at..end)
            .ok_or("the FBX file ends early")?;
        self.at = end;
        Ok(bytes)
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, String> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> Result<u64, String> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn offset(&mut self) -> Result<usize, String> {
        let value = if self.wide {
            self.u64()?
        } else {
            u64::from(self.u32()?)
        };
        usize::try_from(value).map_err(|_| "an FBX offset is too large".to_owned())
    }

    /// One node record, or `None` at the null record ending a list.
    fn node(&mut self, depth: usize) -> Result<Option<Node>, String> {
        if depth > MAX_DEPTH {
            return Err("the FBX nodes nest too deeply".to_owned());
        }
        let end = self.offset()?;
        let count = self.offset()?;
        let _list_length = self.offset()?;
        let name_length = usize::from(self.u8()?);
        if end == 0 {
            return Ok(None);
        }
        if end > self.bytes.len() || end < self.at {
            return Err("an FBX record points outside the file".to_owned());
        }
        let name = String::from_utf8_lossy(self.take(name_length)?).into_owned();
        // Each property takes at least one byte.
        if count > end - self.at {
            return Err("an FBX record claims more properties than it holds".to_owned());
        }
        let properties = (0..count)
            .map(|_| self.property())
            .collect::<Result<Vec<_>, _>>()?;
        let mut children = Vec::new();
        while self.at < end {
            match self.node(depth + 1)? {
                Some(child) => children.push(child),
                None => break,
            }
        }
        self.at = end;
        Ok(Some(Node {
            name,
            properties,
            children,
        }))
    }

    fn property(&mut self) -> Result<Property, String> {
        let code = self.u8()?;
        Ok(match code {
            b'C' => Property::Integer(i64::from(self.u8()?)),
            b'Y' => {
                let b = self.take(2)?;
                Property::Integer(i64::from(i16::from_le_bytes([b[0], b[1]])))
            }
            b'I' => Property::Integer(i64::from(self.u32()?.cast_signed())),
            b'L' => Property::Integer(self.u64()?.cast_signed()),
            b'F' => Property::Float(f64::from(f32::from_bits(self.u32()?))),
            b'D' => Property::Float(f64::from_bits(self.u64()?)),
            b'S' => {
                let length = usize::try_from(self.u32()?).map_err(|_| "a string is too long")?;
                Property::Text(String::from_utf8_lossy(self.take(length)?).into_owned())
            }
            b'R' => {
                let length = usize::try_from(self.u32()?).map_err(|_| "raw data is too long")?;
                self.take(length)?;
                Property::Other
            }
            b'f' | b'd' | b'i' | b'l' | b'b' | b'c' => self.array(code)?,
            other => {
                return Err(format!(
                    "the FBX holds an unknown property type `{}`",
                    char::from(other)
                ));
            }
        })
    }

    fn array(&mut self, code: u8) -> Result<Property, String> {
        let count = usize::try_from(self.u32()?).map_err(|_| "an array is too long")?;
        let encoding = self.u32()?;
        let length = usize::try_from(self.u32()?).map_err(|_| "an array is too long")?;
        let size = match code {
            b'd' | b'l' => 8,
            b'f' | b'i' => 4,
            _ => 1,
        };
        self.elements = self.elements.saturating_add(count);
        if self.elements > MAX_ELEMENTS {
            return Err("the FBX holds more data than a preview reads".to_owned());
        }
        let stored = self.take(length)?;
        let raw: std::borrow::Cow<'_, [u8]> = match encoding {
            0 => std::borrow::Cow::Borrowed(stored),
            1 => {
                let mut out = Vec::with_capacity(count * size);
                flate2::read::ZlibDecoder::new(stored)
                    .take((count * size) as u64)
                    .read_to_end(&mut out)
                    .map_err(|_| "a compressed FBX array is corrupt")?;
                std::borrow::Cow::Owned(out)
            }
            _ => return Err("an FBX array has an unknown encoding".to_owned()),
        };
        if raw.len() < count * size {
            return Err("an FBX array is shorter than it claims".to_owned());
        }
        let words = raw[..count * size].chunks_exact(size);
        Ok(match code {
            b'd' => Property::Floats(
                words
                    .map(|w| f64::from_le_bytes([w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7]]))
                    .collect(),
            ),
            b'f' => Property::Floats(
                words
                    .map(|w| f64::from(f32::from_le_bytes([w[0], w[1], w[2], w[3]])))
                    .collect(),
            ),
            b'i' => Property::Integers(
                words
                    .map(|w| i64::from(i32::from_le_bytes([w[0], w[1], w[2], w[3]])))
                    .collect(),
            ),
            b'l' => Property::Integers(
                words
                    .map(|w| i64::from_le_bytes([w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7]]))
                    .collect(),
            ),
            _ => Property::Other,
        })
    }
}

/// Parses a binary FBX into `mesh`; returns the file's up axis.
pub(in super::super) fn parse(bytes: &[u8], mesh: &mut MeshBuilder) -> Result<Up, String> {
    if !is_binary(bytes) {
        return Err(if is_ascii(bytes) {
            "ASCII FBX is not supported; export it as binary FBX".to_owned()
        } else {
            "the file is not an FBX".to_owned()
        });
    }
    let version = {
        let b = bytes.get(23..27).ok_or("the FBX header is cut short")?;
        u32::from_le_bytes([b[0], b[1], b[2], b[3]])
    };
    let mut reader = Reader {
        bytes,
        at: 27,
        wide: version >= 7500,
        elements: 0,
    };
    let mut roots = Vec::new();
    while reader.at < bytes.len() {
        match reader.node(0) {
            Ok(Some(node)) => roots.push(node),
            // The null record ends the top level; a footer follows.
            Ok(None) => break,
            // Damage after the parts the block reads does not matter.
            Err(_) if roots.iter().any(|root| root.name == "Connections") => break,
            Err(error) => return Err(error),
        }
    }
    let find = |name: &str| roots.iter().find(|root| root.name == name);
    let up = find("GlobalSettings")
        .and_then(|settings| settings.property("UpAxis"))
        .and_then(|values| values.first())
        .and_then(Property::integer)
        .map_or(Up::Y, |axis| if axis == 2 { Up::Z } else { Up::Y });
    let objects = find("Objects").ok_or("the FBX has no objects")?;
    Scene::new(objects, find("Connections")).build(mesh)?;
    Ok(up)
}

/// The parts of the object graph the block draws.
struct Scene<'a> {
    geometries: HashMap<i64, &'a Node>,
    models: HashMap<i64, &'a Node>,
    /// Every material, with its linear diffuse color when it has one.
    materials: HashMap<i64, Option<Vec3>>,
    /// Each model's parent model.
    parents: HashMap<i64, i64>,
    /// The models each geometry is instanced on.
    instances: Vec<(i64, i64)>,
    /// Each model's materials, in connection order.
    model_materials: HashMap<i64, Vec<i64>>,
}

impl<'a> Scene<'a> {
    fn new(objects: &'a Node, connections: Option<&'a Node>) -> Self {
        let mut scene = Self {
            geometries: HashMap::new(),
            models: HashMap::new(),
            materials: HashMap::new(),
            parents: HashMap::new(),
            instances: Vec::new(),
            model_materials: HashMap::new(),
        };
        for object in &objects.children {
            let Some(id) = object.id() else { continue };
            match object.name.as_str() {
                "Geometry" => {
                    scene.geometries.insert(id, object);
                }
                "Model" => {
                    scene.models.insert(id, object);
                }
                "Material" => {
                    let color = object
                        .vector("DiffuseColor")
                        .or_else(|| object.vector("Diffuse"));
                    scene.materials.insert(id, color);
                }
                _ => {}
            }
        }
        for link in connections.into_iter().flat_map(|c| c.children_named("C")) {
            let (Some(kind), Some(child), Some(parent)) = (
                link.properties.first().and_then(Property::text),
                link.properties.get(1).and_then(Property::integer),
                link.properties.get(2).and_then(Property::integer),
            ) else {
                continue;
            };
            if kind != "OO" {
                continue;
            }
            if scene.geometries.contains_key(&child) && scene.models.contains_key(&parent) {
                scene.instances.push((child, parent));
            } else if scene.models.contains_key(&child) && scene.models.contains_key(&parent) {
                scene.parents.insert(child, parent);
            } else if scene.models.contains_key(&parent) && scene.materials.contains_key(&child) {
                scene.model_materials.entry(parent).or_default().push(child);
            }
        }
        // A file without connections still shows its geometry, untransformed.
        if connections.is_none() {
            scene.instances = scene.geometries.keys().map(|id| (*id, 0)).collect();
        }
        scene
    }

    fn build(&self, mesh: &mut MeshBuilder) -> Result<(), String> {
        let mut instances = self.instances.clone();
        // Connection order is stable, but sort so output never depends on hashing.
        instances.sort_unstable();
        for (geometry, model) in instances {
            let Some(node) = self.geometries.get(&geometry) else {
                continue;
            };
            let (world, geometric) = match self.models.get(&model) {
                Some(model_node) => (self.world(model, 0)?, geometric(model_node)),
                None => (Mat4::IDENTITY, Mat4::IDENTITY),
            };
            let colors: Vec<Option<Vec3>> = self
                .model_materials
                .get(&model)
                .map(|ids| {
                    ids.iter()
                        .map(|id| self.materials.get(id).copied().flatten())
                        .collect()
                })
                .unwrap_or_default();
            geometry_into(node, world * geometric, &colors, mesh)?;
        }
        Ok(())
    }

    fn world(&self, model: i64, depth: usize) -> Result<Mat4, String> {
        if depth > MAX_DEPTH {
            return Err("the FBX model hierarchy is too deep or cyclic".to_owned());
        }
        let Some(node) = self.models.get(&model) else {
            return Ok(Mat4::IDENTITY);
        };
        let local = local(node);
        Ok(match self.parents.get(&model) {
            Some(parent) => self.world(*parent, depth + 1)? * local,
            None => local,
        })
    }
}

/// Euler angles in degrees, applied in FBX `order` (0 = XYZ, the default).
fn euler(angles: Vec3, order: i64) -> Mat4 {
    let x = Mat4::axis_rotation(0, angles.x);
    let y = Mat4::axis_rotation(1, angles.y);
    let z = Mat4::axis_rotation(2, angles.z);
    // The first named axis applies first, so it sits rightmost.
    match order {
        1 => y * z * x,
        2 => x * z * y,
        3 => z * x * y,
        4 => y * x * z,
        5 => x * y * z,
        _ => z * y * x,
    }
}

/// A model's local transform, by the FBX SDK's formula:
/// `T * Roff * Rp * Rpre * R * Rpost⁻¹ * Rp⁻¹ * Soff * Sp * S * Sp⁻¹`.
fn local(model: &Node) -> Mat4 {
    let vector = |name: &str, default: Vec3| model.vector(name).unwrap_or(default);
    let one = Vec3::new(1.0, 1.0, 1.0);
    let order = model
        .property("RotationOrder")
        .and_then(|values| values.first())
        .and_then(Property::integer)
        .unwrap_or(0);
    let translation = Mat4::translation(vector("Lcl Translation", Vec3::ZERO));
    let rotation = euler(vector("Lcl Rotation", Vec3::ZERO), order);
    let scale = Mat4::scale(vector("Lcl Scaling", one));
    let pre = euler(vector("PreRotation", Vec3::ZERO), 0);
    let post = euler(vector("PostRotation", Vec3::ZERO), 0);
    let post_inverse = transpose3(post);
    let rotation_pivot = vector("RotationPivot", Vec3::ZERO);
    let scaling_pivot = vector("ScalingPivot", Vec3::ZERO);
    translation
        * Mat4::translation(vector("RotationOffset", Vec3::ZERO))
        * Mat4::translation(rotation_pivot)
        * pre
        * rotation
        * post_inverse
        * Mat4::translation(-rotation_pivot)
        * Mat4::translation(vector("ScalingOffset", Vec3::ZERO))
        * Mat4::translation(scaling_pivot)
        * scale
        * Mat4::translation(-scaling_pivot)
}

/// The transform a model applies to its own geometry only.
fn geometric(model: &Node) -> Mat4 {
    let vector = |name: &str, default: Vec3| model.vector(name).unwrap_or(default);
    Mat4::translation(vector("GeometricTranslation", Vec3::ZERO))
        * euler(vector("GeometricRotation", Vec3::ZERO), 0)
        * Mat4::scale(vector("GeometricScaling", Vec3::new(1.0, 1.0, 1.0)))
}

/// The inverse of a pure rotation.
fn transpose3(matrix: Mat4) -> Mat4 {
    let mut out = Mat4::IDENTITY;
    for row in 0..3 {
        for column in 0..3 {
            out.rows[row][column] = matrix.rows[column][row];
        }
    }
    out
}

/// How a layer maps its values onto the mesh.
#[derive(Clone, Copy)]
enum Mapping {
    PolygonVertex,
    Vertex,
    Polygon,
    AllSame,
}

/// One layer element: its mapping, values, and optional index.
struct Layer<'a> {
    mapping: Mapping,
    index: Option<&'a [i64]>,
}

impl<'a> Layer<'a> {
    fn of(node: &'a Node, index_name: &str) -> Option<Self> {
        let mapping = match node.child("MappingInformationType")?.text()? {
            "ByPolygonVertex" => Mapping::PolygonVertex,
            "ByVertice" | "ByVertex" | "ByControlPoint" => Mapping::Vertex,
            "ByPolygon" => Mapping::Polygon,
            "AllSame" => Mapping::AllSame,
            _ => return None,
        };
        let indexed = node
            .child("ReferenceInformationType")
            .and_then(Node::text)
            .is_some_and(|kind| kind == "IndexToDirect" || kind == "Index");
        let index = if indexed {
            Some(node.child(index_name)?.integers()?)
        } else {
            None
        };
        Some(Self { mapping, index })
    }

    /// The value slot for corner `corner` of polygon `polygon` at control
    /// point `vertex`.
    fn slot(&self, corner: usize, vertex: usize, polygon: usize) -> Option<usize> {
        let direct = match self.mapping {
            Mapping::PolygonVertex => corner,
            Mapping::Vertex => vertex,
            Mapping::Polygon => polygon,
            Mapping::AllSame => 0,
        };
        match self.index {
            Some(index) => usize::try_from(*index.get(direct)?).ok(),
            None => Some(direct),
        }
    }
}

#[expect(clippy::cast_possible_truncation, reason = "f32 is the mesh precision")]
fn geometry_into(
    node: &Node,
    transform: Mat4,
    materials: &[Option<Vec3>],
    mesh: &mut MeshBuilder,
) -> Result<(), String> {
    let (Some(vertices), Some(polygons)) = (
        node.child("Vertices").and_then(Node::floats),
        node.child("PolygonVertexIndex").and_then(Node::integers),
    ) else {
        // NURBS, lines, and shapes carry no polygons.
        return Ok(());
    };
    let normal_matrix = transform.normal_matrix();
    let normals = node.child("LayerElementNormal").and_then(|layer| {
        Some((
            Layer::of(layer, "NormalsIndex")?,
            layer.child("Normals")?.floats()?,
        ))
    });
    let material_layer = node.child("LayerElementMaterial").and_then(|layer| {
        Some((
            Layer::of(layer, "Materials")?,
            layer.child("Materials")?.integers()?,
        ))
    });
    let point = |values: &[f64], at: usize| -> Option<Vec3> {
        let slice = values.get(at * 3..at * 3 + 3)?;
        Some(Vec3::new(slice[0] as f32, slice[1] as f32, slice[2] as f32))
    };
    let mut corners: Vec<Corner> = Vec::new();
    let mut polygon = 0;
    for (corner_index, raw) in polygons.iter().enumerate() {
        // A negative index ends its polygon and stores `-(index + 1)`.
        let (vertex, last) = if *raw < 0 {
            (!*raw, true)
        } else {
            (*raw, false)
        };
        let vertex = usize::try_from(vertex).map_err(|_| "a polygon index is invalid")?;
        let position =
            point(vertices, vertex).ok_or("a polygon names a vertex that does not exist")?;
        let normal = normals
            .as_ref()
            .and_then(|(layer, values)| point(values, layer.slot(corner_index, vertex, polygon)?));
        corners.push(Corner {
            position: transform.transform_point(position),
            normal: normal.map(|normal| transform_normal(&normal_matrix, normal)),
            color: None,
        });
        if last {
            let color = material_layer.as_ref().and_then(|(layer, values)| {
                // The material layer's values are the model's material slots.
                let slot = match layer.mapping {
                    Mapping::AllSame => 0,
                    _ => polygon,
                };
                let material = usize::try_from(*values.get(slot)?).ok()?;
                *materials.get(material)?
            });
            let color = color.or_else(|| materials.first().copied().flatten());
            for corner in &mut corners {
                corner.color = color;
            }
            mesh.polygon(&corners)?;
            corners.clear();
            polygon += 1;
        }
    }
    Ok(())
}
