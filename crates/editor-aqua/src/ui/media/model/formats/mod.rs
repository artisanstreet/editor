use serde::Deserialize;

use super::mesh::{Mesh, MeshBuilder, Up};

mod fbx;
mod gltf;
mod obj;
mod ply;
mod scene;
mod stl;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Obj,
    Fbx,
    Stl,
    Ply,
    #[serde(alias = "glb")]
    Gltf,
}

impl Format {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Obj => "OBJ",
            Self::Fbx => "FBX",
            Self::Stl => "STL",
            Self::Ply => "PLY",
            Self::Gltf => "glTF",
        }
    }

    const fn default_up(self) -> Up {
        match self {
            Self::Stl => Up::Z,
            Self::Obj | Self::Fbx | Self::Ply | Self::Gltf => Up::Y,
        }
    }

    pub fn sniff(bytes: &[u8]) -> Option<Self> {
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

#[derive(Debug)]
pub struct Model {
    pub mesh: Mesh,
    pub triangles: usize,
    pub format: Format,
}

impl Model {
    pub fn resident_bytes(&self) -> usize {
        self.mesh.positions.capacity() * std::mem::size_of::<super::mesh::Vec3>()
            + self.mesh.normals.capacity() * std::mem::size_of::<super::mesh::Vec3>()
            + self.mesh.colors.as_ref().map_or(0, |colors| {
                colors.capacity() * std::mem::size_of::<super::mesh::Vec3>()
            })
    }
}

pub fn parse(bytes: &[u8], format: Option<Format>, up: Option<Up>) -> Result<Model, String> {
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
    let mesh = builder.finish(up.unwrap_or(file_up))?;
    let triangles = mesh.triangles();
    Ok(Model {
        mesh,
        triangles,
        format,
    })
}

fn text_head(bytes: &[u8]) -> &str {
    let head = &bytes[..bytes.len().min(1024)];
    match std::str::from_utf8(head) {
        Ok(text) => text,
        Err(error) => std::str::from_utf8(&head[..error.valid_up_to()]).unwrap_or_default(),
    }
}

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
