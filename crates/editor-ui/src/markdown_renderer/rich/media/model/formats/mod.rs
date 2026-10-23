//! The model formats, recognized by their bytes.
//!
//! Binary formats carry a signature (`glTF`, `Kaydara FBX Binary`, `ply`);
//! binary STL is recognized by its length matching its triangle count. Text
//! formats are told apart by their first lines, and OBJ, which has no
//! signature, is whatever text is left. A spec's `format` overrides the
//! guess.

use std::sync::Arc;

use serde::Deserialize;

use super::mesh::{Mesh, MeshBuilder, Up};

mod fbx;
mod gltf;
mod obj;
mod ply;
mod scene;
mod stl;

/// A model file format.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(in super::super) enum Format {
    Obj,
    Fbx,
    Stl,
    Ply,
    #[serde(alias = "glb")]
    Gltf,
}

impl Format {
    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Obj => "OBJ",
            Self::Fbx => "FBX",
            Self::Stl => "STL",
            Self::Ply => "PLY",
            Self::Gltf => "glTF",
        }
    }

    /// The up axis files of this format use unless they say otherwise:
    /// STL comes from CAD and printing, which are Z up.
    const fn default_up(self) -> Up {
        match self {
            Self::Stl => Up::Z,
            Self::Obj | Self::Fbx | Self::Ply | Self::Gltf => Up::Y,
        }
    }

    /// The format `bytes` are in, if any.
    pub(super) fn sniff(bytes: &[u8]) -> Option<Self> {
        if gltf::is_glb(bytes) || gltf::is_json(bytes) {
            Some(Self::Gltf)
        } else if fbx::is_binary(bytes) || fbx::is_ascii(bytes) {
            Some(Self::Fbx)
        } else if bytes.starts_with(b"ply") {
            Some(Self::Ply)
        } else if stl::is_binary(bytes) || stl::is_ascii(bytes) {
            Some(Self::Stl)
        } else if looks_like_obj(bytes) {
            Some(Self::Obj)
        } else {
            None
        }
    }
}

/// A parsed model and what the toolbar says about it.
#[derive(Debug)]
pub(in super::super) struct Model {
    pub(in super::super) mesh: Mesh,
    /// The mesh as GPU geometry, for windows whose renderer draws meshes.
    pub(in super::super) gpu: Option<Arc<gpui::Mesh>>,
    pub(in super::super) triangles: usize,
    pub(in super::super) format: Format,
}

impl Model {
    pub(in super::super) fn resident_bytes(&self) -> usize {
        self.mesh.positions.capacity() * std::mem::size_of::<super::mesh::Vec3>()
            + self.mesh.normals.capacity() * std::mem::size_of::<super::mesh::Vec3>()
            + self.mesh.colors.as_ref().map_or(0, |colors| {
                colors.capacity() * std::mem::size_of::<super::mesh::Vec3>()
            })
            + self
                .gpu
                .as_ref()
                .map_or(0, |gpu| std::mem::size_of_val(gpu.vertices()))
    }
}

/// Parses `bytes` as `format`, or as the format they look like. `up`
/// overrides the file's own up axis.
#[cfg(test)]
pub(super) fn parse(bytes: &[u8], format: Option<Format>, up: Option<Up>) -> Result<Model, String> {
    parse_for_renderer(bytes, format, up, false)
}

pub(super) fn parse_for_renderer(
    bytes: &[u8],
    format: Option<Format>,
    up: Option<Up>,
    gpu_enabled: bool,
) -> Result<Model, String> {
    let format = format
        .or_else(|| Format::sniff(bytes))
        .ok_or("the file is not a model format the viewer reads (OBJ, FBX, STL, PLY, glTF)")?;
    let mut builder = MeshBuilder::default();
    let file_up = match format {
        Format::Fbx => fbx::parse(bytes, &mut builder)?,
        Format::Gltf => {
            gltf::parse(bytes, &mut builder)?;
            format.default_up()
        }
        Format::Stl => {
            stl::parse(bytes, &mut builder)?;
            format.default_up()
        }
        Format::Ply => {
            ply::parse(bytes, &mut builder)?;
            format.default_up()
        }
        Format::Obj => {
            let text = std::str::from_utf8(bytes).map_err(|_not_utf8| "the OBJ is not text")?;
            obj::parse(text, &mut builder)?;
            format.default_up()
        }
    };
    let mut mesh = builder.finish(up.unwrap_or(file_up))?;
    let triangles = mesh.triangles();
    let gpu = gpu_enabled.then(|| Arc::new(mesh.to_gpu()));
    if gpu_enabled {
        mesh.positions = Vec::new();
        mesh.normals = Vec::new();
        mesh.colors = None;
    }
    Ok(Model {
        mesh,
        gpu,
        triangles,
        format,
    })
}

/// The first kilobyte of `bytes` as text, cut at the first invalid byte.
fn text_head(bytes: &[u8]) -> &str {
    let head = &bytes[..bytes.len().min(1024)];
    match std::str::from_utf8(head) {
        Ok(text) => text,
        Err(error) => std::str::from_utf8(&head[..error.valid_up_to()]).unwrap_or_default(),
    }
}

/// OBJ has no signature: text whose first records include a vertex.
fn looks_like_obj(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(64 * 1024)];
    if head.contains(&0) {
        return false;
    }
    let Ok(text) = std::str::from_utf8(head)
        .or_else(|error| std::str::from_utf8(&head[..error.valid_up_to()]))
    else {
        return false;
    };
    text.lines().any(|line| line.trim_start().starts_with("v "))
}

#[cfg(test)]
mod tests;
