#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Inset {
    Edge,
    Menu,
    Frame,
}

impl Inset {
    pub const ALL: [Self; 3] = [Self::Edge, Self::Menu, Self::Frame];

    #[must_use]
    pub const fn px(self) -> f32 {
        match self {
            Self::Edge => 4.0,
            Self::Menu => 6.0,
            Self::Frame => 8.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Radius {
    Row,
    Field,
    Panel,
    Well,
    Shell,
}

const SHELL_PX: f32 = 18.4;

impl Radius {
    pub const ALL: [Self; 5] = [Self::Row, Self::Field, Self::Panel, Self::Well, Self::Shell];

    #[must_use]
    pub const fn parent(self) -> Option<(Self, Inset)> {
        match self {
            Self::Row => Some((Self::Panel, Inset::Menu)),
            Self::Field => Some((Self::Panel, Inset::Edge)),
            Self::Panel => Some((Self::Shell, Inset::Frame)),
            Self::Well => Some((Self::Shell, Inset::Edge)),
            Self::Shell => None,
        }
    }

    #[must_use]
    pub const fn px(self) -> f32 {
        match self.parent() {
            Some((outer, inset)) => outer.px() - inset.px(),
            None => SHELL_PX,
        }
    }

    #[must_use]
    pub fn inset(self, inset: Inset) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|radius| radius.parent() == Some((self, inset)))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Corner {
    Fixed(Radius),
    Nested(f32),
    Full,
}

impl Corner {
    #[must_use]
    pub fn resolve(self, height: f32) -> f32 {
        match self {
            Self::Fixed(radius) => radius.px(),
            Self::Nested(px) => px,
            Self::Full => (height / 2.0).max(0.0),
        }
    }
}

#[must_use]
pub fn concentric(outer: f32, inset: f32) -> f32 {
    (outer - inset).max(0.0)
}
