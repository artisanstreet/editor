//! Media frame geometry belongs to this component across loading, ready, and error states.

use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens};
use gpui::{
    AnyElement, App, ClickEvent, ElementId, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, RenderOnce, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, canvas, div, px,
};

type Activation = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
type Visible = Box<dyn Fn(&mut App)>;
enum Size {
    Aspect(f32),
    Attachment,
    SentAttachment,
    Transcript,
}

/// Callers place the frame and supply its content. Radius, clipping, border,
/// and state geometry cannot be refined by feature code.
#[derive(IntoElement)]
pub struct MediaFrame {
    theme: ArtisanTheme,
    size: Size,
    content: AnyElement,
    id: Option<ElementId>,
    selector: Option<SharedString>,
    activation: Option<(SharedString, Activation)>,
    error: bool,
    visible: Option<Visible>,
}
impl MediaFrame {
    /// Sent attachments keep the original 96px transcript thumbnail recipe.
    pub fn sent_attachment(theme: ArtisanTheme, content: impl IntoElement) -> Self {
        Self {
            size: Size::SentAttachment,
            ..Self::new(theme, 1.0, content)
        }
    }
    /// A generated image keeps its intrinsic aspect within the transcript measure.
    pub fn transcript_image(theme: ArtisanTheme, content: impl IntoElement) -> Self {
        Self {
            size: Size::Transcript,
            ..Self::new(theme, 1.0, content)
        }
    }
    pub fn new(theme: ArtisanTheme, aspect: f32, content: impl IntoElement) -> Self {
        let aspect = if aspect.is_finite() && aspect > 0.0 {
            aspect.clamp(0.05, 20.0)
        } else {
            16.0 / 9.0
        };
        Self {
            theme,
            size: Size::Aspect(aspect),
            content: content.into_any_element(),
            id: None,
            selector: None,
            activation: None,
            error: false,
            visible: None,
        }
    }
    pub(crate) fn attachment(
        id: impl Into<ElementId>,
        theme: &ArtisanTheme,
        content: impl IntoElement,
    ) -> Self {
        Self {
            size: Size::Attachment,
            id: Some(id.into()),
            ..Self::new(*theme, 1.0, content)
        }
    }
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.selector = Some(selector.into());
        self
    }
    /// Makes the whole frame a native keyboard and pointer action. Nested
    /// controls can stop propagation to keep their own action separate.
    #[must_use]
    pub fn on_activate(
        mut self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        action: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.id = Some(id.into());
        self.activation = Some((label.into(), Box::new(action)));
        self
    }
    /// Defers feature work only after this individual frame paints inside the
    /// native content clip. Building or measuring a frame never runs it.
    #[must_use]
    pub fn on_visible(mut self, action: impl Fn(&mut App) + 'static) -> Self {
        self.visible = Some(Box::new(action));
        self
    }
    pub(crate) const fn error(mut self, value: bool) -> Self {
        self.error = value;
        self
    }
    /// The composer tile's outer corner.
    pub(crate) const ATTACHMENT_RADIUS: RadiusStep = RadiusStep::Sm;
    /// The sent thumbnail's outer corner.
    const SENT_ATTACHMENT_RADIUS: RadiusStep = RadiusStep::Md;
    /// The frame clips square, so content rounds itself to the corner inside
    /// the 1 px border.
    #[must_use]
    pub fn attachment_content_radius() -> Pixels {
        RadiusTokens::nested(RadiusTokens::value(Self::ATTACHMENT_RADIUS), px(1.0))
    }
    /// [`Self::attachment_content_radius`] for the sent thumbnail.
    #[must_use]
    pub fn sent_attachment_content_radius() -> Pixels {
        RadiusTokens::nested(RadiusTokens::value(Self::SENT_ATTACHMENT_RADIUS), px(1.0))
    }
}
impl RenderOnce for MediaFrame {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let mut frame = div()
            .relative()
            .overflow_hidden()
            .border_1()
            .border_color(if self.error {
                theme.colors.destructive.to_paint()
            } else {
                theme
                    .colors
                    .text
                    .at(crate::theme::Opacity::MediaEdge)
                    .to_paint()
            });
        frame = match self.size {
            Size::Aspect(aspect) => frame
                .w_full()
                .aspect_ratio(aspect)
                .rounded(RadiusTokens::value(RadiusStep::Xl)),
            Size::Attachment => frame
                .size(px(72.0))
                .flex_none()
                .rounded(RadiusTokens::value(Self::ATTACHMENT_RADIUS))
                .bg(theme.colors.surface.to_paint())
                .shadow(crate::glass::card_shadows(&theme)),
            Size::SentAttachment => frame
                .size(px(96.0))
                .flex_none()
                .rounded(RadiusTokens::value(Self::SENT_ATTACHMENT_RADIUS))
                .border_color(theme.colors.line.to_paint())
                .bg(theme.colors.surface.to_paint()),
            Size::Transcript => frame
                .max_w(px(576.0))
                .max_h(px(480.0))
                .rounded(RadiusTokens::value(RadiusStep::Lg))
                .border_0(),
        };
        if let Some(selector) = self.selector {
            frame = frame.debug_selector(move || selector.to_string());
        }
        let mut frame = frame.child(self.content);
        if let Some(visible) = self.visible {
            frame = frame.child(
                canvas(
                    |_, _, _| {},
                    move |bounds, (), window, cx| {
                        let clip = window.content_mask().bounds;
                        if clip.size.width > px(0.0)
                            && clip.size.height > px(0.0)
                            && bounds.right() > clip.left()
                            && bounds.left() < clip.right()
                            && bounds.bottom() > clip.top()
                            && bounds.top() < clip.bottom()
                            && bounds.size.width > px(0.0)
                            && bounds.size.height > px(0.0)
                        {
                            // The native list can own layout borrows until paint ends.
                            cx.defer(move |cx| visible(cx));
                        }
                    },
                )
                .absolute()
                .inset_0(),
            );
        }
        let Some(id) = self.id else {
            return frame.into_any_element();
        };
        let mut frame = frame.id(id);
        if let Some((label, action)) = self.activation {
            frame = frame
                .role(gpui::Role::Button)
                .aria_label(label)
                .focusable()
                .tab_index(0)
                .cursor_pointer()
                .focus_visible(move |style| style.border_color(theme.colors.ring.to_paint()))
                .on_click(action);
        }
        frame.into_any_element()
    }
}
