use aqua_core::{GpuiTextColor, Transform2D};
use aqua_text::{StyledSpan, TextStyle, style::FontFeature};
use aqua_ui::{
    AlignItems, JustifyContent,
    element::{Box as UiBox, Element, Text},
    style::{Axis, Length, Style},
};

use std::cell::Cell;
use std::rc::Rc;

use super::motion::MotionCurve;

const ROLL_SECONDS: f64 = 0.33;
const ROLL_STAGGER_SECONDS: f64 = 0.04;
const SWAP_SECONDS: f64 = 0.15;
const SWAP_RISE_PX: f32 = 4.0;

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

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum PhraseFit {
    #[default]
    Whole,
    Cut {
        words: usize,
        at: f32,
    },
}

pub fn phrase_fit(edges: &[f32], available: f32, ellipsis: f32) -> PhraseFit {
    match edges.last() {
        Some(end) if *end > available + 0.5 => {
            let words = edges
                .iter()
                .take_while(|edge| **edge + ellipsis <= available)
                .count();
            PhraseFit::Cut {
                words,
                at: words.checked_sub(1).map_or(0.0, |last| edges[last]),
            }
        }
        _ => PhraseFit::Whole,
    }
}

#[derive(Clone)]
pub struct PhraseOverflow {
    pub fit: Rc<Cell<PhraseFit>>,
    pub refit: Option<Rc<dyn Fn()>>,
}

#[derive(Default)]
pub struct PhraseMotion {
    current: Vec<String>,
    previous: Vec<String>,
    changed: Option<f64>,
    initialized: bool,
    fit: Rc<Cell<PhraseFit>>,
}

impl PhraseMotion {
    pub fn fit(&self) -> Rc<Cell<PhraseFit>> {
        Rc::clone(&self.fit)
    }

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
            if matches!(self.fit.get(), PhraseFit::Cut { words: kept, .. } if words.len() < kept) {
                self.fit.set(PhraseFit::Whole);
            }
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

pub fn retained_phrase_or_text(
    words: Option<&[PhraseWord]>,
    text: String,
    mut typography: TextStyle,
    color: GpuiTextColor,
    line_height: f32,
    overflow: PhraseOverflow,
) -> Element {
    let Some(words) = words else {
        return Text::new(text)
            .text_style(typography)
            .gpui_source_color(color)
            .overflow(aqua_text::overflow::OverflowMode::EndEllipsis)
            .style(
                Style::new()
                    .min_width(Length::Px(0.0))
                    .max_height(Length::Px(line_height)),
            )
            .build();
    };
    typography.font_features = vec![FontFeature {
        tag: *b"tnum",
        value: 1,
    }];
    let (shown, ellipsis_at) = match overflow.fit.get() {
        PhraseFit::Whole => (words.len(), None),
        PhraseFit::Cut { words, at } => (words, Some(at)),
    };
    let mut row = UiBox::new()
        .style(
            Style::new()
                .axis(Axis::Row)
                .align(AlignItems::CENTER)
                .min_width(Length::Px(0.0)),
        )
        .child(fitting_mask(words.len(), overflow).with_key("mask-start"));
    for (index, word) in words.iter().enumerate() {
        if index > 0 {
            row = row.child(
                label(" ".to_owned(), &typography, color).with_key(format!("space-{index}")),
            );
        }
        let content = match &word.motion {
            WordMotion::Still
                if word
                    .text
                    .chars()
                    .any(|character| character.is_ascii_digit()) =>
            {
                retained_columns(&word.text, &typography, color)
            }
            WordMotion::Still => label(word.text.clone(), &typography, color),
            WordMotion::Enter => retained_enter(word.text.clone(), &typography, color),
            WordMotion::Extend { kept } => {
                retained_extension(&word.text, *kept, &typography, color)
            }
            WordMotion::Roll { from, up } => {
                retained_odometer(from, &word.text, *up, line_height, &typography, color)
            }
        };
        let style = Style::new().flex_shrink(0.0);
        row = row.child(
            UiBox::new()
                .style(if index < shown {
                    style
                } else {
                    style.opacity(0.0)
                })
                .child(content)
                .build()
                .with_key(format!("word-{index}")),
        );
    }
    row.child(
        label("\u{2026}".to_owned(), &typography, color)
            .style(
                Style::new()
                    .absolute_at(Some(ellipsis_at.unwrap_or(0.0)), Some(0.0), None, None)
                    .opacity(if ellipsis_at.is_some() { 1.0 } else { 0.0 }),
            )
            .with_key("ellipsis"),
    )
    .child(mask(false).with_key("mask-end"))
    .build()
}

fn fitting_mask(count: usize, overflow: PhraseOverflow) -> Element {
    aqua_ui::element::Canvas::new()
        .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), Some(0.0)))
        .scroll_stable()
        .on_paint(|bounds| vec![aqua_core::DisplayItem::PushGpuiContentMask { bounds }])
        .on_prepaint(move |tree, node| {
            let Some(row) = tree.get(node).and_then(|record| record.parent) else {
                return;
            };
            let (Some(bounds), Some(record)) = (tree.rect(row), tree.get(row)) else {
                return;
            };
            let rect = |index: usize| {
                record
                    .children
                    .get(index)
                    .and_then(|child| tree.rect(*child))
            };
            let edges: Option<Vec<f32>> = (0..count)
                .map(|word| rect(1 + 2 * word).map(|rect| rect.x + rect.w - bounds.x))
                .collect();
            let (Some(edges), Some(ellipsis)) = (edges, rect(2 * count)) else {
                return;
            };
            let fit = phrase_fit(&edges, bounds.w, ellipsis.w);
            if overflow.fit.replace(fit) != fit
                && let Some(refit) = &overflow.refit
            {
                refit();
            }
        })
        .build()
}

fn retained_enter(text: String, typography: &TextStyle, color: GpuiTextColor) -> Element {
    use aqua_ui::motion::{Easing, Motion, Presence};
    let key = format!("enter-{text}");
    UiBox::new()
        .style(Style::new().flex_shrink(0.0))
        .presence(
            Presence::new()
                .enter(Motion::tween(SWAP_SECONDS, Easing::EaseInOut))
                .slide(0.0, SWAP_RISE_PX),
        )
        .child(label(text, typography, color))
        .build()
        .with_key(key)
}

fn retained_extension(
    word: &str,
    kept: usize,
    typography: &TextStyle,
    color: GpuiTextColor,
) -> Element {
    let kept = kept.min(word.len());
    UiBox::new()
        .style(Style::new().flex_shrink(0.0))
        .child(label(word.to_owned(), typography, color).style(Style::new().opacity(0.0)))
        .child(
            UiBox::new()
                .layout("row")
                .style(Style::new().absolute_at(Some(0.0), Some(0.0), None, None))
                .child(label(word[..kept].to_owned(), typography, color))
                .child(retained_enter(word[kept..].to_owned(), typography, color))
                .build(),
        )
        .build()
}

fn retained_column(from_right: usize, content: Element) -> Element {
    UiBox::new()
        .style(Style::new().flex_shrink(0.0))
        .child(content)
        .build()
        .with_key(format!("column-{from_right}"))
}

fn retained_columns(text: &str, typography: &TextStyle, color: GpuiTextColor) -> Element {
    let count = text.chars().count();
    let mut row = UiBox::new()
        .layout("row")
        .style(Style::new().flex_shrink(0.0));
    for (column, character) in text.chars().enumerate() {
        row = row.child(retained_column(
            count - 1 - column,
            label(character.to_string(), typography, color),
        ));
    }
    row.build()
}

fn retained_odometer(
    from: &str,
    to: &str,
    up: bool,
    line_height: f32,
    typography: &TextStyle,
    color: GpuiTextColor,
) -> Element {
    let before: Vec<char> = from.chars().collect();
    let after: Vec<char> = to.chars().collect();
    let mut row = UiBox::new()
        .layout("row")
        .style(Style::new().flex_shrink(0.0));
    for (column, &target) in after.iter().enumerate() {
        let from_right = after.len() - 1 - column;
        let source = before
            .len()
            .checked_sub(1 + from_right)
            .map(|index| before[index]);
        let content = match source {
            Some(source) if source == target => label(target.to_string(), typography, color),
            Some(source) => retained_reel(
                source,
                target,
                up,
                from_right,
                line_height,
                typography,
                color,
            ),
            None => retained_enter(target.to_string(), typography, color),
        };
        row = row.child(retained_column(from_right, content));
    }
    row.build()
}

fn retained_reel(
    source: char,
    target: char,
    up: bool,
    from_right: usize,
    line_height: f32,
    typography: &TextStyle,
    color: GpuiTextColor,
) -> Element {
    use aqua_ui::motion::{Easing, Motion, Presence};
    let cells = reel_cells(source, target, up);
    let steps = f32::from(u16::try_from(cells.len() - 1).unwrap_or(u16::MAX));
    let travel = steps * line_height;
    let (rest, slide) = if up {
        (-travel, travel)
    } else {
        (0.0, -travel)
    };
    let stagger = f64::from(u32::try_from(from_right).unwrap_or(0)) * ROLL_STAGGER_SECONDS;
    let strip = UiBox::new()
        .style(
            Style::new()
                .axis(Axis::Column)
                .absolute_at(Some(0.0), Some(0.0), Some(0.0), None)
                .transform(Transform2D::translate(0.0, rest)),
        )
        .presence(
            Presence::new()
                .enter(Motion::tween(ROLL_SECONDS + stagger, Easing::EaseOut))
                .slide(0.0, slide)
                .without_fade(),
        )
        .children(cells.into_iter().map(|cell| {
            UiBox::new()
                .height(line_height)
                .style(
                    Style::new()
                        .align(AlignItems::CENTER)
                        .justify(JustifyContent::CENTER),
                )
                .child(label(cell.to_string(), typography, color))
                .build()
        }))
        .build()
        .with_key(format!("reel-{source}-{target}-{up}"));
    UiBox::new()
        .height(line_height)
        .style(Style::new().flex_shrink(0.0))
        .child(mask(true))
        .child(label(target.to_string(), typography, color).style(Style::new().opacity(0.0)))
        .child(strip)
        .child(mask(false))
        .build()
}

fn reel_cells(source: char, target: char, up: bool) -> Vec<char> {
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
    cells
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
    let cells = reel_cells(source, target, up);
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

#[cfg(test)]
#[path = "../../tests/ui/phrase.rs"]
mod tests;
