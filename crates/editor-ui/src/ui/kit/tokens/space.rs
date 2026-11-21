use aqua_ui::element::{Box as UiBox, Scroll};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Space {
    S2,
    S4,
    S6,
    S8,
    S12,
    S16,
    S20,
    S24,
    S32,
}

impl Space {
    #[must_use]
    pub const fn px(self) -> f32 {
        match self {
            Self::S2 => 2.0,
            Self::S4 => 4.0,
            Self::S6 => 6.0,
            Self::S8 => 8.0,
            Self::S12 => 12.0,
            Self::S16 => 16.0,
            Self::S20 => 20.0,
            Self::S24 => 24.0,
            Self::S32 => 32.0,
        }
    }
}

impl From<super::radius::Inset> for Space {
    fn from(inset: super::radius::Inset) -> Self {
        match inset {
            super::radius::Inset::Edge => Self::S4,
            super::radius::Inset::Menu => Self::S6,
            super::radius::Inset::Frame => Self::S8,
        }
    }
}

pub trait Spaced {
    #[must_use]
    fn spaced(self, space: Space) -> Self;

    #[must_use]
    fn padded(self, space: Space) -> Self;
}

impl Spaced for UiBox {
    fn spaced(self, space: Space) -> Self {
        self.gap(space.px())
    }

    fn padded(self, space: Space) -> Self {
        self.padding(space.px())
    }
}

impl Spaced for Scroll {
    fn spaced(self, space: Space) -> Self {
        self.gap(space.px())
    }

    fn padded(self, space: Space) -> Self {
        self.padding(space.px())
    }
}
