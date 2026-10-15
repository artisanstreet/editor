//! FBX geometry, transforms, and material colors, decoded by ufbx.
use super::super::mesh::{MeshBuilder, Up};
pub(in super::super) fn is_binary(bytes: &[u8]) -> bool {
    bytes.starts_with(b"Kaydara FBX Binary  \0\x1a\0")
}
pub(in super::super) fn is_ascii(bytes: &[u8]) -> bool {
    super::text_head(bytes).trim_start().starts_with("; FBX")
}
pub(in super::super) fn parse(bytes: &[u8], mesh: &mut MeshBuilder) -> Result<Up, String> {
    super::scene::parse(bytes, ufbx::FileFormat::Fbx, mesh)
}
