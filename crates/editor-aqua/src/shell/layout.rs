pub const TITLEBAR_HEIGHT: f32 = 48.0;
pub const COLUMN_WIDTH: f32 = 327.0;
pub const COLUMN_MIN_WIDTH: f32 = 240.0;
pub const CHAT_MIN_WIDTH: f32 = 864.0;
pub const MIN_WINDOW_WIDTH: f32 = 480.0;
pub const MIN_WINDOW_HEIGHT: f32 = 420.0;
pub const CARD_RADIUS: f32 = 12.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

pub fn launch_size(visible: Option<Size>) -> Size {
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

pub fn sidebar_width(window_width: f32) -> Option<f32> {
    let room = window_width - CHAT_MIN_WIDTH;
    (room >= COLUMN_MIN_WIDTH).then(|| room.min(COLUMN_WIDTH))
}

pub fn inspector_width(content_width: f32) -> Option<f32> {
    let room = content_width - CHAT_MIN_WIDTH;
    (room >= COLUMN_MIN_WIDTH).then(|| room.min(COLUMN_WIDTH))
}

pub fn settings_sidebar_width(window_width: f32) -> f32 {
    (window_width - 520.0).clamp(COLUMN_MIN_WIDTH, COLUMN_WIDTH)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SettingsLayout {
    pub size: Size,
    pub navigation_width: f32,
}

pub fn settings_layout(window: Size) -> SettingsLayout {
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
    pub width: f32,
    pub max_height: f32,
}

pub fn entry_layout(window: Size) -> EntryLayout {
    EntryLayout {
        width: if window.width < 640.0 {
            window.width - 32.0
        } else {
            560.0
        },
        max_height: (window.height - 96.0).max(200.0),
    }
}

pub const RAIL_WIDTH: f32 = 48.0;
pub const CONTROL_WIDTH: f32 = 46.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorkspaceLayout {
    pub sidebar: Option<f32>,
    pub frame_width: f32,
    pub inspector: Option<f32>,
    pub meter_right: f32,
    pub header_right_inset: f32,
}

pub fn workspace_layout(
    window_width: f32,
    screen: &crate::core::Screen,
    show_sidebar: bool,
    show_inspector: bool,
    has_meter: bool,
) -> WorkspaceLayout {
    use crate::core::Screen;
    let sidebar = if matches!(screen, Screen::Home) || !show_sidebar {
        None
    } else {
        sidebar_width(window_width)
    };
    let frame_width = RAIL_WIDTH + sidebar.unwrap_or(0.0);
    let inspector =
        if show_inspector && matches!(screen, Screen::Thread { .. } | Screen::NewThread { .. }) {
            inspector_width(window_width - frame_width)
        } else {
            None
        };
    let controls = CONTROL_WIDTH * 3.0;
    let meter_right = inspector.unwrap_or(0.0).max(controls) + 12.0;
    let header_right_inset = if has_meter {
        (meter_right + 24.0 - controls).max(0.0)
    } else {
        0.0
    };
    WorkspaceLayout {
        sidebar,
        frame_width,
        inspector,
        meter_right,
        header_right_inset,
    }
}
