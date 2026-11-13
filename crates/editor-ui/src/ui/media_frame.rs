use aqua::ImageHandle;
use aqua_core::{CursorIcon, GpuiBackground, Insets, Role, image::ImageFit};
use aqua_ui::{
    element::{Box as UiBox, Button, Canvas, Element, Key, Media},
    style::{Length, Style},
    tree::ActionEvent,
};
use std::rc::Rc;

use super::{
    kit::{Media as MediaKind, Surface, concentric},
    theme::{ArtisanTheme, Opacity},
};

const FRAME: Surface = Surface::Media(MediaKind::Frame);
const THUMB: Surface = Surface::Media(MediaKind::Thumb);
const EDGE: f32 = 1.0;

enum FrameSize {
    Aspect { aspect: f32, width: f32 },
    Attachment,
    SentAttachment,
    Transcript { aspect: f32, width: f32 },
}

pub struct MediaFrame {
    theme: ArtisanTheme,
    size: FrameSize,
    content: Element,
    id: Option<Key>,
    selector: Option<String>,
    activation: Option<(String, Box<dyn Fn(ActionEvent)>)>,
    error: bool,
    visible: Option<Rc<dyn Fn()>>,
}

impl MediaFrame {
    pub fn new(theme: ArtisanTheme, aspect: f32, measured_width: f32, content: Element) -> Self {
        let aspect = if aspect.is_finite() && aspect > 0.0 {
            aspect.clamp(0.05, 20.0)
        } else {
            16.0 / 9.0
        };
        Self {
            theme,
            size: FrameSize::Aspect {
                aspect,
                width: measured_width,
            },
            content,
            id: None,
            selector: None,
            activation: None,
            error: false,
            visible: None,
        }
    }

    pub fn sent_attachment(theme: ArtisanTheme, content: Element) -> Self {
        Self {
            size: FrameSize::SentAttachment,
            ..Self::new(theme, 1.0, 96.0, content)
        }
    }

    pub fn attachment(id: impl Into<Key>, theme: ArtisanTheme, content: Element) -> Self {
        Self {
            size: FrameSize::Attachment,
            id: Some(id.into()),
            ..Self::new(theme, 1.0, 72.0, content)
        }
    }

    pub fn transcript_image(
        theme: ArtisanTheme,
        aspect: f32,
        measured_width: f32,
        content: Element,
    ) -> Self {
        Self {
            size: FrameSize::Transcript {
                aspect: aspect.max(16.0 / 9.0),
                width: measured_width,
            },
            ..Self::new(theme, 1.0, measured_width, content)
        }
    }

    pub fn image_content(image: ImageHandle) -> Element {
        let background = UiBox::new()
            .style(
                Style::new()
                    .absolute_at(Some(-48.0), Some(-48.0), Some(-48.0), Some(-48.0))
                    .content_blur(24.0),
            )
            .child(
                Media::new(image.clone())
                    .width(Length::Pct(100.0))
                    .height(Length::Pct(100.0))
                    .fit(ImageFit::Cover)
                    .build(),
            )
            .build();
        let foreground = Media::new(image)
            .width(Length::Pct(100.0))
            .height(Length::Pct(100.0))
            .fit(ImageFit::Contain)
            .build();
        UiBox::new()
            .width(Length::Pct(100.0))
            .height(Length::Pct(100.0))
            .clip(true)
            .child(background)
            .child(foreground)
            .build()
    }

    pub fn debug_selector(mut self, selector: impl Into<String>) -> Self {
        self.selector = Some(selector.into());
        self
    }

    pub fn on_activate(
        mut self,
        id: impl Into<Key>,
        label: impl Into<String>,
        action: impl Fn(ActionEvent) + 'static,
    ) -> Self {
        self.id = Some(id.into());
        self.activation = Some((label.into(), Box::new(action)));
        self
    }

    pub const fn error(mut self, error: bool) -> Self {
        self.error = error;
        self
    }

    pub fn on_visible(mut self, action: impl Fn() + 'static) -> Self {
        self.visible = Some(Rc::new(action));
        self
    }

    pub fn thumb_radius() -> f32 {
        THUMB.radius_px()
    }

    pub fn thumb_content_style() -> Style {
        Style::new().radius(Length::Px(concentric(THUMB.radius_px(), EDGE)))
    }

    pub fn sent_attachment_content_style() -> Style {
        Self::thumb_content_style()
    }

    pub fn placeholder(theme: ArtisanTheme) -> Element {
        UiBox::new()
            .width(Length::Pct(100.0))
            .height(Length::Pct(100.0))
            .style(Style::new().gpui_background(GpuiBackground::Solid(
                theme.colors.raised.to_reference_paint(),
            )))
            .build()
    }

    pub fn build(self) -> Element {
        let edge = match self.error {
            true => self.theme.colors.destructive,
            false => self.theme.colors.text.at(Opacity::MediaEdge),
        };
        let (width, height, surface, border) = match self.size {
            FrameSize::Aspect { aspect, width } => (width, width / aspect, FRAME, None),
            FrameSize::Attachment => (72.0, 72.0, THUMB, Some(edge)),
            FrameSize::SentAttachment => (96.0, 96.0, THUMB, Some(self.theme.colors.line)),
            FrameSize::Transcript { aspect, width } => {
                let width = width.min(576.0);
                (width, (width / aspect).min(480.0), FRAME, None)
            }
        };
        let mut style = Style::new()
            .width(Length::Px(width))
            .height(Length::Px(height))
            .radius(Length::Px(surface.radius_px()))
            .shadows(surface.elevation().shadows(&self.theme));
        if let Some(color) = border {
            style = style
                .flex_shrink(0.0)
                .gpui_border(Insets::uniform(EDGE), color.to_reference_paint(), false)
                .gpui_background(GpuiBackground::Solid(
                    self.theme.colors.surface.to_reference_paint(),
                ));
        }
        let mut children = vec![self.content];
        if let Some(visible) = self.visible {
            children.push(
                Canvas::new()
                    .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
                    .on_observe(move |tree, node| {
                        let Some(bounds) = tree.visual_rect(node) else {
                            return;
                        };
                        if !bounds.intersect(tree.observe_clip(node)).is_empty() {
                            visible();
                        }
                    })
                    .build()
                    .exclude_from_hit_testing(),
            );
        }
        let mut element = if let Some((label, action)) = self.activation {
            let focus = Style::new().gpui_border(
                Insets::uniform(if border.is_some() { EDGE } else { 0.0 }),
                self.theme.colors.ring.to_reference_paint(),
                false,
            );
            Button::new()
                .style(style)
                .clip(true)
                .children(children)
                .on_activate_event(action)
                .build()
                .role(Role::Button)
                .aria_label(label)
                .focusable(true)
                .cursor(CursorIcon::Pointer)
                .focus_visible_style(focus)
        } else {
            UiBox::new()
                .style(style)
                .clip(true)
                .children(children)
                .build()
        };
        if let Some(id) = self.id {
            element = element.with_key(id);
        }
        if let Some(selector) = self.selector {
            element = element.debug_selector(selector);
        }
        element
    }
}
