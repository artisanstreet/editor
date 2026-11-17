#![expect(
    clippy::cast_precision_loss,
    clippy::float_cmp,
    clippy::needless_borrow,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#[path = "ui/kit/divider.rs"]
mod divider;
#[path = "ui/kit/hover.rs"]
mod hover;
#[path = "ui/kit/tokens/radius.rs"]
mod radius;
#[path = "ui/kit/row.rs"]
mod row;
#[path = "ui/kit/button/special.rs"]
mod special;
#[path = "ui/kit/surface.rs"]
mod surface;
#[path = "ui/kit/tokens/text_role.rs"]
mod text_role;
