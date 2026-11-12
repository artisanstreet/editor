pub mod band;
pub mod glass;

use crate::ui::chrome::card::{Surfaces, outlined};
use crate::ui::kit::tokens::{Corner, Elevation, Inset, Radius, Space};
use crate::ui::theme::{ArtisanTheme, Opacity};
use aqua_core::GpuiBackground;
use aqua_ui::element::{Box as UiBox, Element};
use aqua_ui::style::{Length, Style};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Tooltip {
    Brief,
    Card,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Media {
    Frame,
    Thumb,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Tone {
    Neutral,
    Info,
    Success,
    Warning,
    Danger,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Material {
    Glass,
    Recessed,
    Key,
    Raised,
    Clip,
    Tint(Tone),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Surface {
    Menu,
    Tooltip(Tooltip),
    Dialog,
    Card,
    Inset,
    Composer,
    Field,
    Media(Media),
    Callout(Tone),
    Pill(f32),
    Bubble,
}

impl Surface {
    #[must_use]
    pub const fn corner(self) -> Corner {
        match self {
            Self::Menu | Self::Tooltip(_) | Self::Inset | Self::Callout(_) => {
                Corner::Fixed(Radius::Panel)
            }
            Self::Media(Media::Frame) => Corner::Fixed(Radius::Panel),
            Self::Dialog | Self::Card | Self::Bubble => Corner::Fixed(Radius::Shell),
            Self::Composer => Corner::Fixed(Radius::Well),
            Self::Field | Self::Media(Media::Thumb) => Corner::Fixed(Radius::Field),
            Self::Pill(_) => Corner::Full,
        }
    }

    #[must_use]
    pub const fn padding(self) -> Option<Space> {
        match self {
            Self::Menu | Self::Inset | Self::Pill(_) => Some(Space::S6),
            Self::Tooltip(Tooltip::Brief) | Self::Dialog | Self::Card | Self::Composer => {
                Some(Space::S8)
            }
            Self::Field => Some(Space::S8),
            Self::Callout(_) | Self::Bubble => Some(Space::S12),
            Self::Tooltip(Tooltip::Card) => Some(Space::S16),
            Self::Media(_) => None,
        }
    }

    #[must_use]
    pub const fn material(self) -> Material {
        match self {
            Self::Menu
            | Self::Tooltip(_)
            | Self::Dialog
            | Self::Pill(_)
            | Self::Composer
            | Self::Card => Material::Glass,
            Self::Inset => Material::Recessed,
            Self::Field => Material::Key,
            Self::Bubble => Material::Raised,
            Self::Media(_) => Material::Clip,
            Self::Callout(tone) => Material::Tint(tone),
        }
    }

    #[must_use]
    pub const fn elevation(self) -> Elevation {
        match self {
            Self::Menu
            | Self::Tooltip(_)
            | Self::Dialog
            | Self::Pill(_)
            | Self::Composer
            | Self::Card => Elevation::Overlay,
            Self::Inset | Self::Bubble | Self::Media(Media::Frame) => Elevation::Rest,
            Self::Field => Elevation::Key,
            Self::Media(Media::Thumb) | Self::Callout(_) => Elevation::Flat,
        }
    }

    #[must_use]
    pub fn padding_px(self) -> f32 {
        self.padding().map_or(0.0, Space::px)
    }

    #[must_use]
    pub fn radius_px(self) -> f32 {
        match self {
            Self::Pill(height) => Corner::Full.resolve(height),
            _ => self.corner().resolve(0.0),
        }
    }

    #[must_use]
    pub fn child_radius(self) -> Option<Radius> {
        let Corner::Fixed(radius) = self.corner() else {
            return None;
        };
        Inset::of(self.padding_px()).and_then(|inset| radius.inset(inset))
    }

    #[must_use]
    pub fn build(self, theme: &ArtisanTheme, children: Vec<Element>) -> Element {
        let radius = self.radius_px();
        let shadows = self.elevation().shadows(theme);
        let surfaces = Surfaces::of(theme);
        let padding = Style::new().padding(Length::Px(self.padding_px()));
        let sized = match self {
            Self::Pill(height) => padding.height(Length::Px(height)),
            _ => padding,
        };
        let shell = || {
            UiBox::new()
                .layout("column")
                .radius(radius)
                .style(Style::new().shadows(shadows.clone()))
        };
        match self.material() {
            Material::Glass => shell()
                .child(glass::paint(theme, radius))
                .children(children)
                .build()
                .style(sized),
            Material::Recessed => {
                outlined(surfaces.ink, surfaces.inner, radius, shadows, children).style(sized)
            }
            Material::Key => {
                outlined(surfaces.ink, surfaces.bg, radius, shadows, children).style(sized)
            }
            Material::Raised => shell()
                .style(Style::new().gpui_background(GpuiBackground::Solid(
                    theme.colors.raised.to_reference_paint(),
                )))
                .children(children)
                .build()
                .style(sized),
            Material::Clip => shell().clip(true).children(children).build().style(sized),
            Material::Tint(tone) => shell()
                .style(Style::new().gpui_background(GpuiBackground::Solid(
                    tint(theme, tone).at(Opacity::ToneTint).to_reference_paint(),
                )))
                .children(children)
                .build()
                .style(sized),
        }
    }
}

#[must_use]
pub fn tint(theme: &ArtisanTheme, tone: Tone) -> crate::ui::theme::Oklch {
    match tone {
        Tone::Neutral => theme.colors.text,
        Tone::Info => theme.colors.banner_info.text,
        Tone::Success => theme.colors.banner_success.text,
        Tone::Warning => theme.colors.banner_warning.text,
        Tone::Danger => theme.colors.banner_error.text,
    }
}

#[must_use]
pub fn backdrop(theme: &ArtisanTheme) -> Style {
    Style::new().gpui_background(GpuiBackground::Solid(
        theme.colors.background.at(Opacity::Backdrop).to_reference_paint(),
    ))
}

#[must_use]
pub fn frame(theme: &ArtisanTheme) -> Style {
    Style::new().gpui_background(GpuiBackground::Solid(
        theme.colors.frame.to_reference_paint(),
    ))
}
