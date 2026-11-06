use aqua_core::{GpuiTextColor, Transform2D};
use aqua_text::{StyledSpan, TextStyle, style::FontFeature};
use aqua_ui::{
    AlignItems, JustifyContent,
    element::{Box as UiBox, Element, Text},
    style::{Axis, Length, Style},
};

use super::motion::MotionCurve;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WordMotion {
    Still,
    Roll { from: String, up: bool },
    Extend { kept: usize },
    Enter,
}

#[derive(Clone, Debug)]
pub struct PhraseWord {
    pub text: String,
    pub motion: WordMotion,
    pub elapsed: f64,
}

#[derive(Default)]
pub struct PhraseMotion {
    current: Vec<String>,
    previous: Vec<String>,
    changed: Option<f64>,
    initialized: bool,
}

impl PhraseMotion {
    pub fn words(
        &mut self,
        text: &str,
        now: f64,
        reduced_motion: bool,
    ) -> (Option<Vec<PhraseWord>>, bool) {
        let words: Vec<String> = text.split(' ').map(str::to_owned).collect();
        if !self.initialized {
            self.current = words.clone();
            self.previous = words.clone();
            self.initialized = true;
        }
        if self.current != words {
            self.previous = std::mem::replace(&mut self.current, words);
            self.changed = (!reduced_motion).then_some(now);
        }
        if reduced_motion {
            self.changed = None;
            return (None, false);
        }
        let elapsed = self
            .changed
            .map(|started| (now - started).max(0.0))
            .filter(|elapsed| *elapsed < 0.66);
        if elapsed.is_none() {
            self.changed = None;
        }
        let previous = if elapsed.is_some() {
            &self.previous
        } else {
            &self.current
        };
        let elapsed = elapsed.unwrap_or(0.66);
        let words = self
            .current
            .iter()
            .enumerate()
            .map(|(index, text)| PhraseWord {
                text: text.clone(),
                motion: word_motion(previous.get(index).map(String::as_str), text),
                elapsed,
            })
            .collect();
        (Some(words), self.changed.is_some())
    }
}

fn word_motion(previous: Option<&str>, word: &str) -> WordMotion {
    let Some(previous) = previous else {
        return WordMotion::Enter;
    };
    if previous == word {
        return WordMotion::Still;
    }
    if let (Some((from_suffix, from_value)), Some((to_suffix, to_value))) =
        (number_word(previous), number_word(word))
    {
        if from_suffix == to_suffix {
            return WordMotion::Roll {
                from: previous.to_owned(),
                up: to_value >= from_value,
            };
        }
    }
    if word.starts_with(previous) {
        return WordMotion::Extend {
            kept: previous.len(),
        };
    }
    WordMotion::Enter
}

fn number_word(word: &str) -> Option<(&str, u64)> {
    if matches!(word, "a" | "an") {
        return Some(("", 1));
    }
    let end = word
        .char_indices()
        .find(|(_, character)| !character.is_ascii_digit())
        .map_or(word.len(), |(index, _)| index);
    if end == 0 {
        return None;
    }
    let (digits, suffix) = word.split_at(end);
    Some((suffix, digits.parse().ok()?))
}

fn progress(elapsed: f64, duration: f64, curve: MotionCurve) -> f32 {
    curve.sample_f32(elapsed / duration)
}

fn label(text: String, typography: &TextStyle, color: GpuiTextColor) -> Element {
    Text::new(text)
        .text_style(typography.clone())
        .gpui_source_color(color)
        .style(Style::new().flex_shrink(0.0))
        .build()
}

fn mask(start: bool) -> Element {
    aqua_ui::element::Canvas::new()
        .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
        .scroll_stable()
        .on_paint(move |bounds| {
            vec![if start {
                aqua_core::DisplayItem::PushGpuiContentMask { bounds }
            } else {
                aqua_core::DisplayItem::PopClip
            }]
        })
        .build()
}

pub fn phrase_or_text(
    words: Option<&[PhraseWord]>,
    text: String,
    typography: TextStyle,
    color: GpuiTextColor,
    line_height: f32,
) -> Element {
    match words {
        Some(words) => phrase_element(words, typography, color, line_height),
        None => label(text, &typography, color),
    }
}

pub fn phrase_element(
    words: &[PhraseWord],
    mut typography: TextStyle,
    color: GpuiTextColor,
    line_height: f32,
) -> Element {
    typography.font_features = vec![FontFeature {
        tag: *b"tnum",
        value: 1,
    }];
    let mut row = UiBox::new()
        .style(
            Style::new()
                .axis(Axis::Row)
                .align(AlignItems::CENTER)
                .min_width(Length::Px(0.0)),
        )
        .child(mask(true));
    for (index, word) in words.iter().enumerate() {
        if index > 0 {
            row = row.child(label(" ".to_owned(), &typography, color));
        }
        row = row.child(word_element(word, &typography, color, line_height));
    }
    row.child(mask(false)).build()
}

fn word_element(
    word: &PhraseWord,
    typography: &TextStyle,
    color: GpuiTextColor,
    line_height: f32,
) -> Element {
    match &word.motion {
        WordMotion::Still
            if word
                .text
                .chars()
                .any(|character| character.is_ascii_digit()) =>
        {
            let mut row = UiBox::new()
                .layout("row")
                .style(Style::new().flex_shrink(0.0));
            for character in word.text.chars() {
                row = row.child(label(character.to_string(), typography, color));
            }
            row.build()
        }
        WordMotion::Still => label(word.text.clone(), typography, color),
        WordMotion::Enter => swap_in(word.text.clone(), word.elapsed, typography, color),
        WordMotion::Extend { kept } => {
            let kept = (*kept).min(word.text.len());
            let amount = progress(word.elapsed, 0.15, MotionCurve::EaseInOut);
            let mut faded = color;
            faded.alpha *= amount;
            let span = |text: &str, color: GpuiTextColor| {
                StyledSpan::new(text, typography.clone()).gpui_source_color([
                    color.hue_degrees,
                    color.saturation,
                    color.lightness,
                    color.alpha,
                ])
            };
            Text::new(word.text.clone())
                .spans(vec![
                    span(&word.text[..kept], color),
                    span(&word.text[kept..], faded),
                ])
                .style(Style::new().flex_shrink(0.0))
                .build()
        }
        WordMotion::Roll { from, up } => odometer(
            from,
            &word.text,
            *up,
            word.elapsed,
            line_height,
            typography,
            color,
        ),
    }
}

fn swap_in(text: String, elapsed: f64, typography: &TextStyle, color: GpuiTextColor) -> Element {
    let progress = progress(elapsed, 0.15, MotionCurve::EaseInOut);
    UiBox::new()
        .style(
            Style::new()
                .flex_shrink(0.0)
                .opacity(progress)
                .transform(Transform2D::translate(0.0, 4.0 * (1.0 - progress)))
                .content_blur(2.0 * (1.0 - progress)),
        )
        .child(label(text, typography, color))
        .build()
}

fn odometer(
    from: &str,
    to: &str,
    up: bool,
    elapsed: f64,
    line_height: f32,
    typography: &TextStyle,
    color: GpuiTextColor,
) -> Element {
    let from_chars: Vec<char> = from.chars().collect();
    let to_chars: Vec<char> = to.chars().collect();
    let mut row = UiBox::new()
        .layout("row")
        .style(Style::new().flex_shrink(0.0));
    for (column, &target) in to_chars.iter().enumerate() {
        let from_right = to_chars.len() - 1 - column;
        let source = from_chars
            .len()
            .checked_sub(1 + from_right)
            .map(|index| from_chars[index]);
        let delay = f64::from(u32::try_from(from_right).unwrap_or(0)) * 0.04;
        let local = (elapsed - delay).max(0.0);
        row = row.child(match source {
            Some(source) if source == target => label(target.to_string(), typography, color),
            Some(source) => reel(source, target, up, local, line_height, typography, color),
            None => swap_in(target.to_string(), local, typography, color),
        });
    }
    row.build()
}

fn reel(
    source: char,
    target: char,
    up: bool,
    elapsed: f64,
    line_height: f32,
    typography: &TextStyle,
    color: GpuiTextColor,
) -> Element {
    let mut cells = vec![source];
    if let (Some(from), Some(to)) = (source.to_digit(10), target.to_digit(10)) {
        let step = if up { 1 } else { 9 };
        let mut digit = from;
        while digit != to {
            digit = (digit + step) % 10;
            cells.push(char::from_digit(digit, 10).unwrap_or(target));
        }
    } else {
        cells.push(target);
    }
    if !up {
        cells.reverse();
    }
    let steps = f32::from(u16::try_from(cells.len() - 1).unwrap_or(u16::MAX));
    let travel = line_height * steps;
    let progress = progress(elapsed, 0.5, MotionCurve::SmoothOut);
    let offset = if up {
        -travel * progress
    } else {
        -travel * (1.0 - progress)
    };
    let mut strip = UiBox::new().style(Style::new().axis(Axis::Column).absolute_at(
        Some(0.0),
        Some(offset),
        Some(0.0),
        None,
    ));
    for cell in cells {
        strip = strip.child(
            UiBox::new()
                .height(line_height)
                .style(
                    Style::new()
                        .align(AlignItems::CENTER)
                        .justify(JustifyContent::CENTER),
                )
                .child(label(cell.to_string(), typography, color))
                .build(),
        );
    }
    UiBox::new()
        .height(line_height)
        .style(Style::new().flex_shrink(0.0))
        .child(mask(true))
        .child(label(target.to_string(), typography, color).style(Style::new().opacity(0.0)))
        .child(strip.build())
        .child(mask(false))
        .build()
}
