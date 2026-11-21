use super::surface::{Pixels, Surface};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CaptureRecord {
    pub(crate) name: String,
    pub(crate) implementation: String,
    pub(crate) route: String,
    pub(crate) logical_width: u32,
    pub(crate) logical_height: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) scale_bits: u32,
    pub(crate) backend: String,
    pub(crate) adapter: String,
    pub(crate) pixel_format: String,
    pub(crate) alpha: String,
    pub(crate) color_profile: String,
    pub(crate) surface: String,
    pub(crate) repeat_identical: bool,
    pub(crate) sha256: String,
}

#[derive(Debug)]
pub struct Context<'a> {
    pub(crate) name: &'a str,
    pub(crate) route: &'a str,
    pub(crate) logical: (f64, f64),
    pub(crate) surface: &'a Surface,
    pub(crate) repeat_identical: bool,
}

impl CaptureRecord {
    pub(crate) fn describe(context: &Context<'_>, pixels: &Pixels) -> Self {
        Self {
            name: context.name.to_owned(),
            implementation: "Aqua".to_owned(),
            route: context.route.to_owned(),
            logical_width: crate::measure::saturate_pixels(context.logical.0.round()),
            logical_height: crate::measure::saturate_pixels(context.logical.1.round()),
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
