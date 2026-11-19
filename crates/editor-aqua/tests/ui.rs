#![expect(
    clippy::cast_precision_loss,
    clippy::clone_on_ref_ptr,
    clippy::float_cmp,
    clippy::imprecise_flops,
    clippy::match_bool,
    clippy::needless_for_each,
    clippy::string_slice,
    clippy::suboptimal_flops,
    clippy::unreadable_literal,
    clippy::wildcard_enum_match_arm,
    unreachable_pub,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
#![cfg(test)]

#[path = "ui/code_copy.rs"]
mod code_copy;
#[path = "ui/color_copy_menu.rs"]
mod color_copy_menu;
#[expect(
    clippy::disallowed_types,
    reason = "test fixture driving the raw aqua text and element APIs, not view code"
)]
#[path = "ui/dialog.rs"]
mod dialog;
#[path = "ui/hover_tooltip.rs"]
mod hover_tooltip;
#[expect(
    clippy::disallowed_types,
    reason = "test fixture driving the raw aqua text and element APIs, not view code"
)]
#[path = "../../editor-ui/tests/ui/inline_code_text.rs"]
mod inline_code_text;
#[path = "ui/marquee.rs"]
mod marquee;
#[path = "ui/motion.rs"]
mod motion;
#[path = "ui/page.rs"]
mod page;
#[path = "ui/phrase.rs"]
mod phrase;
#[path = "ui/popover.rs"]
mod popover;
#[path = "ui/select.rs"]
mod select;
#[expect(
    clippy::disallowed_types,
    reason = "test fixture driving the raw aqua text and element APIs, not view code"
)]
#[path = "ui/shimmer_text.rs"]
mod shimmer_text;
#[path = "ui/tab_card.rs"]
mod tab_card;

#[path = "ui/chat_bubble.rs"]
mod chat_bubble;

#[expect(
    clippy::disallowed_types,
    reason = "test fixture driving the raw aqua text and element APIs, not view code"
)]
#[path = "ui/media_frame.rs"]
mod media_frame;

#[expect(
    unreachable_pub,
    reason = "the included editor-ui sky sampler keeps the pub items its own crate exports"
)]
#[path = "../../editor-ui/tests/ui/sky/sample.rs"]
mod sky_sample;
