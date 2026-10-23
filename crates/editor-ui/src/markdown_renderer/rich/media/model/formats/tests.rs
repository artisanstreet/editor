//! Model formats: recognizing, parsing, and normalizing.

use std::io::Write as _;

use base64::Engine as _;

use super::super::mesh::{Up, Vec3};
use super::super::raster::{Camera, Look, render};
use super::{Format, Model, parse};

fn model(bytes: &[u8]) -> Model {
    parse(bytes, None, None).unwrap_or_else(|error| panic!("expected a model, got {error}"))
}

fn assert_unit_sphere(model: &Model) {
    let farthest = model
        .mesh
        .positions
        .iter()
        .map(|point| point.length())
        .fold(0.0_f32, f32::max);
    assert!(
        (farthest - 1.0).abs() < 1e-4,
        "farthest corner at {farthest}"
    );
}

const QUAD_OBJ: &str = "\
# a unit quad
v 0 0 0 1 0 0
v 2 0 0
v 2 2 0
v 0 2 0
vn 0 0 1
f -4//1 -3//1 -2//1 -1//1
";

#[test]
fn obj_fans_polygons_and_resolves_negative_indices() {
    let model = model(QUAD_OBJ.as_bytes());
    assert_eq!(model.format, Format::Obj);
    assert_eq!(model.mesh.triangles(), 2);
    assert_unit_sphere(&model);
    assert!(
        model
            .mesh
            .normals
            .iter()
            .all(|normal| (normal.z - 1.0).abs() < 1e-6)
    );
    let colors = model.mesh.colors.as_ref().expect("the first vertex is red");
    assert_eq!(colors[0], Vec3::new(1.0, 0.0, 0.0));
}

#[test]
fn obj_names_the_line_of_a_bad_face() {
    let error = parse(b"v 0 0 0\nv 1 0 0\nf 1 2 9\n", None, None).unwrap_err();
    assert!(error.contains("line 3"), "{error}");
}

fn binary_stl(triangles: &[[[f32; 3]; 3]]) -> Vec<u8> {
    let mut bytes = vec![0_u8; 80];
    bytes.extend_from_slice(&u32::try_from(triangles.len()).unwrap().to_le_bytes());
    for triangle in triangles {
        bytes.extend_from_slice(&[0_u8; 12]);
        for corner in triangle {
            for value in corner {
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        bytes.extend_from_slice(&[0, 0]);
    }
    bytes
}

#[test]
fn stl_reads_binary_and_ascii_and_stands_z_up() {
    let triangle = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]];
    let binary = model(&binary_stl(&[triangle]));
    assert_eq!(binary.format, Format::Stl);
    assert_eq!(binary.mesh.triangles(), 1);
    // Z up turns Y up: the triangle's tall side now rises along Y.
    let rise = binary.mesh.positions[2].y - binary.mesh.positions[0].y;
    assert!(rise > 0.5, "{rise}");

    let ascii = "solid t\nfacet normal 0 0 0\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid t\n";
    assert_eq!(model(ascii.as_bytes()).mesh.triangles(), 1);
}

#[test]
fn a_binary_stl_whose_header_says_solid_stays_binary() {
    let mut bytes = binary_stl(&[[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]]);
    bytes[..5].copy_from_slice(b"solid");
    assert_eq!(model(&bytes).mesh.triangles(), 1);
}

#[test]
fn ply_reads_ascii_and_binary_with_colors() {
    let ascii = "ply\nformat ascii 1.0\nelement vertex 3\nproperty float x\nproperty float y\nproperty float z\nproperty uchar red\nproperty uchar green\nproperty uchar blue\nelement face 1\nproperty list uchar int vertex_indices\nend_header\n0 0 0 255 0 0\n1 0 0 255 0 0\n0 1 0 255 0 0\n3 0 1 2\n";
    let parsed = model(ascii.as_bytes());
    assert_eq!(parsed.format, Format::Ply);
    assert_eq!(parsed.mesh.triangles(), 1);
    assert_eq!(
        parsed.mesh.colors.as_ref().unwrap()[1],
        Vec3::new(1.0, 0.0, 0.0)
    );

    let mut binary = b"ply\nformat binary_little_endian 1.0\nelement vertex 4\nproperty float x\nproperty float y\nproperty float z\nelement face 1\nproperty list uchar uint vertex_indices\nend_header\n".to_vec();
    for corner in [
        [0.0_f32, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
    ] {
        for value in corner {
            binary.extend_from_slice(&value.to_le_bytes());
        }
    }
    binary.push(4);
    for index in 0_u32..4 {
        binary.extend_from_slice(&index.to_le_bytes());
    }
    assert_eq!(model(&binary).mesh.triangles(), 2);
}

/// A glTF document with one triangle, its buffer at `uri` (or in the GLB).
fn gltf_json(uri: Option<&str>, length: usize) -> String {
    let uri = uri
        .map(|uri| format!(r#", "uri": "{uri}""#))
        .unwrap_or_default();
    format!(
        r#"{{"asset": {{"version": "2.0"}}, "scene": 0, "scenes": [{{"nodes": [0]}}],
        "nodes": [{{"children": [1], "scale": [2, 2, 2]}}, {{"mesh": 0, "translation": [5, 0, 0]}}],
        "meshes": [{{"primitives": [{{"attributes": {{"POSITION": 0}}, "material": 0}}]}}],
        "materials": [{{"pbrMetallicRoughness": {{"baseColorFactor": [0.5, 0.25, 1, 1]}}}}],
        "accessors": [{{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0,0,0], "max": [1,1,0]}}],
        "bufferViews": [{{"buffer": 0, "byteLength": {length}}}],
        "buffers": [{{"byteLength": {length}{uri}}}]}}"#
    )
}

fn triangle_buffer() -> Vec<u8> {
    [0.0_f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn glb(json: &str, binary: &[u8]) -> Vec<u8> {
    let mut json = json.as_bytes().to_vec();
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let mut bytes = b"glTF".to_vec();
    bytes.extend_from_slice(&2_u32.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&u32::try_from(json.len()).unwrap().to_le_bytes());
    bytes.extend_from_slice(b"JSON");
    bytes.extend_from_slice(&json);
    bytes.extend_from_slice(&u32::try_from(binary.len()).unwrap().to_le_bytes());
    bytes.extend_from_slice(b"BIN\0");
    bytes.extend_from_slice(binary);
    let length = u32::try_from(bytes.len()).unwrap();
    bytes[8..12].copy_from_slice(&length.to_le_bytes());
    bytes
}

#[test]
fn malformed_glb_lengths_are_errors_without_library_underflow() {
    let buffer = triangle_buffer();
    let valid = glb(&gltf_json(None, buffer.len()), &buffer);
    for length in [0u32, 1, 11, 12, u32::MAX] {
        let mut bytes = valid.clone();
        bytes[8..12].copy_from_slice(&length.to_le_bytes());
        assert!(
            parse(&bytes, Some(Format::Gltf), None)
                .unwrap_err()
                .contains("length")
        );
    }
    for end in 4..12 {
        assert!(parse(&valid[..end], Some(Format::Gltf), None).is_err());
    }
}

#[test]
fn glb_walks_the_node_tree_and_takes_material_colors() {
    let buffer = triangle_buffer();
    let parsed = model(&glb(&gltf_json(None, buffer.len()), &buffer));
    assert_eq!(parsed.format, Format::Gltf);
    assert_eq!(parsed.mesh.triangles(), 1);
    assert_unit_sphere(&parsed);
    assert_eq!(
        parsed.mesh.colors.as_ref().unwrap()[0],
        Vec3::new(0.5, 0.25, 1.0)
    );
}

#[test]
fn gltf_reads_embedded_buffers_and_explains_external_ones() {
    let buffer = triangle_buffer();
    let data = format!(
        "data:application/octet-stream;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&buffer)
    );
    let embedded = gltf_json(Some(&data), buffer.len());
    assert_eq!(model(embedded.as_bytes()).mesh.triangles(), 1);

    let external = gltf_json(Some("scene.bin"), buffer.len());
    let error = parse(external.as_bytes(), None, None).unwrap_err();
    assert!(
        error.contains("scene.bin") && error.contains(".glb"),
        "{error}"
    );
}

/// Writes FBX 7.4 node records.
#[derive(Default)]
struct FbxWriter {
    bytes: Vec<u8>,
}

enum Value<'a> {
    Int(i32),
    Long(i64),
    Double(f64),
    Text(&'a str),
    Doubles(&'a [f64]),
    /// Stored zlib-compressed, as exporters store large arrays.
    Ints(&'a [i32]),
}

impl FbxWriter {
    fn new() -> Self {
        let mut bytes = b"Kaydara FBX Binary  \0\x1a\0".to_vec();
        bytes.extend_from_slice(&7400_u32.to_le_bytes());
        Self { bytes }
    }

    fn node(&mut self, name: &str, values: &[Value<'_>], children: impl FnOnce(&mut Self)) {
        let start = self.bytes.len();
        self.bytes.extend_from_slice(&[0; 12]);
        self.bytes.push(u8::try_from(name.len()).unwrap());
        self.bytes.extend_from_slice(name.as_bytes());
        let properties = self.bytes.len();
        for value in values {
            self.value(value);
        }
        let list = u32::try_from(self.bytes.len() - properties).unwrap();
        let before = self.bytes.len();
        children(self);
        if self.bytes.len() != before {
            self.bytes.extend_from_slice(&[0; 13]);
        }
        let end = u32::try_from(self.bytes.len()).unwrap();
        self.bytes[start..start + 4].copy_from_slice(&end.to_le_bytes());
        let count = u32::try_from(values.len()).unwrap();
        self.bytes[start + 4..start + 8].copy_from_slice(&count.to_le_bytes());
        self.bytes[start + 8..start + 12].copy_from_slice(&list.to_le_bytes());
    }

    fn value(&mut self, value: &Value<'_>) {
        let bytes = &mut self.bytes;
        match value {
            Value::Int(v) => {
                bytes.push(b'I');
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            Value::Long(v) => {
                bytes.push(b'L');
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            Value::Double(v) => {
                bytes.push(b'D');
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            Value::Text(text) => {
                bytes.push(b'S');
                bytes.extend_from_slice(&u32::try_from(text.len()).unwrap().to_le_bytes());
                bytes.extend_from_slice(text.as_bytes());
            }
            Value::Doubles(values) => {
                bytes.push(b'd');
                bytes.extend_from_slice(&u32::try_from(values.len()).unwrap().to_le_bytes());
                bytes.extend_from_slice(&0_u32.to_le_bytes());
                bytes.extend_from_slice(&u32::try_from(values.len() * 8).unwrap().to_le_bytes());
                for v in *values {
                    bytes.extend_from_slice(&v.to_le_bytes());
                }
            }
            Value::Ints(values) => {
                let raw: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
                let mut encoder =
                    flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(&raw).unwrap();
                let compressed = encoder.finish().unwrap();
                bytes.push(b'i');
                bytes.extend_from_slice(&u32::try_from(values.len()).unwrap().to_le_bytes());
                bytes.extend_from_slice(&1_u32.to_le_bytes());
                bytes.extend_from_slice(&u32::try_from(compressed.len()).unwrap().to_le_bytes());
                bytes.extend_from_slice(&compressed);
            }
        }
    }

    fn property(&mut self, name: &str, values: &[Value<'_>]) {
        let mut all = vec![
            Value::Text(name),
            Value::Text(""),
            Value::Text(""),
            Value::Text("A"),
        ];
        all.extend(values.iter().map(|value| match value {
            Value::Int(v) => Value::Int(*v),
            Value::Double(v) => Value::Double(*v),
            Value::Long(_) | Value::Text(_) | Value::Doubles(_) | Value::Ints(_) => {
                unreachable!("properties hold numbers")
            }
        }));
        self.node("P", &all, |_| {});
    }

    fn finish(mut self) -> Vec<u8> {
        self.bytes.extend_from_slice(&[0; 13]);
        self.bytes
    }
}

/// A quad on a model moved 10 along X, in a Z-up file, with one red material.
fn fbx_quad() -> Vec<u8> {
    let mut fbx = FbxWriter::new();
    fbx.node("GlobalSettings", &[], |w| {
        w.node("Properties70", &[], |w| {
            w.property("UpAxis", &[Value::Int(2)]);
            // Historical authoring-axis metadata must not override the file's
            // current geometry axes.
            w.property("OriginalUpAxis", &[Value::Int(1)]);
        });
    });
    fbx.node("Objects", &[], |w| {
        w.node(
            "Geometry",
            &[Value::Long(1), Value::Text("Quad"), Value::Text("Mesh")],
            |w| {
                w.node(
                    "Vertices",
                    &[Value::Doubles(&[
                        0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0,
                    ])],
                    |_| {},
                );
                w.node("PolygonVertexIndex", &[Value::Ints(&[0, 1, 2, -4])], |_| {});
            },
        );
        w.node(
            "Model",
            &[Value::Long(2), Value::Text("Quad"), Value::Text("Mesh")],
            |w| {
                w.node("Properties70", &[], |w| {
                    w.property(
                        "Lcl Translation",
                        &[Value::Double(10.0), Value::Double(0.0), Value::Double(0.0)],
                    );
                });
            },
        );
        w.node(
            "Material",
            &[Value::Long(3), Value::Text("Red"), Value::Text("")],
            |w| {
                w.node("Properties70", &[], |w| {
                    w.property(
                        "DiffuseColor",
                        &[Value::Double(1.0), Value::Double(0.0), Value::Double(0.0)],
                    );
                });
            },
        );
    });
    fbx.node("Connections", &[], |w| {
        w.node(
            "C",
            &[Value::Text("OO"), Value::Long(2), Value::Long(0)],
            |_| {},
        );
        w.node(
            "C",
            &[Value::Text("OO"), Value::Long(1), Value::Long(2)],
            |_| {},
        );
        w.node(
            "C",
            &[Value::Text("OO"), Value::Long(3), Value::Long(2)],
            |_| {},
        );
    });
    fbx.finish()
}

#[test]
fn fbx_reads_compressed_geometry_models_and_materials() {
    let parsed = model(&fbx_quad());
    assert_eq!(parsed.format, Format::Fbx);
    assert_eq!(parsed.mesh.triangles(), 2);
    assert_unit_sphere(&parsed);
    assert_eq!(
        parsed.mesh.colors.as_ref().unwrap()[0],
        Vec3::new(1.0, 0.0, 0.0)
    );
    // The file says Z up: the quad, flat in XY, now stands in XZ.
    assert!(parsed.mesh.positions.iter().all(|p| p.y.abs() < 1e-6));
    // The spec's `up` wins over the file's.
    let upright = parse(&fbx_quad(), None, Some(Up::Y)).unwrap();
    assert!(upright.mesh.positions.iter().all(|p| p.z.abs() < 1e-6));
}

#[test]
fn ascii_fbx_and_unknown_bytes_explain_themselves() {
    let error = parse(b"; FBX 7.4.0 project file\n", None, None).unwrap_err();
    assert!(error.contains("ASCII FBX"), "{error}");
    let error = parse(&[0, 1, 2, 3], None, None).unwrap_err();
    assert!(error.contains("OBJ, FBX"), "{error}");
}

#[test]
#[expect(
    clippy::let_underscore_must_use,
    reason = "a cut file may fail or not; it must not panic"
)]
fn a_truncated_fbx_fails_without_panicking() {
    let bytes = fbx_quad();
    for cut in (27..bytes.len()).step_by(7) {
        let _ = parse(&bytes[..cut], None, None);
    }
}

#[test]
fn the_rasterizer_draws_the_model_on_a_transparent_frame() {
    let parsed = model(QUAD_OBJ.as_bytes());
    let look = Look {
        base: Vec3::new(0.5, 0.5, 0.5),
        edges: None,
    };
    let camera = Camera::new(0.0, 0.0);
    let frame = render(&parsed.mesh, &camera, &look, 64, 48, 2);
    assert_eq!(frame.pixels.len(), 64 * 48 * 4);
    let alpha = |x: usize, y: usize| frame.pixels[(y * 64 + x) * 4 + 3];
    assert_eq!(alpha(32, 24), 255, "the quad covers the center");
    assert_eq!(alpha(0, 0), 0, "the corner stays transparent");
    // Seen from behind, the two-sided quad still draws.
    let behind = render(&parsed.mesh, &Camera::new(180.0, 0.0), &look, 64, 48, 1);
    assert_eq!(behind.pixels[(24 * 64 + 32) * 4 + 3], 255);
}

#[test]
fn tiny_ply_headers_cannot_request_unbounded_empty_records() {
    let bytes = b"ply\nformat ascii 1.0\nelement face 10000000\nend_header\n";
    assert!(
        parse(bytes, Some(Format::Ply), None)
            .unwrap_err()
            .contains("budget")
    );
}

#[test]
fn fbx_record_name_must_fit_inside_its_declared_end() {
    let mut bytes = b"Kaydara FBX Binary  \0\x1a\0".to_vec();
    bytes.extend_from_slice(&7400u32.to_le_bytes());
    bytes.extend_from_slice(&40u32.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.push(1);
    bytes.push(b'x');
    assert!(parse(&bytes, Some(Format::Fbx), None).is_err());
}

#[test]
fn renderer_capability_keeps_only_the_needed_geometry_representation() {
    let gpu = super::parse_for_renderer(QUAD_OBJ.as_bytes(), None, None, true).unwrap();
    assert_eq!(gpu.triangles, 2);
    assert!(gpu.gpu.is_some());
    assert!(gpu.mesh.positions.is_empty() && gpu.mesh.normals.is_empty());
    let cpu = super::parse_for_renderer(QUAD_OBJ.as_bytes(), None, None, false).unwrap();
    assert_eq!(cpu.triangles, 2);
    assert!(cpu.gpu.is_none());
    assert_eq!(cpu.mesh.triangles(), 2);
}
