use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
pub(crate) struct SizeHint {
    #[serde(default)]
    width: Option<f32>,
    #[serde(default)]
    height: Option<f32>,
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

pub(crate) fn opens_context_menu(key: &aqua_core::KeyEvent) -> bool {
    match key.key {
        aqua_core::Key::Named(aqua_core::NamedKey::ContextMenu) => key.modifiers.is_empty(),
        aqua_core::Key::Named(aqua_core::NamedKey::Function(10)) => {
            key.modifiers == aqua_core::Modifiers::SHIFT
        }
        aqua_core::Key::Char(_) | aqua_core::Key::Named(_) => false,
    }
}

pub(crate) fn menu_anchor(bounds: aqua_core::Rect) -> aqua_core::Point {
    aqua_core::Point::new(bounds.x, bounds.y)
}

pub(crate) mod audio;
pub(crate) mod image;
pub(crate) mod image_compare;
pub(crate) mod model;
