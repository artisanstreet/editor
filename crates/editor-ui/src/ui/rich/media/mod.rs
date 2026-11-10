use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
pub(crate) struct SizeHint {
    #[serde(default)]
    pub(crate) width: Option<f32>,
    #[serde(default)]
    pub(crate) height: Option<f32>,
}

impl SizeHint {
    fn aspect(self) -> Option<f32> {
        match (self.width, self.height) {
            (Some(width), Some(height)) if width > 0.0 && height > 0.0 => {
                Some((width / height).clamp(0.05, 20.0))
            }
            _ => None,
        }
    }
}

pub(crate) mod audio;
pub(crate) mod image;
pub(crate) mod image_compare;
pub(crate) mod model;
