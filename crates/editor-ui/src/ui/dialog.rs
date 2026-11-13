use std::rc::Rc;

use aqua_core::{Role, Size};
use aqua_ui::{
    AlignItems, JustifyContent,
    element::{Box as UiBox, Element, Key, Scroll},
    style::{Axis, Length, Style},
};
use editor_assets::AssetId;

use super::{
    kit::{
        Finish, IconButton, IconButtonSize, Space, Spaced, Surface, TextRole, surface::backdrop,
    },
    page,
    theme::ArtisanTheme,
};

const SURFACE: Surface = Surface::Dialog;
const MARGIN: Space = Space::S16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DialogDismissReason {
    Escape,
    Backdrop,
    CloseButton,
}

pub struct Dialog {
    id: Key,
    theme: ArtisanTheme,
    title: String,
    description: Option<String>,
    content: Element,
    dismiss: Option<Rc<dyn Fn(DialogDismissReason)>>,
    max_width: f32,
    viewport_width: Option<(f32, f32)>,
    dismissible: bool,
    bare: bool,
    soft_backdrop: bool,
}

impl Dialog {
    pub fn new(
        id: impl Into<Key>,
        theme: ArtisanTheme,
        title: impl Into<String>,
        content: Element,
    ) -> Self {
        Self {
            id: id.into(),
            theme,
            title: title.into(),
            description: None,
            content,
            dismiss: None,
            max_width: 560.0,
            viewport_width: None,
            dismissible: true,
            bare: false,
            soft_backdrop: false,
        }
    }
    pub const fn dismissible(mut self, value: bool) -> Self {
        self.dismissible = value;
        self
    }
    pub const fn bare(mut self) -> Self {
        self.bare = true;
        self
    }
    pub const fn soft_backdrop(mut self) -> Self {
        self.soft_backdrop = true;
        self
    }
    pub const fn max_width(mut self, width: f32) -> Self {
        self.max_width = width;
        self.viewport_width = None;
        self
    }
    pub const fn viewport_max_width(mut self, inset: f32, maximum: f32) -> Self {
        self.viewport_width = Some((inset, maximum));
        self
    }
    pub fn description(mut self, text: impl Into<String>) -> Self {
        self.description = Some(text.into());
        self
    }
    pub fn on_dismiss(mut self, handler: impl Fn(DialogDismissReason) + 'static) -> Self {
        self.dismiss = Some(Rc::new(handler));
        self
    }

    fn heading(&self) -> Element {
        let theme = self.theme;
        let title = TextRole::Heading
            .text(&theme, self.title.clone())
            .style(Style::new().min_width(Length::Px(0.0)).flex_grow(1.0))
            .build();
        let mut close = IconButton::new(
            "dialog-close",
            theme,
            AssetId::TABLER_X,
            "Close",
            IconButtonSize::Md,
            Finish::Plain,
        )
        .disabled(!self.dismissible);
        if let Some(dismiss) = self.dismiss.clone() {
            close = close.on_activate(move |_| dismiss(DialogDismissReason::CloseButton));
        }
        UiBox::new()
            .spaced(Space::S12)
            .padded(Space::S4)
            .style(
                Style::new()
                    .align(AlignItems::CENTER)
                    .justify(JustifyContent::SPACE_BETWEEN),
            )
            .child(title)
            .child(close.build())
            .build()
    }

    pub fn build_panel(self, viewport: Size) -> Element {
        let theme = self.theme;
        let margin = 2.0 * MARGIN.px();
        let requested = self
            .viewport_width
            .map_or(self.max_width, |(inset, maximum)| {
                (viewport.w - inset).max(1.0).min(maximum)
            });
        let width = requested.min((viewport.w - margin).max(1.0));
        let heading = (!self.bare).then(|| self.heading());
        let mut frame = UiBox::new()
            .layout("column")
            .spaced(Space::S16)
            .children(heading);
        if let Some(description) = self.description {
            frame = frame.child(page::note(&theme, description).build());
        }
        let frame = frame.child(self.content).build();
        let panel = match self.bare {
            true => frame,
            false => SURFACE.build(&theme, vec![frame]),
        };
        Scroll::new()
            .width(width)
            .style(
                Style::new()
                    .axis(Axis::Column)
                    .max_height(Length::Px((viewport.h - margin).max(1.0))),
            )
            .style(TextRole::Label.inherit(&theme))
            .child(panel)
            .build()
            .with_key(self.id)
            .role(Role::Dialog)
            .aria_label(self.title)
    }

    pub fn dismiss_callback(&self) -> Option<Rc<dyn Fn(DialogDismissReason)>> {
        if self.dismissible {
            self.dismiss.clone()
        } else {
            None
        }
    }

    pub const fn is_soft_backdrop(&self) -> bool {
        self.soft_backdrop
    }
}

pub struct DialogHost {
    overlay: aqua_ui::overlay::OverlayId,
    pending: Rc<std::cell::Cell<Option<DialogDismissReason>>>,
    closing: Rc<std::cell::Cell<Option<DialogDismissReason>>>,
    theme: Rc<std::cell::Cell<ArtisanTheme>>,
    rebuild: DialogRebuild,
}

type DialogRebuild = Rc<dyn Fn(Element, Size) -> Element>;

impl DialogHost {
    pub fn open(
        app: &mut aqua::UiApp,
        owner: aqua_core::NodeId,
        mut dialog: Dialog,
    ) -> Result<Self, String> {
        use aqua_ui::{
            overlay::{CloseReason, DismissPolicy},
            popup::{PopupFocus, PopupOptions},
        };
        let viewport = app.viewport();
        let theme = Rc::new(std::cell::Cell::new(dialog.theme));
        let bare = dialog.bare;
        let soft = dialog.soft_backdrop;
        let dismissible = dialog.dismissible;
        let callback = dialog.dismiss.take();
        let pending = Rc::new(std::cell::Cell::new(None));
        let closing = Rc::new(std::cell::Cell::new(None));
        let id = dialog.id.clone();
        let title = dialog.title.clone();
        let description = dialog.description.clone();
        let max_width = dialog.max_width;
        let viewport_width = dialog.viewport_width;
        let render_pending = pending.clone();
        let render_theme = Rc::clone(&theme);
        let rebuild: DialogRebuild = Rc::new(move |content, viewport| {
            let queue = render_pending.clone();
            let dialog = Dialog {
                id: id.clone(),
                theme: render_theme.get(),
                title: title.clone(),
                description: description.clone(),
                content,
                dismiss: Some(Rc::new(move |reason| queue.set(Some(reason)))),
                max_width,
                viewport_width,
                dismissible,
                bare,
                soft_backdrop: soft,
            };
            dialog_root(dialog, viewport, render_pending.clone())
        });
        let root = rebuild(dialog.content, viewport);
        let reason = closing.clone();
        let mut options = PopupOptions::dialog();
        options.placement.margin = 0.0;
        options.focus = PopupFocus::Root;
        options.dismiss = if dismissible {
            DismissPolicy::ESCAPE_ONLY
        } else {
            DismissPolicy::MANUAL
        };
        options.on_close = Some(Rc::new(move |closed| {
            let resolved = match closed {
                CloseReason::Escape => Some(DialogDismissReason::Escape),
                CloseReason::Requested => reason.take(),
                _ => None,
            };
            if let (Some(reason), Some(callback)) = (resolved, &callback) {
                callback(reason);
            }
        }));
        let overlay = app
            .open_popup(owner, root, options)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            overlay,
            pending,
            closing,
            theme,
            rebuild,
        })
    }

    pub fn set_theme(&self, theme: ArtisanTheme) {
        self.theme.set(theme);
    }

    pub fn refresh_content(&self, app: &mut aqua::UiApp, content: Element) -> Result<(), String> {
        let viewport = app.viewport();
        app.refresh_popup(self.overlay, (self.rebuild)(content, viewport))
            .map_err(|error| error.to_string())
    }

    pub fn dispose(self, app: &mut aqua::UiApp) {
        self.pending.set(None);
        self.closing.set(None);
        app.close_popup(self.overlay);
    }

    pub fn sync(&self, app: &mut aqua::UiApp) {
        if let Some(reason) = self.pending.take() {
            self.closing.set(Some(reason));
            if !app.close_popup(self.overlay) {
                self.closing.set(None);
            }
        }
    }

    pub fn handle_input(&self, app: &mut aqua::UiApp, event: &aqua_core::InputEvent) -> bool {
        let handled = app.handle_input(event);
        self.sync(app);
        handled
    }

    pub fn is_open(&self, app: &aqua::UiApp) -> bool {
        app.host().is_open(self.overlay)
    }
}

fn dialog_root(
    dialog: Dialog,
    viewport: Size,
    pending: Rc<std::cell::Cell<Option<DialogDismissReason>>>,
) -> Element {
    use aqua_core::DisplayItem;
    use aqua_ui::element::Canvas;
    let theme = dialog.theme;
    let bare = dialog.bare;
    let soft = dialog.soft_backdrop;
    let dismissible = dialog.dismissible;
    let panel = dialog.build_panel(viewport);
    let shade = Style::new().gpui_background(aqua_core::GpuiBackground::Solid(
        theme
            .colors
            .black
            .at(super::theme::Opacity::Shade)
            .to_reference_paint(),
    ));
    let mut root = UiBox::new().width(viewport.w).height(viewport.h).style(
        Style::new()
            .align(AlignItems::CENTER)
            .justify(JustifyContent::CENTER),
    );
    if !soft {
        root = root.style(if bare { shade } else { backdrop(&theme) });
    }
    if soft {
        let shade = theme
            .colors
            .black
            .at(super::theme::Opacity::SettingsBackdrop)
            .to_paint();
        let backdrop = Canvas::new()
            .style(Style::new().absolute_at(Some(0.0), Some(48.0), Some(0.0), Some(0.0)))
            .on_paint(move |bounds| {
                vec![
                    DisplayItem::BackdropBlur {
                        bounds,
                        radius: 4.0,
                    },
                    DisplayItem::Rect {
                        bounds,
                        radius: 0.0,
                        fill: shade,
                        border_width: 0.0,
                        border_color: aqua_core::Oklch::TRANSPARENT,
                        shadow: None,
                    },
                ]
            })
            .build();
        let backdrop = if dismissible {
            backdrop_events(backdrop, pending.clone())
        } else {
            backdrop
        };
        root = root.child(backdrop);
    }
    let mut root = root.child(panel).build();
    if dismissible {
        root = backdrop_events(root, pending.clone());
    }
    root
}

fn backdrop_events(
    element: Element,
    pending: Rc<std::cell::Cell<Option<DialogDismissReason>>>,
) -> Element {
    use aqua_core::{InputEvent, PointerButton};
    use aqua_ui::input::{EventPhase, InputKind, InputResponse, RoutedInput};
    let pressed = Rc::new(std::cell::Cell::new(false));
    let down = pressed.clone();
    element
        .on_input(InputKind::PointerDown, EventPhase::Bubble, move |event| {
            if event.target == event.current_target
                && matches!(
                    &event.input,
                    RoutedInput::Native(InputEvent::PointerDown(_, PointerButton::Primary))
                )
            {
                down.set(true);
            }
            InputResponse::default()
        })
        .on_input(InputKind::PointerUp, EventPhase::Bubble, move |event| {
            if pressed.replace(false)
                && event.target == event.current_target
                && matches!(
                    &event.input,
                    RoutedInput::Native(InputEvent::PointerUp(_, PointerButton::Primary))
                )
            {
                pending.set(Some(DialogDismissReason::Backdrop));
            }
            InputResponse::default()
        })
}
