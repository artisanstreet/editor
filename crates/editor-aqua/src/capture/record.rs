use super::surface::{Pixels, Surface};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CaptureRecord {
    pub name: String,
    pub implementation: String,
    pub route: String,
    pub logical_width: u32,
    pub logical_height: u32,
    pub width: u32,
    pub height: u32,
    pub scale_bits: u32,
    pub backend: String,
    pub adapter: String,
    pub pixel_format: String,
    pub alpha: String,
    pub color_profile: String,
    pub surface: String,
    pub repeat_identical: bool,
    pub sha256: String,
}

pub struct Context<'a> {
    pub name: &'a str,
    pub route: &'a str,
    pub logical: (f64, f64),
    pub surface: &'a Surface,
    pub repeat_identical: bool,
}

impl CaptureRecord {
    pub fn describe(context: &Context<'_>, pixels: &Pixels) -> Self {
        Self {
            name: context.name.to_owned(),
            implementation: "Aqua".to_owned(),
            route: context.route.to_owned(),
            logical_width: context.logical.0.round() as u32,
            logical_height: context.logical.1.round() as u32,
            width: pixels.width,
            height: pixels.height,
            scale_bits: context.surface.scale.to_bits(),
            backend: context.surface.backend.clone(),
            adapter: context.surface.adapter.clone(),
            pixel_format: "rgba8-unorm".to_owned(),
            alpha: if pixels.is_opaque() {
                "opaque"
            } else {
                "premultiplied-composed"
            }
            .to_owned(),
            color_profile: "gpui-windows-encoded".to_owned(),
            surface: "offscreen-client-area-no-native-chrome".to_owned(),
            repeat_identical: context.repeat_identical,
            sha256: hex::encode(Sha256::digest(&pixels.rgba)),
        }
    }
}
