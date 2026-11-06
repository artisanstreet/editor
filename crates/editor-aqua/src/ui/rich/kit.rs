use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Tone {
    #[default]
    Neutral,
    Info,
    Success,
    Warning,
    Danger,
    Accent,
}

pub(crate) fn sequential_color(
    theme: &crate::ui::theme::ArtisanTheme,
    t: f32,
) -> crate::ui::theme::Oklch {
    const STEPS: [usize; 7] = [3, 4, 5, 6, 7, 8, 11];
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the fraction is clamped to the ramp"
    )]
    let index = (t.clamp(0.0, 1.0) * 6.0).round() as usize;
    theme.colors.accent.step(STEPS[index])
}

pub(crate) fn diverging_color(
    theme: &crate::ui::theme::ArtisanTheme,
    t: f32,
) -> crate::ui::theme::Oklch {
    let colors = &theme.colors;
    let t = t.clamp(-1.0, 1.0);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the fraction is clamped to the ramp"
    )]
    let index = (t.abs() * 4.0).round() as usize;
    if index == 0 {
        return colors.raised;
    }
    let side = if t < 0.0 {
        [
            colors.red.bg,
            colors.red.border,
            colors.red.solid,
            colors.red.text,
        ]
    } else {
        [
            colors.accent.step(3),
            colors.accent.step(6),
            colors.accent.step(8),
            colors.accent.step(11),
        ]
    };
    side[index - 1]
}

pub(crate) fn format_number(value: f64) -> String {
    if !value.is_finite() {
        return "–".to_owned();
    }
    let magnitude = value.abs();
    let (scaled, suffix) = if magnitude >= 1e9 {
        (value / 1e9, "B")
    } else if magnitude >= 1e6 {
        (value / 1e6, "M")
    } else if magnitude >= 1e4 {
        (value / 1e3, "k")
    } else {
        (value, "")
    };
    let decimals = if scaled.abs() >= 100.0 || scaled.fract() == 0.0 {
        0
    } else if scaled.abs() >= 10.0 {
        1
    } else {
        2
    };
    let text = format!("{scaled:.decimals$}");
    let text = if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        text
    };
    format!("{text}{suffix}")
}

pub(crate) fn with_unit(value: &str, unit: Option<&str>) -> String {
    match unit.map(str::trim).filter(|unit| !unit.is_empty()) {
        Some(unit) if unit == "%" || unit == "x" || unit == "×" => format!("{value}{unit}"),
        Some(unit) => format!("{value} {unit}"),
        None => value.to_owned(),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CodeRef {
    pub(crate) path: String,
    pub(crate) start: Option<u32>,
    pub(crate) end: Option<u32>,
}

impl CodeRef {
    pub(crate) fn parse(text: &str) -> Self {
        let text = text.trim();
        let parsed = text.rsplit_once(':').and_then(|(path, lines)| {
            let (start, end) = match lines.split_once('-') {
                Some((start, end)) => (start.parse().ok()?, Some(end.parse().ok()?)),
                None => (lines.parse().ok()?, None),
            };
            Some((path, start, end))
        });
        match parsed {
            Some((path, start, end)) => Self {
                path: String::from(path.to_owned()),
                start: Some(start),
                end,
            },
            None => Self {
                path: String::from(text.to_owned()),
                start: None,
                end: None,
            },
        }
    }

    pub(crate) fn display(&self) -> String {
        match (self.start, self.end) {
            (Some(start), Some(end)) => format!("{}:{start}-{end}", self.path),
            (Some(start), None) => format!("{}:{start}", self.path),
            _ => self.path.to_string(),
        }
    }
}

impl<'de> Deserialize<'de> for CodeRef {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(|text| Self::parse(&text))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LineSpan {
    pub(crate) start: u32,
    pub(crate) end: u32,
}

impl LineSpan {
    pub(crate) fn contains(self, line: u32) -> bool {
        (self.start..=self.end).contains(&line)
    }
}

impl<'de> Deserialize<'de> for LineSpan {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Line(u32),
            Text(String),
            Pair([u32; 2]),
        }
        let invalid = || serde::de::Error::custom("expected a line like 4 or a range like \"3-5\"");
        match Raw::deserialize(deserializer)? {
            Raw::Line(line) => Ok(Self {
                start: line,
                end: line,
            }),
            Raw::Pair([start, end]) => Ok(Self {
                start: start.min(end),
                end: start.max(end),
            }),
            Raw::Text(text) => {
                let (start, end) = if let Some((start, end)) = text.trim().split_once('-') {
                    (
                        start.trim().parse().map_err(|_unparsable| invalid())?,
                        end.trim().parse().map_err(|_unparsable| invalid())?,
                    )
                } else {
                    let line = text.trim().parse().map_err(|_unparsable| invalid())?;
                    (line, line)
                };
                Ok(Self {
                    start: u32::min(start, end),
                    end: u32::max(start, end),
                })
            }
        }
    }
}
pub(crate) fn retained_vec<T>(values: &Vec<T>) -> usize {
    values.capacity() * std::mem::size_of::<T>()
}

pub(crate) fn retained_strings(values: &Vec<String>) -> usize {
    retained_vec(values) + values.iter().map(String::capacity).sum::<usize>()
}

pub(crate) fn retained_optional(value: Option<&String>) -> usize {
    value.map_or(0, String::capacity)
}
