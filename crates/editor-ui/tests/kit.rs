fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

#[path = "ui/code_copy.rs"]
mod code_copy;
#[path = "ui/kit/divider.rs"]
mod divider;
#[path = "ui/kit/control/hit_area.rs"]
mod hit_area;
#[path = "ui/kit/hover.rs"]
mod hover;
#[path = "ui/kit/control/icon_button.rs"]
mod icon_button;
#[path = "ui/kit/problem.rs"]
mod problem;
#[path = "ui/kit/tokens/radius.rs"]
mod radius;
#[path = "ui/kit/row.rs"]
mod row;
#[path = "ui/kit/button/special.rs"]
mod special;
#[path = "ui/kit/surface.rs"]
mod surface;
