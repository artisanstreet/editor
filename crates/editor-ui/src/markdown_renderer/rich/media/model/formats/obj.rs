//! OBJ geometry and mixed vertex colors, decoded by ufbx.
use super::super::mesh::MeshBuilder;
pub(in super::super) fn parse(text: &str, mesh: &mut MeshBuilder) -> Result<(), String> {
    super::scene::parse(text.as_bytes(), ufbx::FileFormat::Obj, mesh).map(|_| ())
}
