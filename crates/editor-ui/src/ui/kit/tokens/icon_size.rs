#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum IconSize {
    Sm,
    Md,
    Lg,
}

impl IconSize {
    #[must_use]
    pub const fn px(self) -> f32 {
        match self {
            Self::Sm => 12.0,
            Self::Md => 16.0,
            Self::Lg => 20.0,
        }
    }
}
