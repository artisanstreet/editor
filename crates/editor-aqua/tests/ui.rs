#![cfg(test)]

#[path = "ui/page.rs"]
mod page;
#[path = "ui/code_copy.rs"]
mod code_copy;
#[path = "ui/color_copy_menu.rs"]
mod color_copy_menu;
#[path = "ui/hover_tooltip.rs"]
mod hover_tooltip;
#[expect(clippy::disallowed_types, reason = "test fixture driving the raw aqua text and element APIs, not view code")]
#[path = "../../editor-ui/tests/ui/inline_code_text.rs"]
mod inline_code_text;
#[path = "ui/popover.rs"]
mod popover;
#[path = "ui/tab_card.rs"]
mod tab_card;
#[path = "ui/marquee.rs"]
mod marquee;
#[expect(clippy::disallowed_types, reason = "test fixture driving the raw aqua text and element APIs, not view code")]
#[path = "ui/dialog.rs"]
mod dialog;
#[path = "ui/motion.rs"]
mod motion;
#[expect(clippy::disallowed_types, reason = "test fixture driving the raw aqua text and element APIs, not view code")]
#[path = "ui/shimmer_text.rs"]
mod shimmer_text;
#[path = "ui/phrase.rs"]
mod phrase;

#[path = "ui/chat_bubble.rs"]
mod chat_bubble;

#[expect(clippy::disallowed_types, reason = "test fixture driving the raw aqua text and element APIs, not view code")]
#[path = "ui/media_frame.rs"]
mod media_frame;

#[path = "../../editor-ui/tests/ui/solar/sample.rs"]
mod solar_sample;

#[path = "../../editor-ui/tests/ui/sky/sample.rs"]
mod sky_sample;

