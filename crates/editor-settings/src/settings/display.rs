use std::time::Duration;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DisplayRate {
    #[default]
    Unknown,
    Hertz(u32),
}

impl DisplayRate {
    #[must_use]
    pub fn of(refresh: Option<Duration>) -> Self {
        match refresh.map(|interval| interval.as_nanos()) {
            Some(nanos) if nanos > 0 => Self::Hertz(
                u32::try_from((1_000_000_000 + nanos / 2) / nanos)
                    .expect("invariant: a refresh interval of at least 1 ns is at most 1 GHz"),
            ),
            Some(_) | None => Self::Unknown,
        }
    }

    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::Hertz(hertz) => format!("{hertz} Hz"),
            Self::Unknown => "Native".to_owned(),
        }
    }
}
