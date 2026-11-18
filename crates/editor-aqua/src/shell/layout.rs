pub use crate::ui::chrome::TITLEBAR_HEIGHT;
const COLUMN_WIDTH: f32 = 327.0;
const COLUMN_MIN_WIDTH: f32 = 240.0;
pub const CHAT_MIN_WIDTH: f32 = 864.0;
pub(crate) const MIN_WINDOW_WIDTH: f32 = 720.0;
pub(crate) const MIN_WINDOW_HEIGHT: f32 = 480.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

pub(crate) fn launch_size(visible: Option<Size>) -> Size {
    let mut size = Size {
        width: 2.0_f32.mul_add(COLUMN_WIDTH, CHAT_MIN_WIDTH),
        height: 900.0,
    };
    if let Some(visible) = visible {
        size.width = size.width.min(visible.width).max(MIN_WINDOW_WIDTH);
        size.height = size.height.min(visible.height).max(MIN_WINDOW_HEIGHT);
    }
    size
}

fn settings_sidebar_width(window_width: f32) -> f32 {
    (window_width - 520.0).clamp(COLUMN_MIN_WIDTH, COLUMN_WIDTH)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SettingsLayout {
    pub(crate) size: Size,
    pub(crate) navigation_width: f32,
}

pub(crate) fn settings_layout(window: Size) -> SettingsLayout {
    let scale = (2.0_f32.mul_add(-24.0, window.width) / window.width)
        .min(2.0_f32.mul_add(-24.0, window.height) / window.height)
        .min(1100.0 / window.width)
        .min(760.0 / window.height)
        .max(0.0);
    let size = Size {
        width: (window.width * scale).max(1.0),
        height: (window.height * scale).max(1.0),
    };
    SettingsLayout {
        navigation_width: settings_sidebar_width(size.width)
            .min(260.0)
            .min(size.width * 0.4),
        size,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EntryLayout {
    width: f32,
    max_height: f32,
}

const CONTROL_WIDTH: f32 = crate::ui::kit::control::caption::WIDTH;
const CONTROL_GLYPH: f32 = crate::ui::kit::IconSize::Md.px();
const CONTROLS_WIDTH: f32 = CONTROL_WIDTH * 3.0;
pub(crate) const CONTROL_INSET_X: f32 = (CONTROL_WIDTH - CONTROL_GLYPH) / 2.0;
pub(crate) const CONTROL_INK_INSET: f32 = CONTROL_GLYPH * 5.0 / 24.0;

pub(crate) fn meter_right(inspector: Option<f32>) -> f32 {
    inspector.unwrap_or(0.0).max(CONTROLS_WIDTH) + crate::ui::kit::Space::S12.px()
}

pub(crate) fn header_right_inset(meter_right: f32, has_meter: bool) -> f32 {
    match has_meter {
        true => (meter_right + crate::ui::kit::Space::S24.px() - CONTROLS_WIDTH).max(0.0),
        false => 0.0,
    }
}
