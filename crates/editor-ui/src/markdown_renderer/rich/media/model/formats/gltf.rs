//! glTF 2.0: binary `.glb`, or `.gltf` JSON with its buffers embedded.
//!
//! The default scene's node tree is walked with each node's matrix or
//! translation, rotation, and scale; every triangle primitive (lists,
//! strips, and fans) contributes `POSITION`, `NORMAL` when present, and its
//! material's base color factor times `COLOR_0`. Textures are not drawn.
//! A `.gltf` whose buffers live in separate files cannot be read, because
//! a block fetches one file; such a model needs to be a `.glb`.

use std::collections::HashMap;

use base64::Engine as _;
use serde::Deserialize;

use super::super::mesh::{Corner, Mat4, MeshBuilder, Vec3, transform_normal};

const GLB_MAGIC: &[u8] = b"glTF";
const CHUNK_JSON: u32 = 0x4E4F_534A;
const CHUNK_BIN: u32 = 0x004E_4942;

/// The deepest node tree walked; deeper (or cyclic) trees are malformed.
const MAX_DEPTH: usize = 64;

pub(in super::super) fn is_glb(bytes: &[u8]) -> bool {
    bytes.starts_with(GLB_MAGIC)
}

/// Whether `bytes` read as glTF JSON.
pub(in super::super) fn is_json(bytes: &[u8]) -> bool {
    let head = super::text_head(bytes);
    head.trim_start().starts_with('{') && head.contains("\"asset\"")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Document {
    #[serde(default)]
    scene: Option<usize>,
    #[serde(default)]
    scenes: Vec<Scene>,
    #[serde(default)]
    nodes: Vec<Node>,
    #[serde(default)]
    meshes: Vec<MeshEntry>,
    #[serde(default)]
    accessors: Vec<Accessor>,
    #[serde(default)]
    buffer_views: Vec<BufferView>,
    #[serde(default)]
    buffers: Vec<Buffer>,
    #[serde(default)]
    materials: Vec<Material>,
    #[serde(default)]
    extensions_required: Vec<String>,
}

#[derive(Deserialize)]
struct Scene {
    #[serde(default)]
    nodes: Vec<usize>,
}

#[derive(Deserialize)]
struct Node {
    #[serde(default)]
    children: Vec<usize>,
    #[serde(default)]
    mesh: Option<usize>,
    #[serde(default)]
    matrix: Option<[f32; 16]>,
    #[serde(default)]
    translation: Option<[f32; 3]>,
    #[serde(default)]
    rotation: Option<[f32; 4]>,
    #[serde(default)]
    scale: Option<[f32; 3]>,
}

impl Node {
    fn local(&self) -> Mat4 {
        if let Some(matrix) = &self.matrix {
            return Mat4::from_column_major(matrix);
        }
        let [tx, ty, tz] = self.translation.unwrap_or([0.0; 3]);
        let [sx, sy, sz] = self.scale.unwrap_or([1.0; 3]);
        Mat4::translation(Vec3::new(tx, ty, tz))
            * Mat4::rotation(self.rotation.unwrap_or([0.0, 0.0, 0.0, 1.0]))
            * Mat4::scale(Vec3::new(sx, sy, sz))
    }
}

#[derive(Deserialize)]
struct MeshEntry {
    #[serde(default)]
    primitives: Vec<Primitive>,
}

#[derive(Deserialize)]
struct Primitive {
    attributes: HashMap<String, usize>,
    #[serde(default)]
    indices: Option<usize>,
    #[serde(default)]
    material: Option<usize>,
    #[serde(default = "triangles")]
    mode: u32,
}

const fn triangles() -> u32 {
    4
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Accessor {
    #[serde(default)]
    buffer_view: Option<usize>,
    #[serde(default)]
    byte_offset: usize,
    component_type: u32,
    #[serde(default)]
    normalized: bool,
    count: usize,
    #[serde(rename = "type")]
    shape: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BufferView {
    buffer: usize,
    #[serde(default)]
    byte_offset: usize,
    byte_length: usize,
    #[serde(default)]
    byte_stride: Option<usize>,
}

#[derive(Deserialize)]
struct Buffer {
    #[serde(default)]
    uri: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Material {
    #[serde(default)]
    pbr_metallic_roughness: Option<Pbr>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pbr {
    #[serde(default)]
    base_color_factor: Option<[f32; 4]>,
}

pub(in super::super) fn parse(bytes: &[u8], mesh: &mut MeshBuilder) -> Result<(), String> {
    let (json, binary) = if is_glb(bytes) {
        glb_chunks(bytes)?
    } else {
        (bytes, None)
    };
    let document: Document = serde_json::from_slice(json)
        .map_err(|error| format!("the glTF JSON is invalid: {error}"))?;
    if let Some(extension) = document
        .extensions_required
        .iter()
        .find(|name| name.contains("compression") || name.contains("draco"))
    {
        return Err(format!(
            "the glTF needs `{extension}`, which is not supported"
        ));
    }
    let buffers = document
        .buffers
        .iter()
        .enumerate()
        .map(|(index, buffer)| load_buffer(buffer, index, binary))
        .collect::<Result<Vec<_>, _>>()?;
    let reader = Reader {
        document: &document,
        buffers: &buffers,
    };
    let scene = document
        .scene
        .and_then(|scene| document.scenes.get(scene))
        .or_else(|| document.scenes.first());
    if let Some(scene) = scene {
        for root in &scene.nodes {
            reader.node(*root, Mat4::IDENTITY, 0, mesh)?;
        }
    } else {
        // No scenes: draw every mesh once, untransformed.
        for entry in &document.meshes {
            reader.mesh(entry, Mat4::IDENTITY, mesh)?;
        }
    }
    Ok(())
}

/// The JSON chunk and the binary chunk of a `.glb`.
fn glb_chunks(bytes: &[u8]) -> Result<(&[u8], Option<&[u8]>), String> {
    let word = |at: usize| {
        bytes
            .get(at..at + 4)
            .map(|word| u32::from_le_bytes([word[0], word[1], word[2], word[3]]))
    };
    if word(4) != Some(2) {
        return Err("only glTF 2.0 is supported".to_owned());
    }
    let mut at = 12;
    let mut json = None;
    let mut binary = None;
    while let (Some(length), Some(kind)) = (word(at), word(at + 4)) {
        let length = usize::try_from(length).map_err(|_| "a GLB chunk is too long")?;
        let start = at + 8;
        let chunk = bytes
            .get(start..start.saturating_add(length))
            .ok_or("a GLB chunk runs past the file")?;
        match kind {
            CHUNK_JSON => json = Some(chunk),
            CHUNK_BIN => binary = Some(chunk),
            _ => {}
        }
        at = start + length;
    }
    Ok((json.ok_or("the GLB has no JSON chunk")?, binary))
}

fn load_buffer<'a>(
    buffer: &Buffer,
    index: usize,
    binary: Option<&'a [u8]>,
) -> Result<std::borrow::Cow<'a, [u8]>, String> {
    match buffer.uri.as_deref() {
        None => binary
            .map(std::borrow::Cow::Borrowed)
            .ok_or_else(|| format!("buffer {index} has no data")),
        Some(uri) if uri.starts_with("data:") => {
            let (_, data) = uri
                .split_once(";base64,")
                .ok_or_else(|| format!("buffer {index} is not base64 data"))?;
            base64::engine::general_purpose::STANDARD
                .decode(data)
                .map(std::borrow::Cow::Owned)
                .map_err(|_| format!("buffer {index} is not valid base64"))
        }
        Some(uri) => Err(format!(
            "the glTF keeps its geometry in `{uri}`; export it as .glb to show it here"
        )),
    }
}

struct Reader<'a> {
    document: &'a Document,
    buffers: &'a [std::borrow::Cow<'a, [u8]>],
}

impl Reader<'_> {
    fn node(
        &self,
        index: usize,
        parent: Mat4,
        depth: usize,
        mesh: &mut MeshBuilder,
    ) -> Result<(), String> {
        if depth > MAX_DEPTH {
            return Err("the glTF node tree is too deep or cyclic".to_owned());
        }
        let node = self
            .document
            .nodes
            .get(index)
            .ok_or_else(|| format!("node {index} does not exist"))?;
        let world = parent * node.local();
        if let Some(entry) = node.mesh {
            let entry = self
                .document
                .meshes
                .get(entry)
                .ok_or_else(|| format!("mesh {entry} does not exist"))?;
            self.mesh(entry, world, mesh)?;
        }
        for child in &node.children {
            self.node(*child, world, depth + 1, mesh)?;
        }
        Ok(())
    }

    fn mesh(&self, entry: &MeshEntry, world: Mat4, mesh: &mut MeshBuilder) -> Result<(), String> {
        let normal_matrix = world.normal_matrix();
        for primitive in &entry.primitives {
            if !matches!(primitive.mode, 4..=6) {
                // Points and lines have no surface to shade.
                continue;
            }
            let Some(position) = primitive.attributes.get("POSITION") else {
                continue;
            };
            let positions = self.vectors(*position)?;
            let normals = match primitive.attributes.get("NORMAL") {
                Some(accessor) => Some(self.vectors(*accessor)?),
                None => None,
            };
            let colors = match primitive.attributes.get("COLOR_0") {
                Some(accessor) => Some(self.vectors(*accessor)?),
                None => None,
            };
            let factor = primitive
                .material
                .and_then(|material| self.document.materials.get(material))
                .and_then(|material| material.pbr_metallic_roughness.as_ref())
                .and_then(|pbr| pbr.base_color_factor)
                .map(|[r, g, b, _]| Vec3::new(r, g, b));
            let indices = match primitive.indices {
                Some(accessor) => self.indices(accessor)?,
                None => (0..positions.len()).collect(),
            };
            let corner = |index: usize| -> Result<Corner, String> {
                let position = *positions
                    .get(index)
                    .ok_or("a glTF index points past its vertices")?;
                let color = colors
                    .as_ref()
                    .and_then(|colors| colors.get(index).copied());
                let color = match (color, factor) {
                    (Some(c), Some(f)) => Some(Vec3::new(c.x * f.x, c.y * f.y, c.z * f.z)),
                    (color, factor) => color.or(factor),
                };
                Ok(Corner {
                    position: world.transform_point(position),
                    normal: normals
                        .as_ref()
                        .and_then(|normals| normals.get(index))
                        .map(|normal| transform_normal(&normal_matrix, *normal)),
                    color,
                })
            };
            for [a, b, c] in assemble(primitive.mode, &indices) {
                mesh.triangle([corner(a)?, corner(b)?, corner(c)?])?;
            }
        }
        Ok(())
    }

    /// The bytes of `accessor`'s elements: each element's start and the
    /// stride between them.
    fn layout(&self, accessor: &Accessor, element: usize) -> Result<(&[u8], usize), String> {
        let view_index = accessor
            .buffer_view
            .ok_or("a sparse or empty glTF accessor is not supported")?;
        let view = self
            .document
            .buffer_views
            .get(view_index)
            .ok_or_else(|| format!("buffer view {view_index} does not exist"))?;
        let buffer = self
            .buffers
            .get(view.buffer)
            .ok_or_else(|| format!("buffer {} does not exist", view.buffer))?;
        let start = view.byte_offset.saturating_add(accessor.byte_offset);
        let end = view.byte_offset.saturating_add(view.byte_length);
        let bytes = buffer
            .get(start..end.min(buffer.len()))
            .ok_or("a glTF accessor runs past its buffer")?;
        let stride = view.byte_stride.unwrap_or(element).max(element);
        let needed = accessor
            .count
            .checked_sub(1)
            .map_or(Some(0), |last| {
                last.checked_mul(stride)?.checked_add(element)
            })
            .ok_or("a glTF accessor is too large")?;
        if needed > bytes.len() {
            return Err("a glTF accessor runs past its buffer view".to_owned());
        }
        Ok((bytes, stride))
    }

    /// Reads a `VEC3` or `VEC4` accessor's first three components.
    fn vectors(&self, index: usize) -> Result<Vec<Vec3>, String> {
        let accessor = self.accessor(index)?;
        let components = match accessor.shape.as_str() {
            "VEC3" => 3,
            "VEC4" => 4,
            other => return Err(format!("a vector accessor has type `{other}`")),
        };
        let size = component_size(accessor.component_type)?;
        let (bytes, stride) = self.layout(accessor, components * size)?;
        let read = |at: usize| component(bytes, at, accessor.component_type, accessor.normalized);
        (0..accessor.count)
            .map(|item| {
                let at = item * stride;
                Ok(Vec3::new(read(at)?, read(at + size)?, read(at + 2 * size)?))
            })
            .collect()
    }

    fn indices(&self, index: usize) -> Result<Vec<usize>, String> {
        let accessor = self.accessor(index)?;
        let size = component_size(accessor.component_type)?;
        let (bytes, stride) = self.layout(accessor, size)?;
        (0..accessor.count)
            .map(|item| {
                let at = item * stride;
                let raw = bytes
                    .get(at..at + size)
                    .ok_or("an index runs past its buffer")?;
                Ok(match raw {
                    [a] => usize::from(*a),
                    [a, b] => usize::from(u16::from_le_bytes([*a, *b])),
                    [a, b, c, d] => usize::try_from(u32::from_le_bytes([*a, *b, *c, *d]))
                        .map_err(|_| "an index is too large")?,
                    _ => return Err("indices must be unsigned integers".to_owned()),
                })
            })
            .collect()
    }

    fn accessor(&self, index: usize) -> Result<&Accessor, String> {
        self.document
            .accessors
            .get(index)
            .ok_or_else(|| format!("accessor {index} does not exist"))
    }
}

fn component_size(kind: u32) -> Result<usize, String> {
    Ok(match kind {
        5120 | 5121 => 1,
        5122 | 5123 => 2,
        5125 | 5126 => 4,
        other => return Err(format!("component type {other} is unknown")),
    })
}

/// One component as `f32`; normalized integers map to `0..=1` or `-1..=1`.
fn component(bytes: &[u8], at: usize, kind: u32, normalized: bool) -> Result<f32, String> {
    let raw = bytes
        .get(at..at + component_size(kind)?)
        .ok_or("a component runs past its buffer")?;
    #[expect(
        clippy::cast_precision_loss,
        reason = "u32 components are rare and coarse"
    )]
    Ok(match (kind, raw) {
        (5126, [a, b, c, d]) => f32::from_le_bytes([*a, *b, *c, *d]),
        (5121, [a]) if normalized => f32::from(*a) / 255.0,
        (5121, [a]) => f32::from(*a),
        (5120, [a]) if normalized => (f32::from(i8::from_le_bytes([*a])) / 127.0).max(-1.0),
        (5120, [a]) => f32::from(i8::from_le_bytes([*a])),
        (5123, [a, b]) if normalized => f32::from(u16::from_le_bytes([*a, *b])) / 65535.0,
        (5123, [a, b]) => f32::from(u16::from_le_bytes([*a, *b])),
        (5122, [a, b]) if normalized => {
            (f32::from(i16::from_le_bytes([*a, *b])) / 32767.0).max(-1.0)
        }
        (5122, [a, b]) => f32::from(i16::from_le_bytes([*a, *b])),
        (5125, [a, b, c, d]) => u32::from_le_bytes([*a, *b, *c, *d]) as f32,
        _ => return Err("a component has the wrong size".to_owned()),
    })
}

/// Triangles from an index list in `mode`: 4 lists, 5 strips, 6 fans.
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
