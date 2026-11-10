pub(crate) mod view;

use super::SizeHint;
use crate::ui::media::source::MediaSource;
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum Mode {
    #[default]
    Slider,
    SideBySide,
    Toggle,
    Blend,
}

impl Mode {
    const ALL: [Self; 4] = [Self::Slider, Self::SideBySide, Self::Toggle, Self::Blend];

    const fn label(self) -> &'static str {
        match self {
            Self::Slider => "Slider",
            Self::SideBySide => "Side by side",
            Self::Toggle => "Toggle",
            Self::Blend => "Blend",
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct ImageCompare {
    before: Side,
    after: Side,
    #[serde(default)]
    mode: Mode,
    #[serde(flatten)]
    hint: SizeHint,
}
impl ImageCompare {
    pub(crate) fn resident_bytes(&self) -> usize {
        use crate::ui::rich::kit::retained_optional;
        std::mem::size_of::<Self>()
            + self.before.src.resident_bytes()
            + self.after.src.resident_bytes()
            + retained_optional(self.before.label.as_ref())
            + retained_optional(self.after.label.as_ref())
    }
}

#[derive(Clone, Debug, Deserialize)]
struct Side {
    src: MediaSource,
    #[serde(default)]
    label: Option<String>,
}

impl Side {
    fn label_or(&self, default: &str) -> String {
        String::from(
            self.label
                .as_deref()
                .map(str::trim)
                .filter(|label| !label.is_empty())
                .unwrap_or(default)
                .to_owned(),
        )
    }
}
