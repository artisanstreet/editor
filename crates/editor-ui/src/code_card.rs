//! One code surface and copy-action geometry for ordinary and rich fences.
use crate::glass::{
    GlassStrength, glass_card_shadows, glass_foreground_base, glass_highlight_layer,
    glass_material_layer,
};
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens};
use gpui::{
    AnyElement, App, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce,
    SharedString, Styled as _, Window, div,
};

#[derive(Clone, Copy)]
pub enum CopyPlacement {
    TopRight,
    CenterRight,
}
#[derive(IntoElement)]
pub struct CodeCard {
    theme: ArtisanTheme,
    content: AnyElement,
    copy: Option<(AnyElement, CopyPlacement)>,
    balanced: bool,
    selector: Option<SharedString>,
}
impl CodeCard {
    pub fn new(theme: ArtisanTheme, content: impl IntoElement) -> Self {
        Self {
            theme,
            content: content.into_any_element(),
            copy: None,
            balanced: false,
            selector: None,
        }
    }
    pub fn copy_action(mut self, action: impl IntoElement, placement: CopyPlacement) -> Self {
        self.copy = Some((action.into_any_element(), placement));
        self
    }
    pub fn balanced_gutters(mut self) -> Self {
        self.balanced = true;
        self
    }
    pub fn debug_selector(mut self, selector: SharedString) -> Self {
        self.selector = Some(selector);
        self
    }
}
impl RenderOnce for CodeCard {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let theme = &self.theme;
        let radius = RadiusTokens::value(RadiusStep::X2l);
        let inset = theme.spacing.steps(2.0);
        let gutter = inset * 2.0 + theme.density.control_sm;
        let mut card = div()
            .relative()
            .w_full()
            .min_w_0()
            .text_color(theme.colors.foreground.to_paint())
            .bg(glass_foreground_base(theme))
            .rounded(radius)
            .shadow(glass_card_shadows())
            .p(theme.density.card_padding_compact)
            .child(glass_material_layer(GlassStrength::Quiet, radius))
            .child(glass_highlight_layer(GlassStrength::Quiet, radius));
        if let Some((action, placement)) = self.copy {
            card = card.pr(gutter);
            if self.balanced {
                card = card.pl(gutter);
            }
            let action = div()
                .absolute()
                .right(inset)
                .flex()
                .items_center()
                .child(action);
            let action = match placement {
                CopyPlacement::TopRight => action.top(inset),
                CopyPlacement::CenterRight => action.top_0().bottom_0(),
            };
            card = card.child(action);
        }
        card = card.child(div().relative().w_full().min_w_0().child(self.content));
        if let Some(selector) = self.selector {
            card = card.debug_selector(move || selector.to_string());
        }
        card
    }
}
