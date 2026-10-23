//! `model`: a 3D model the reader can turn, shown on a transparent canvas.
//!
//! OBJ, FBX (binary), STL, PLY, and glTF (`.glb`, or `.gltf` with embedded
//! buffers) load through the same `src` rules as images and parse off the
//! main thread ([`load`], [`formats`]). Every model is centered and scaled
//! into the unit sphere ([`mesh`]), so the camera frames any file the same
//! way. A small software rasterizer ([`raster`]) draws it in a clay
//! material, or the file's own colors, under a studio light rig; the view
//! ([`view`]) orbits, pans, and zooms it and re-renders off the main thread.

use crate::markdown_renderer::rich::budget;
use gpui::{AnyElement, IntoElement as _};
use serde::Deserialize;

use super::super::{BlockContext, RichBlock};
use super::color;
use super::picture::SizeHint;
use super::source::MediaSource;

mod formats;
mod load;
mod mesh;
mod raster;
mod view;

use formats::Format;
use mesh::{Up, Vec3, srgb_to_linear};
use view::{ModelView, Shading};

/// The `model` spec.
#[derive(Debug, Deserialize)]
#[serde(try_from = "Spec")]
pub(super) struct ModelBlock {
    src: MediaSource,
    name: Option<String>,
    format: Option<Format>,
    up: Option<Up>,
    /// Linear RGB for surfaces the file does not color.
    color: Option<Vec3>,
    shading: Shading,
    yaw: Option<f32>,
    pitch: Option<f32>,
    hint: SizeHint,
}

#[derive(Debug, Deserialize)]
struct Spec {
    src: MediaSource,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    format: Option<Format>,
    #[serde(default)]
    up: Option<Up>,
    #[serde(default)]
    color: Option<String>,
    #[serde(default)]
    shading: Shading,
    #[serde(default)]
    yaw: Option<f32>,
    #[serde(default)]
    pitch: Option<f32>,
    #[serde(flatten)]
    hint: SizeHint,
}

impl TryFrom<Spec> for ModelBlock {
    type Error = String;

    fn try_from(spec: Spec) -> Result<Self, String> {
        let src = spec.src;
        if src.is_empty() {
            return Err("a model needs a `src`".to_owned());
        }
        let color = match spec.color.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(text) => {
                let parsed = color::parse(text)
                    .ok_or_else(|| format!("`{text}` is not a CSS color"))?
                    .clipped();
                #[expect(clippy::cast_possible_truncation, reason = "channels are 0..=1")]
                let channel = |value: f64| srgb_to_linear(value.clamp(0.0, 1.0) as f32);
                Some(Vec3::new(
                    channel(parsed.r),
                    channel(parsed.g),
                    channel(parsed.b),
                ))
            }
        };
        let finite = |angle: Option<f32>| angle.filter(|angle| angle.is_finite());
        Ok(Self {
            src,
            name: spec
                .name
                .map(|name| name.trim().to_owned())
                .filter(|name| !name.is_empty()),
            format: spec.format,
            up: spec.up,
            color,
            shading: spec.shading,
            yaw: finite(spec.yaw),
            pitch: finite(spec.pitch),
            hint: spec.hint,
        })
    }
}

impl ModelBlock {
    /// The name shown for the model: the spec's, else the file name.
    fn display_name(&self) -> String {
        self.name.clone().unwrap_or_else(|| {
            self.src
                .rsplit(['/', '\\'])
                .next()
                .filter(|name| !name.is_empty() && !self.src.starts_with("data:"))
                .unwrap_or("Model")
                .to_owned()
        })
    }
}

impl RichBlock for ModelBlock {
    fn resident_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.src.resident_bytes()
            + budget::optional(self.name.as_ref())
    }

    fn render(&self, cx: &BlockContext<'_>) -> AnyElement {
        ModelView {
            id: cx.key("model"),
            media: cx.media.cloned(),
            theme: *cx.theme,
            src: self.src.clone(),
            name: self.display_name().into(),
            format: self.format,
            up: self.up,
            color: self.color,
            shading: self.shading,
            yaw: self.yaw,
            pitch: self.pitch,
            hint: self.hint,
        }
        .into_any_element()
    }
}
