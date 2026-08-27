//! Controlled native GPUI switch primitive for the reached settings surfaces.

use gpui::{
    App, BoxShadow, ClickEvent, DefiniteLength, Div, ElementId, FocusHandle, Hsla,
    InteractiveElement, IntoElement, Length, ParentElement, Pixels, RenderOnce, SharedString,
    StatefulInteractiveElement, StyleRefinement, Styled, Window, div, point, px, transparent_black,
};

use crate::button::FocusVisibility;
use crate::theme::{ArtisanTheme, ThemeMode};

const BORDER_WIDTH_PX: f32 = 1.0;
const HIT_INSET_HORIZONTAL_PX: f32 = 12.0;
const HIT_INSET_VERTICAL_PX: f32 = 8.0;
const PILL_RADIUS_PX: f32 = 9999.0;

/// The reached switch dimensions.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum SwitchSize {
    /// A 32 × 18.4 px track with a 16 px thumb.
    #[default]
    Default,
    /// A 24 × 14 px track with a 12 px thumb.
    Small,
}

/// Theme-resolved geometry and paint for one switch state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwitchStyle {
    /// The visible track width.
    pub track_width: Pixels,
    /// The visible track height.
    pub track_height: Pixels,
    /// The circular thumb edge length.
    pub thumb_size: Pixels,
    /// The checked thumb's exact relative travel.
    pub checked_travel: Pixels,
    /// The pill radius used by the track and thumb.
    pub corner_radius: Pixels,
    /// The transparent track border width.
    pub border_width: Pixels,
    /// The transparent track border paint.
    pub border_color: Hsla,
    /// The resolved track paint.
    pub track_color: Hsla,
    /// The resolved thumb paint.
    pub thumb_color: Hsla,
    /// The horizontal hit-target inset around the visible track.
    pub hit_inset_horizontal: Pixels,
    /// The vertical hit-target inset around the visible track.
    pub hit_inset_vertical: Pixels,
    /// The disabled opacity.
    pub disabled_opacity: f32,
}

impl SwitchStyle {
    /// Resolves the legacy switch recipe from the shared theme and size.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme, size: SwitchSize, checked: bool) -> Self {
        let (track_width, track_height) = match size {
            SwitchSize::Default => theme.density.switch_default,
            SwitchSize::Small => theme.density.switch_sm,
        };
        let thumb_size = match size {
            SwitchSize::Default => px(16.0),
            SwitchSize::Small => px(12.0),
        };
        let checked_travel = px(f32::from(track_width) - f32::from(thumb_size) - 2.0);

        let (track_color, thumb_color) = match (theme.mode, checked) {
            (ThemeMode::Light, false) => (
                theme.colors.input.to_paint(),
                theme.colors.background.to_paint(),
            ),
            (ThemeMode::Light, true) => (
                theme.colors.primary.to_paint(),
                theme.colors.background.to_paint(),
            ),
            (ThemeMode::Dark, false) => (
                theme.colors.input.with_alpha(0.8).to_paint(),
                theme.colors.foreground.to_paint(),
            ),
            (ThemeMode::Dark, true) => (
                theme.colors.primary.to_paint(),
                theme.colors.primary_foreground.to_paint(),
            ),
        };

        Self {
            track_width,
            track_height,
            thumb_size,
            checked_travel,
            corner_radius: px(PILL_RADIUS_PX),
            border_width: px(BORDER_WIDTH_PX),
            border_color: transparent_black(),
            track_color,
            thumb_color,
            hit_inset_horizontal: px(HIT_INSET_HORIZONTAL_PX),
            hit_inset_vertical: px(HIT_INSET_VERTICAL_PX),
            disabled_opacity: 0.5,
        }
    }
}

type ChangeHandler = Box<dyn Fn(bool, &ClickEvent, &mut Window, &mut App)>;

/// A controlled, focusable native switch with no visible text child.
#[derive(IntoElement)]
pub struct Switch {
    id: ElementId,
    focus: FocusHandle,
    theme: ArtisanTheme,
    size: SwitchSize,
    checked: bool,
    disabled: bool,
    focus_visibility: FocusVisibility,
    on_change: Option<ChangeHandler>,
    track: Div,
    thumb: Div,
    debug_selector: Option<SharedString>,
}

impl Switch {
    /// Constructs a switch from the caller-owned checked value.
    #[must_use]
    pub fn new(
        id: impl Into<ElementId>,
        focus: FocusHandle,
        theme: ArtisanTheme,
        size: SwitchSize,
        checked: bool,
    ) -> Self {
        let style = SwitchStyle::resolve(theme, size, checked);
        let track = div()
            .flex()
            .flex_row()
            .items_center()
            .flex_shrink_0()
            .w(style.track_width)
            .h(style.track_height)
            .rounded(style.corner_radius)
            .border_1()
            .border_color(style.border_color)
            .bg(style.track_color);
        let thumb = div()
            .flex_shrink_0()
            .w(style.thumb_size)
            .h(style.thumb_size)
            .rounded(style.corner_radius)
            .bg(style.thumb_color);

        Self {
            id: id.into(),
            focus,
            theme,
            size,
            checked,
            disabled: false,
            focus_visibility: FocusVisibility::Hidden,
            on_change: None,
            track,
            thumb,
            debug_selector: None,
        }
    }

    /// Selects the disabled presentation and suppresses every interaction.
    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Selects whether actual focus should receive a visible ring.
    #[must_use]
    pub const fn focus_visibility(mut self, visibility: FocusVisibility) -> Self {
        self.focus_visibility = visibility;
        self
    }

    /// Installs the callback with the next controlled checked value.
    #[must_use]
    pub fn on_change(
        mut self,
        handler: impl Fn(bool, &ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(Box::new(handler));
        self
    }

    /// Adds a stable selector to the interactive hitbox.
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.debug_selector = Some(selector.into());
        self
    }

    /// Returns the controlled checked value retained by this render recipe.
    #[must_use]
    pub const fn checked(&self) -> bool {
        self.checked
    }

    /// Returns the resolved theme and geometry recipe for this value.
    #[must_use]
    pub fn visual_style(&self) -> SwitchStyle {
        SwitchStyle::resolve(self.theme, self.size, self.checked)
    }

    /// Whether this switch should paint its focus ring now.
    #[must_use]
    pub fn focus_ring_visible(&self, window: &Window) -> bool {
        !self.disabled
            && self.focus_visibility == FocusVisibility::Visible
            && self.focus.is_focused(window)
    }
}

impl Styled for Switch {
    fn style(&mut self) -> &mut StyleRefinement {
        self.track.style()
    }
}

fn absolute_track_dimension(length: Option<Length>, fallback: Pixels, rem_size: Pixels) -> Pixels {
    match length {
        Some(Length::Definite(DefiniteLength::Absolute(absolute))) => absolute.to_pixels(rem_size),
        None | Some(Length::Auto | Length::Definite(DefiniteLength::Fraction(_))) => fallback,
    }
}

impl RenderOnce for Switch {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let style = self.visual_style();
        let disabled = self.disabled;
        let focus_visibility = self.focus_visibility;
        let focus = self.focus.clone();
        let checked = self.checked;
        let theme = self.theme;
        let on_change = self.on_change;

        let mut track = self.track;
        let track_width = absolute_track_dimension(
            track.style().size.width,
            style.track_width,
            window.rem_size(),
        );
        let track_height = absolute_track_dimension(
            track.style().size.height,
            style.track_height,
            window.rem_size(),
        );
        let mut thumb = self.thumb.relative().left(if checked {
            style.checked_travel
        } else {
            px(0.0)
        });

        let hitbox_selector = self.debug_selector;
        if let Some(selector) = hitbox_selector.as_ref() {
            let selector = selector.clone();
            let track_selector = format!("{selector}-track");
            let thumb_selector = format!("{selector}-thumb");
            track = track.debug_selector(move || track_selector);
            thumb = thumb.debug_selector(move || thumb_selector);
        }

        track = track.child(thumb);

        let mut hitbox = div()
            .id(self.id)
            .flex()
            .flex_row()
            .items_center()
            .justify_center()
            .flex_shrink_0()
            .w(track_width + style.hit_inset_horizontal * 2.0)
            .h(track_height + style.hit_inset_vertical * 2.0)
            .px(style.hit_inset_horizontal)
            .py(style.hit_inset_vertical)
            .mx(-style.hit_inset_horizontal)
            .my(-style.hit_inset_vertical);

        if let Some(selector) = hitbox_selector {
            hitbox = hitbox.debug_selector(move || selector.to_string());
        }

        if disabled {
            return hitbox.opacity(style.disabled_opacity).child(track);
        }

        if focus_visibility == FocusVisibility::Visible {
            let focus_border = theme.colors.ring.to_paint();
            let focus_ring = theme.interaction.focus_ring_color.to_paint();
            let focus_ring_width = theme.interaction.focus_ring_width;
            track = track
                .focus(move |focused| {
                    focused.border_color(focus_border).shadow(vec![BoxShadow {
                        color: focus_ring,
                        offset: point(px(0.0), px(0.0)),
                        blur_radius: px(0.0),
                        spread_radius: focus_ring_width,
                    }])
                })
                .track_focus(&focus);
        }

        hitbox = hitbox.track_focus(&focus);
        if let Some(handler) = on_change {
            hitbox = hitbox.on_click(move |event, window, cx| {
                handler(!checked, event, window, cx);
            });
        }

        hitbox.child(track)
    }
}
