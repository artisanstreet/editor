//! Joined message segments keep one frame recipe while native rows own visibility.
use crate::gradient::vertical_gradient;
use crate::theme::{
    ArtisanTheme, Oklch, ProseTypography, RadiusStep, RadiusTokens, SurfaceStep, ThemeMode,
};
use gpui::InteractiveElement as _;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _,
    Window, div, px,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SegmentPosition {
    Single,
    Head,
    Body,
    Tail,
}
impl SegmentPosition {
    pub fn for_index(index: usize, count: usize) -> Self {
        match (index == 0, index + 1 >= count) {
            (true, true) => Self::Single,
            (true, false) => Self::Head,
            (false, true) => Self::Tail,
            (false, false) => Self::Body,
        }
    }
    fn first(self) -> bool {
        matches!(self, Self::Single | Self::Head)
    }
    fn last(self) -> bool {
        matches!(self, Self::Single | Self::Tail)
    }
}

#[derive(IntoElement)]
pub struct ChatBubbleSegment {
    theme: ArtisanTheme,
    position: SegmentPosition,
    content: AnyElement,
    selector: Option<SharedString>,
}
impl ChatBubbleSegment {
    /// Attachment placement shares the message's horizontal measure.
    pub const MAX_WIDTH: f32 = 576.0;
    pub fn new(theme: ArtisanTheme, position: SegmentPosition, content: impl IntoElement) -> Self {
        Self {
            theme,
            position,
            content: content.into_any_element(),
            selector: None,
        }
    }
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.selector = Some(selector.into());
        self
    }
}
impl ChatBubbleSegment {
    fn frame(self) -> gpui::Div {
        let position = self.position;
        let (top, bottom) = match self.theme.mode {
            ThemeMode::Dark => (SurfaceStep::S775.oklch(), SurfaceStep::S850.oklch()),
            ThemeMode::Light => (SurfaceStep::S150.oklch(), SurfaceStep::S225.oklch()),
        };
        let middle = Oklch::new((top.l + bottom.l) * 0.5, 0.0, 0.0);
        let (top, bottom) = match position {
            SegmentPosition::Single => (top, bottom),
            SegmentPosition::Head => (top, middle),
            SegmentPosition::Body => (middle, middle),
            SegmentPosition::Tail => (middle, bottom),
        };
        div()
            .max_w(px(Self::MAX_WIDTH))
            .when(position != SegmentPosition::Single, |frame| frame.w_full())
            .when(position.first(), |frame| {
                frame
                    .rounded_tl(RadiusTokens::value(RadiusStep::X2l))
                    .rounded_tr(RadiusTokens::value(RadiusStep::X2l))
            })
            .when(position.last(), |frame| {
                frame
                    .rounded_bl(RadiusTokens::value(RadiusStep::X2l))
                    .rounded_br(RadiusTokens::value(RadiusStep::X2l))
            })
            .bg(vertical_gradient(top, bottom))
            .when(position == SegmentPosition::Single, |frame| {
                frame.shadow(crate::glass::card_shadows(&self.theme))
            })
            .px(px(16.0))
            .pt(px(if position.first() { 12.0 } else { 0.0 }))
            .pb(px(if position.last() { 12.0 } else { 0.0 }))
            .text_color(self.theme.colors.foreground.to_paint())
            .text_size(px(ProseTypography::BODY_SIZE_PX))
            .line_height(px(ProseTypography::BODY_LINE_PX))
            .font_weight(ProseTypography::BODY_WEIGHT)
            .tracking(ProseTypography::TRACKING_EM)
            .whitespace_normal()
            .when_some(self.selector, |frame, selector| {
                frame.debug_selector(move || selector.to_string())
            })
            .child(self.content)
    }
}

impl RenderOnce for ChatBubbleSegment {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        self.frame()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Render, TestAppContext};
    struct Segments;
    impl Render for Segments {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let theme = ArtisanTheme::default();
            div()
                .w_full()
                .flex()
                .flex_col()
                .items_end()
                .children((0..3).map(|index| {
                    ChatBubbleSegment::new(
                        theme,
                        SegmentPosition::for_index(index, 3),
                        div().h(px(20.0)),
                    )
                    .debug_selector(format!("chat-segment-{index}"))
                }))
        }
    }
    #[gpui::test]
    fn segmented_messages_join_and_shrink_with_the_viewport(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| Segments);
        cx.simulate_resize(gpui::size(px(720.0), px(240.0)));
        let head = cx.debug_bounds("chat-segment-0").unwrap();
        let body = cx.debug_bounds("chat-segment-1").unwrap();
        let tail = cx.debug_bounds("chat-segment-2").unwrap();
        assert_eq!(head.size.width, px(ChatBubbleSegment::MAX_WIDTH));
        assert_eq!(head.size.width, body.size.width);
        assert_eq!(body.size.width, tail.size.width);
        assert_eq!(head.bottom(), body.top());
        assert_eq!(body.bottom(), tail.top());
        assert_eq!(head.size.height, px(32.0));
        assert_eq!(body.size.height, px(20.0));
        assert_eq!(tail.size.height, px(32.0));
        cx.simulate_resize(gpui::size(px(360.0), px(240.0)));
        for selector in ["chat-segment-0", "chat-segment-1", "chat-segment-2"] {
            assert_eq!(cx.debug_bounds(selector).unwrap().size.width, px(360.0));
        }
    }
    #[test]
    fn rendered_frame_rounds_only_the_exposed_message_corners() {
        let theme = ArtisanTheme::default();
        let radius: gpui::AbsoluteLength = RadiusTokens::value(RadiusStep::X2l).into();
        for (position, expected) in [
            (SegmentPosition::Single, [true, true, true, true]),
            (SegmentPosition::Head, [true, true, false, false]),
            (SegmentPosition::Body, [false, false, false, false]),
            (SegmentPosition::Tail, [false, false, true, true]),
        ] {
            let mut frame = ChatBubbleSegment::new(theme, position, "text").frame();
            let corners = &frame.style().corner_radii;
            for (corner, rounded) in [
                corners.top_left,
                corners.top_right,
                corners.bottom_right,
                corners.bottom_left,
            ]
            .into_iter()
            .zip(expected)
            {
                assert_eq!(corner, rounded.then_some(radius), "{position:?}");
            }
        }
    }
}
