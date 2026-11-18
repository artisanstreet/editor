#![expect(
    clippy::float_cmp,
    clippy::needless_borrow,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#[path = "ui/kit/divider.rs"]
mod divider;
#[path = "ui/kit/control/hit_area.rs"]
mod hit_area;
#[path = "ui/kit/hover.rs"]
mod hover;
#[path = "ui/kit/control/icon_button.rs"]
mod icon_button;
#[path = "ui/kit/tokens/radius.rs"]
mod radius;
#[path = "ui/kit/row.rs"]
mod row;
#[path = "ui/kit/button/special.rs"]
mod special;
#[path = "ui/kit/surface.rs"]
mod surface;
